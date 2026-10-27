//! Safety cases: the born-closed Config and its governance, revoke on cancel and close, rollback after a late Core CPI
//! failure, account substitution, pre-funded Listed PDAs and offer-id reuse. Real Core binary.

mod common;

use anchor_lang::{prelude::Pubkey, solana_program::system_program, Space};
use common::*;
use mpl_core::{
    instructions::{BurnV1Builder, CreateV1Builder},
    types::{
        DataState, PermanentFreezeDelegate, Plugin, PluginAuthority, PluginAuthorityPair, RuleSet,
    },
};
use mud_trade_book::{
    error::TradeError,
    state::{Listed, MakerPermit, Slot},
    FINISH_ALL,
};
use solana_keypair::Keypair;
use solana_signer::Signer;

fn wallets(env: &Env) -> Vec<Pubkey> {
    vec![
        env.maker.pubkey(),
        env.taker.pubkey(),
        env.stranger.pubkey(),
        env.admin.pubkey(),
    ]
}

fn week(env: &Env) -> i64 {
    env.now() + 7 * DAY
}

/// The card still carries a delegation to some address (its own offer's, or a new owner's).
fn live(env: &Env, a: &Pubkey) -> bool {
    matches!(
        env.read(a).transfer_delegate,
        Some(PluginAuthority::Address { .. })
    )
}

#[track_caller]
fn assert_revoked(env: &Env, a: &Pubkey) {
    assert!(
        matches!(env.read(a).transfer_delegate, Some(PluginAuthority::Owner)),
        "delegation on {a} not returned to the owner: {:?}",
        env.read(a).transfer_delegate
    );
}

fn keys(env: &Env) -> (Keypair, Keypair, Keypair, Keypair) {
    (
        env.admin.insecure_clone(),
        env.maker.insecure_clone(),
        env.taker.insecure_clone(),
        env.stranger.insecure_clone(),
    )
}

// ---------------------------------------------------------------- Config and governance

#[test]
fn g01_config_born_closed_permits_and_public() {
    let mut env = Env::raw(RuleSet::None);
    let (admin, maker, taker, stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), taker.pubkey());
    let c = env.config();
    assert_eq!(c.admin, admin.pubkey());
    assert!(c.paused && !c.public && !c.cards_enabled);
    assert_eq!(c.pending_admin, None);

    let a = env.mint(&m, 31, 1);
    let b = env.mint(&t, 9, 1);
    env.add_delegate(a, &maker, &env.offer_pda(&m, 1));
    let exp = week(&env);
    let exact = [Slot::Asset(b)];

    let res = env.create_offer(&maker, 1, &[a], &exact, exp).res;
    assert_trade_err(&res, TradeError::Paused);
    assert!(env.set_paused(&admin, false).res.is_ok());
    let res = env.create_offer(&maker, 1, &[a], &exact, exp).res;
    assert_trade_err(&res, TradeError::NotPermitted);
    assert!(!env.exists(&env.offer_pda(&m, 1)));

    assert_trade_err(&env.grant_permit(&stranger, m).res, TradeError::NotAdmin);
    let admin_before = env.lamports(&admin.pubkey());
    let granted = env.grant_permit(&admin, m).res;
    assert!(granted.is_ok(), "{granted:#?}");
    let permit = env.permit_pda(&m);
    let permit_rent = env
        .svm
        .minimum_balance_for_rent_exemption(8 + MakerPermit::INIT_SPACE);
    assert_eq!(env.lamports(&permit), permit_rent);
    assert_eq!(
        env.lamports(&admin.pubkey()),
        admin_before - fee_of(&granted) - permit_rent
    );
    assert!(
        env.grant_permit(&admin, m).res.is_err(),
        "a second grant must fail"
    );

    let res = env
        .create_offer(&maker, 1, &[a], &[cards_slot(&[10], FINISH_ALL)], exp)
        .res;
    assert_trade_err(&res, TradeError::CardsDisabled);
    let res = env.create_offer(&maker, 1, &[a], &exact, exp).res;
    assert!(res.is_ok(), "{res:#?}");
    let offer = env.offer_pda(&m, 1);

    // Removing the permit stops new offers only; the open one still fills.
    assert!(env.grant_permit(&admin, t).res.is_ok());
    assert_trade_err(&env.revoke_permit(&stranger, t).res, TradeError::NotAdmin);
    let admin_before = env.lamports(&admin.pubkey());
    let revoked = env.revoke_permit(&admin, m).res;
    assert!(revoked.is_ok(), "{revoked:#?}");
    assert!(!env.exists(&permit));
    assert_eq!(
        env.lamports(&admin.pubkey()),
        admin_before + permit_rent - fee_of(&revoked)
    );
    let c2 = env.mint(&m, 32, 2);
    env.add_delegate(c2, &maker, &env.offer_pda(&m, 2));
    let res = env.create_offer(&maker, 2, &[c2], &exact, exp).res;
    assert_trade_err(&res, TradeError::NotPermitted);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert!(res.is_ok(), "{res:#?}");
    assert_eq!(env.owner_of(&a), t);
    assert_eq!(env.owner_of(&b), m);

    // One way: once public, no permit is needed and it cannot be reopened.
    assert_trade_err(&env.open_public(&stranger).res, TradeError::NotAdmin);
    assert!(env.open_public(&admin).res.is_ok());
    assert_trade_err(&env.open_public(&admin).res, TradeError::AlreadyPublic);
    assert!(env.config().public);
    let d = env.mint(&t, 10, 5);
    let res = env
        .create_offer(&maker, 2, &[c2], &[Slot::Asset(d)], exp)
        .res;
    assert!(res.is_ok(), "{res:#?}");
}

#[test]
fn g02_cards_gate_holds_at_fill() {
    let mut env = Env::new(RuleSet::None);
    let (admin, maker, taker, stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), taker.pubkey());
    let a = env.mint(&m, 31, 1);
    let b = env.mint(&t, 9, 1);
    let exp = week(&env);
    let set = [cards_slot(&[10], FINISH_ALL)];
    let offer = env.list(&maker, 1, &[a], &set, exp);

    assert_trade_err(
        &env.set_cards_enabled(&stranger, false).res,
        TradeError::NotAdmin,
    );
    assert!(env.set_cards_enabled(&admin, false).res.is_ok());
    assert!(!env.config().cards_enabled);
    env.prepare_cards(&offer, &[b]);
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_trade_err(&res, TradeError::CardsDisabled);
    env.assert_nothing_moved(&before, &t, &res);

    // The maker can still get out, and the delegation goes with the offer.
    let res = env.cancel(&maker, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
    assert_revoked(&env, &a);

    assert!(env.set_cards_enabled(&admin, true).res.is_ok());
    let offer = env.list(&maker, 2, &[a], &set, exp);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert!(res.is_ok(), "{res:#?}");
    assert_eq!(env.owner_of(&a), t);
    assert_eq!(env.owner_of(&b), m);
}

#[test]
fn g03_two_step_admin_transfer() {
    let mut env = Env::raw(RuleSet::None);
    let (admin, _maker, taker, stranger) = keys(&env);
    let next = Keypair::new();
    env.svm.airdrop(&next.pubkey(), 10 * SOL).unwrap();

    assert_trade_err(
        &env.propose_admin(&admin, Pubkey::default()).res,
        TradeError::BadNewAdmin,
    );
    assert_trade_err(
        &env.propose_admin(&admin, admin.pubkey()).res,
        TradeError::BadNewAdmin,
    );
    assert_trade_err(
        &env.propose_admin(&stranger, next.pubkey()).res,
        TradeError::NotAdmin,
    );
    assert_trade_err(&env.accept_admin(&next).res, TradeError::NoPendingAdmin);
    assert_trade_err(
        &env.cancel_admin_transfer(&admin).res,
        TradeError::NoPendingAdmin,
    );

    // A proposal changes nothing until accepted, and a later proposal replaces it.
    assert!(env.propose_admin(&admin, taker.pubkey()).res.is_ok());
    assert!(env.propose_admin(&admin, next.pubkey()).res.is_ok());
    assert_eq!(env.config().pending_admin, Some(next.pubkey()));
    assert_trade_err(&env.accept_admin(&taker).res, TradeError::NotPendingAdmin);
    assert_trade_err(
        &env.accept_admin(&stranger).res,
        TradeError::NotPendingAdmin,
    );
    assert!(env.set_paused(&admin, false).res.is_ok());
    assert_trade_err(&env.set_paused(&next, true).res, TradeError::NotAdmin);

    assert_trade_err(
        &env.cancel_admin_transfer(&stranger).res,
        TradeError::NotAdmin,
    );
    assert!(env.cancel_admin_transfer(&admin).res.is_ok());
    assert_eq!(env.config().pending_admin, None);
    assert_trade_err(&env.accept_admin(&next).res, TradeError::NoPendingAdmin);

    assert!(env.propose_admin(&admin, next.pubkey()).res.is_ok());
    assert!(env.accept_admin(&next).res.is_ok());
    let c = env.config();
    assert_eq!(c.admin, next.pubkey());
    assert_eq!(c.pending_admin, None);
    assert_trade_err(&env.set_paused(&admin, true).res, TradeError::NotAdmin);
    assert_trade_err(&env.open_public(&admin).res, TradeError::NotAdmin);
    assert_trade_err(
        &env.grant_permit(&admin, stranger.pubkey()).res,
        TradeError::NotAdmin,
    );
    assert_trade_err(&env.accept_admin(&next).res, TradeError::NoPendingAdmin);
    assert!(env.set_paused(&next, true).res.is_ok());
    assert!(env.propose_admin(&next, admin.pubkey()).res.is_ok());
    assert!(env.accept_admin(&admin).res.is_ok());
    assert_eq!(env.config().admin, admin.pubkey());
}

#[test]
fn g04_init_config_guards() {
    let mut env = Env::bare(RuleSet::None);
    let (admin, _maker, _taker, stranger) = keys(&env);
    let col = env.collection;

    assert_trade_err(
        &env.init_config(&stranger, col).res,
        TradeError::NotUpgradeAuthority,
    );
    // Another program's ProgramData whose upgrade authority is the signer: the right key, the
    // wrong program.
    set_upgrade_authority(&mut env.svm, &CORE_ID, &admin.pubkey());
    let core_pd = Pubkey::find_program_address(
        &[CORE_ID.as_ref()],
        &anchor_lang::solana_program::bpf_loader_upgradeable::ID,
    )
    .0;
    assert_trade_err(
        &env.init_config_with(&admin, col, core_pd).res,
        TradeError::NotUpgradeAuthority,
    );
    let asset = env.mint(&admin.pubkey(), 1, 1);
    assert_trade_err(
        &env.init_config(&admin, asset).res,
        TradeError::WrongCollection,
    );
    assert_trade_err(
        &env.init_config(&admin, admin.pubkey()).res,
        TradeError::WrongCollection,
    );
    assert!(!env.exists(&env.config));

    assert!(env.init_config(&admin, col).res.is_ok());
    let first = env.svm.get_account(&env.config).unwrap();
    let other = env.create_collection(RuleSet::None);
    assert!(
        env.init_config(&admin, other).res.is_err(),
        "init_config ran twice"
    );
    assert_eq!(env.svm.get_account(&env.config).unwrap(), first);
    let c = env.config();
    assert!(c.paused && !c.public && !c.cards_enabled);
    assert_eq!(c.collection, col);
}

/// Core accounts of the wrong type used to panic the SBF heap (out of memory) instead of
/// returning an error. Both directions, through the real binary.
#[test]
fn g05_wrong_core_account_type_is_an_error_not_a_panic() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, taker, _stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), taker.pubkey());
    let col = env.collection;
    let a = env.mint(&m, 1, 1);
    let b = env.mint(&t, 9, 1);
    env.add_delegate(a, &maker, &env.offer_pda(&m, 1));

    let res = env
        .create_offer(&maker, 1, &[col], &[Slot::Asset(b)], week(&env))
        .res;
    assert_trade_err(&res, TradeError::NotCoreAsset);
    let res = env
        .create_offer(&maker, 1, &[a], &[Slot::Asset(col)], week(&env))
        .res;
    assert!(res.is_ok(), "an exact want is only a key: {res:#?}");
    let offer = env.offer_pda(&m, 1);
    let ix = env.fill_ix_with(&t, &offer, &[a], &[col], CORE_ID);
    let res = env.send(&[ix], &taker, &[]).res;
    assert_trade_err(&res, TradeError::NotCoreAsset);

    let ix = env.create_offer_ix(&m, 2, &[a], &[cards_slot(&[10], FINISH_ALL)], week(&env));
    assert!(
        env.send(&[ix], &maker, &[]).res.is_err(),
        "a is already listed"
    );
    let c = env.mint(&m, 2, 1);
    let offer = env.list(
        &maker,
        2,
        &[c],
        &[cards_slot(&[10], FINISH_ALL)],
        week(&env),
    );
    let ix = env.fill_ix_with(&t, &offer, &[c], &[col], CORE_ID);
    let res = env.send(&[ix], &taker, &[]).res;
    assert_trade_err(&res, TradeError::NotCoreAsset);
}

// ---------------------------------------------------------------- revoke on cancel and close

#[test]
fn r01_cancel_revokes_every_give_card() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, _taker, _stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), env.taker.pubkey());
    let give: Vec<Pubkey> = (1..=3).map(|n| env.mint(&m, n, 1)).collect();
    let b = env.mint(&t, 9, 1);
    let offer = env.list(&maker, 1, &give, &[Slot::Asset(b)], week(&env));
    assert!(give.iter().all(|a| live(&env, a)));

    let rent = env.offer_rent(&offer);
    let before = env.snap(&wallets(&env), &give);
    let res = env.cancel(&maker, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
    for a in &give {
        assert_eq!(env.owner_of(a), m);
        assert_revoked(&env, a);
        assert!(!env.exists(&env.listed_pda(a)));
    }
    assert!(!env.exists(&offer));
    assert_eq!(env.lamports(&m), before.lamports[0].1 + rent - fee_of(&res));
    for (w, lamports) in &before.lamports[1..] {
        assert_eq!(env.lamports(w), *lamports);
    }
}

#[test]
fn r02_expired_close_by_stranger_revokes() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, _taker, stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), env.taker.pubkey());
    let give: Vec<Pubkey> = (1..=2).map(|n| env.mint(&m, n, 1)).collect();
    let b = env.mint(&t, 9, 1);
    let exp = env.now() + DAY;
    let offer = env.list(&maker, 1, &give, &[Slot::Asset(b)], exp);
    env.warp_to(exp);

    let rent = env.offer_rent(&offer);
    let before = env.snap(&wallets(&env), &give);
    let res = env.close(&stranger, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
    for a in &give {
        assert_eq!(env.owner_of(a), m);
        assert_revoked(&env, a);
    }
    assert_eq!(env.lamports(&m), before.lamports[0].1 + rent);
    assert_eq!(
        env.lamports(&stranger.pubkey()),
        before.lamports[2].1 - fee_of(&res),
        "the closer gains nothing from the revokes"
    );
}

#[test]
fn r03_stale_close_keeps_the_new_owners_delegation() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, taker, stranger) = keys(&env);
    let (m, t, s) = (maker.pubkey(), taker.pubkey(), stranger.pubkey());
    let a = env.mint(&m, 1, 1);
    let c = env.mint(&m, 2, 1);
    let b = env.mint(&t, 9, 1);
    let exp = week(&env);
    let offer = env.list(&maker, 1, &[a, c], &[Slot::Asset(b)], exp);

    // `a` leaves the maker; its new owner delegates it for a listing of their own.
    env.core_transfer(a, &maker, &s);
    env.add_delegate(a, &stranger, &env.offer_pda(&s, 1));
    assert!(live(&env, &a));

    let res = env.close(&taker, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
    assert!(
        live(&env, &a),
        "close revoked a card the maker no longer owns"
    );
    assert_eq!(env.owner_of(&a), s);
    assert_revoked(&env, &c);
    assert_eq!(env.owner_of(&c), m);

    let res = env
        .create_offer(&stranger, 1, &[a], &[Slot::Asset(b)], exp)
        .res;
    assert!(res.is_ok(), "{res:#?}");
}

#[test]
fn r04_card_state_never_blocks_cancel_or_close() {
    let mut env = Env::new(RuleSet::None);
    let (admin, maker, _taker, stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), env.taker.pubkey());
    let b = env.mint(&t, 9, 1);

    // Frozen by an owner-managed FreezeDelegate after listing.
    let frozen = env.mint(&m, 1, 1);
    // Permanently frozen from mint.
    let perma = Keypair::new();
    let ix = CreateV1Builder::new()
        .asset(perma.pubkey())
        .collection(Some(env.collection))
        .authority(Some(admin.pubkey()))
        .payer(admin.pubkey())
        .owner(Some(m))
        .data_state(DataState::AccountState)
        .name("Mudwater Card".into())
        .uri(uri(2, 1))
        .plugins(vec![PluginAuthorityPair {
            plugin: Plugin::PermanentFreezeDelegate(PermanentFreezeDelegate { frozen: true }),
            authority: None,
        }])
        .instruction();
    let res = env.send(&[ix], &admin, &[&perma]).res;
    assert!(res.is_ok(), "{res:#?}");
    let perma = perma.pubkey();
    // Burned by the maker after listing.
    let burned = env.mint(&m, 3, 1);

    let give = [frozen, perma, burned];
    let offer = env.list(&maker, 1, &give, &[Slot::Asset(b)], week(&env));
    env.freeze(frozen, &maker);
    let ix = BurnV1Builder::new()
        .asset(burned)
        .collection(Some(env.collection))
        .payer(m)
        .authority(Some(m))
        .instruction();
    let res = env.send(&[ix], &maker, &[]).res;
    assert!(res.is_ok(), "burn: {res:#?}");

    let rent = env.offer_rent(&offer);
    let before = env.lamports(&m);
    let res = env.cancel(&maker, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
    assert_revoked(&env, &frozen);
    assert_revoked(&env, &perma);
    assert!(give.iter().all(|a| !env.exists(&env.listed_pda(a))));
    assert_eq!(env.lamports(&m), before + rent - fee_of(&res));

    // The same for a stranger's close after expiry.
    let frozen2 = env.mint(&m, 4, 1);
    let exp = env.now() + DAY;
    let offer = env.list(&maker, 2, &[frozen2], &[Slot::Asset(b)], exp);
    env.freeze(frozen2, &maker);
    env.warp_to(exp);
    let res = env.close(&stranger, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
    assert_revoked(&env, &frozen2);
}

#[test]
fn r05_pause_never_blocks_cancel_or_close() {
    let mut env = Env::new(RuleSet::None);
    let (admin, maker, _taker, stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, 1, 1);
    let c = env.mint(&m, 2, 1);
    let b = env.mint(&t, 9, 1);
    let exp = env.now() + DAY;
    let o1 = env.list(&maker, 1, &[a], &[Slot::Asset(b)], week(&env));
    let o2 = env.list(&maker, 2, &[c], &[Slot::Asset(b)], exp);
    assert!(env.set_paused(&admin, true).res.is_ok());

    let res = env.cancel(&maker, &o1).res;
    assert!(res.is_ok(), "{res:#?}");
    assert_revoked(&env, &a);
    env.warp_to(exp);
    let res = env.close(&stranger, &o2).res;
    assert!(res.is_ok(), "{res:#?}");
    assert_revoked(&env, &c);
}

#[test]
fn r06_cancel_rejects_substitutes_and_rolls_back_revokes() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, _taker, _stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, 1, 1);
    let c = env.mint(&m, 2, 1);
    let e = env.mint(&m, 3, 1);
    let b = env.mint(&t, 9, 1);
    let exp = week(&env);
    let offer = env.list(&maker, 1, &[a, c], &[Slot::Asset(b)], exp);
    let other = env.list(&maker, 2, &[e], &[Slot::Asset(b)], exp);
    let (la, lc, le) = (env.listed_pda(&a), env.listed_pda(&c), env.listed_pda(&e));

    type Case = (&'static str, Vec<Pubkey>, Vec<Pubkey>, TradeError);
    let cases: Vec<Case> = vec![
        (
            "one Listed short",
            vec![a, c],
            vec![la],
            TradeError::AccountCount,
        ),
        (
            "decoy give asset",
            vec![a, e],
            vec![la, lc],
            TradeError::WrongAccount,
        ),
        (
            "give order swapped",
            vec![c, a],
            vec![la, lc],
            TradeError::WrongAccount,
        ),
        // These fail after both revokes ran, so they also prove the revokes roll back.
        (
            "Listed order swapped",
            vec![a, c],
            vec![lc, la],
            TradeError::BadListed,
        ),
        (
            "another offer's Listed",
            vec![a, c],
            vec![la, le],
            TradeError::BadListed,
        ),
    ];
    for (name, give, listed, err) in cases {
        let ix = env.cancel_ix_with(&m, &offer, &give, &listed);
        let before = env.snap(&wallets(&env), &[a, c, e]);
        let res = env.send(&[ix], &maker, &[]).res;
        assert_trade_err(&res, err);
        env.assert_nothing_moved(&before, &m, &res);
        assert!(
            live(&env, &a) && live(&env, &c),
            "{name}: a revoke survived"
        );
        assert!(env.exists(&offer) && env.exists(&la) && env.exists(&lc));
        println!("r06 {name}: {err:?}");
    }

    let wrong_col = env.create_collection(RuleSet::None);
    let mut ix = env.cancel_ix(&m, &offer);
    ix.accounts[4].pubkey = wrong_col;
    assert_trade_err(
        &env.send(&[ix], &maker, &[]).res,
        TradeError::WrongCollection,
    );
    let mut ix = env.cancel_ix(&m, &offer);
    ix.accounts[5].pubkey = system_program::ID;
    assert_trade_err(&env.send(&[ix], &maker, &[]).res, TradeError::WrongProgram);

    let res = env.cancel(&maker, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
    assert!(
        live(&env, &e) && env.exists(&other),
        "the other offer was touched"
    );
}

#[test]
fn r07_close_rejects_substitutes_and_rolls_back_revokes() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, _taker, stranger) = keys(&env);
    let (m, t, s) = (maker.pubkey(), env.taker.pubkey(), stranger.pubkey());
    let a = env.mint(&m, 1, 1);
    let c = env.mint(&m, 2, 1);
    let b = env.mint(&t, 9, 1);
    let exp = env.now() + DAY;
    let offer = env.list(&maker, 1, &[a, c], &[Slot::Asset(b)], exp);
    env.warp_to(exp);
    let (la, lc) = (env.listed_pda(&a), env.listed_pda(&c));

    type Case = (&'static str, Pubkey, Vec<Pubkey>, Vec<Pubkey>, TradeError);
    let cases: Vec<Case> = vec![
        (
            "maker substituted",
            s,
            vec![a, c],
            vec![la, lc],
            TradeError::WrongAccount,
        ),
        (
            "one Listed short",
            m,
            vec![a, c],
            vec![la],
            TradeError::AccountCount,
        ),
        (
            "give order swapped",
            m,
            vec![c, a],
            vec![la, lc],
            TradeError::WrongAccount,
        ),
        (
            "Listed order swapped",
            m,
            vec![a, c],
            vec![lc, la],
            TradeError::BadListed,
        ),
    ];
    for (name, maker_key, give, listed, err) in cases {
        let ix = env.close_ix_with(&s, &offer, maker_key, &give, &listed);
        let before = env.snap(&wallets(&env), &[a, c]);
        let res = env.send(&[ix], &stranger, &[]).res;
        assert_trade_err(&res, err);
        env.assert_nothing_moved(&before, &s, &res);
        assert!(
            live(&env, &a) && live(&env, &c),
            "{name}: a revoke survived"
        );
        println!("r07 {name}: {err:?}");
    }
    let mut ix = env.close_ix(&s, &offer);
    ix.accounts[6].pubkey = system_program::ID;
    assert_trade_err(
        &env.send(&[ix], &stranger, &[]).res,
        TradeError::WrongProgram,
    );

    let res = env.close(&stranger, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
}

// ---------------------------------------------------------------- fill rollback and order

#[test]
fn m05_late_core_failure_rolls_back_earlier_transfers() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, taker, _stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), taker.pubkey());
    let give = [env.mint(&m, 1, 1), env.mint(&m, 2, 1)];
    let got = [env.mint(&t, 9, 1), env.mint(&t, 10, 1)];
    let offer = env.list(
        &maker,
        1,
        &give,
        &[Slot::Asset(got[0]), Slot::Asset(got[1])],
        week(&env),
    );
    // Passes every check in fill_offer; only Core's transfer of the last card refuses it.
    env.freeze(got[1], &taker);

    let all = [give[0], give[1], got[0], got[1]];
    let before = env.snap(&wallets(&env), &all);
    let res = env.fill(&taker, &offer, &got).res;
    assert!(res.is_err(), "a frozen card was transferred");
    assert!(
        custom_error(&res).is_some_and(|(_, c)| c < 6000),
        "expected Core's error, got {:?}",
        custom_error(&res)
    );
    let logs = &res.as_ref().unwrap_err().meta.logs;
    let core_ok = logs
        .iter()
        .filter(|l| *l == &format!("Program {CORE_ID} success"))
        .count();
    assert_eq!(
        core_ok, 3,
        "both give transfers and the first taker transfer ran"
    );
    env.assert_nothing_moved(&before, &t, &res);
    assert!(env.exists(&offer));
    assert!(give
        .iter()
        .all(|a| env.exists(&env.listed_pda(a)) && live(&env, a)));
}

#[test]
fn l04_fill_listed_order_and_count() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, taker, _stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), taker.pubkey());
    let a = env.mint(&m, 1, 1);
    let c = env.mint(&m, 2, 1);
    let e = env.mint(&m, 3, 1);
    let b = env.mint(&t, 9, 1);
    let exp = week(&env);
    let offer = env.list(&maker, 1, &[a, c], &[Slot::Asset(b)], exp);
    env.list(&maker, 2, &[e], &[Slot::Asset(b)], exp);
    // FillOffer has 11 named accounts; then 2 give, 1 taker, 2 Listed.
    let (la, lc, le) = (env.listed_pda(&a), env.listed_pda(&c), env.listed_pda(&e));

    type Case = (&'static str, Vec<Pubkey>, TradeError);
    let cases: Vec<Case> = vec![
        ("Listed order swapped", vec![lc, la], TradeError::BadListed),
        (
            "another offer's Listed",
            vec![la, le],
            TradeError::BadListed,
        ),
        ("one Listed short", vec![la], TradeError::AccountCount),
    ];
    for (name, listed, err) in cases {
        let mut ix = env.fill_ix_with(&t, &offer, &[a, c], &[b], CORE_ID);
        ix.accounts.truncate(14);
        ix.accounts.extend(
            listed
                .iter()
                .map(|k| anchor_lang::solana_program::instruction::AccountMeta::new(*k, false)),
        );
        let before = env.snap(&wallets(&env), &[a, c, b]);
        let res = env.send(&[ix], &taker, &[]).res;
        assert_trade_err(&res, err);
        env.assert_nothing_moved(&before, &t, &res);
        println!("l04 fill {name}: {err:?}");
    }
    let res = env.fill(&taker, &offer, &[b]).res;
    assert!(res.is_ok(), "{res:#?}");
}

#[test]
fn l04_create_listed_order_and_count() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, _taker, _stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, 1, 1);
    let c = env.mint(&m, 2, 1);
    let b = env.mint(&t, 9, 1);
    env.add_delegate(a, &maker, &env.offer_pda(&m, 1));
    env.add_delegate(c, &maker, &env.offer_pda(&m, 1));
    let exp = week(&env);
    let (la, lc) = (env.listed_pda(&a), env.listed_pda(&c));
    // CreateOffer has 7 named accounts; then 2 give, 2 Listed.
    let cases: Vec<(&str, Vec<Pubkey>, TradeError)> = vec![
        (
            "Listed order swapped",
            vec![c, a, lc, la],
            TradeError::WrongAccount,
        ),
        ("one Listed short", vec![a, c, la], TradeError::AccountCount),
    ];
    for (name, rest, err) in cases {
        let mut ix = env.create_offer_ix(&m, 1, &[a, c], &[Slot::Asset(b)], exp);
        ix.accounts.truncate(7);
        ix.accounts.extend(
            rest.iter()
                .map(|k| anchor_lang::solana_program::instruction::AccountMeta::new(*k, false)),
        );
        let res = env.send(&[ix], &maker, &[]).res;
        assert_trade_err(&res, err);
        assert!(!env.exists(&env.offer_pda(&m, 1)) && !env.exists(&la) && !env.exists(&lc));
        println!("l04 create {name}: {err:?}");
    }
}

#[test]
fn l04_prefunded_listed_is_claimed_and_refunded() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, _taker, _stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), env.taker.pubkey());
    let listed_rent = env
        .svm
        .minimum_balance_for_rent_exemption(8 + Listed::INIT_SPACE);
    let empty_rent = env.svm.minimum_balance_for_rent_exemption(0);
    let cases = [
        ("1 lamport", 1),
        ("0-byte rent minimum", empty_rent),
        ("1 below Listed rent", listed_rent - 1),
        ("exactly Listed rent", listed_rent),
        ("above Listed rent", listed_rent + 5_000),
    ];
    for (i, (name, prefund)) in cases.into_iter().enumerate() {
        let a = env.mint(&m, i as u32 + 1, 1);
        let b = env.mint(&t, 9, i as u32 + 1);
        let id = i as u64 + 1;
        env.add_delegate(a, &maker, &env.offer_pda(&m, id));
        let l = env.listed_pda(&a);
        env.svm
            .set_account(
                l,
                solana_account::Account {
                    lamports: prefund,
                    data: vec![],
                    owner: system_program::ID,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
        let before = env.lamports(&m);
        let res = env
            .create_offer(&maker, id, &[a], &[Slot::Asset(b)], week(&env))
            .res;
        assert!(res.is_ok(), "{name}: {res:#?}");
        let offer = env.offer_pda(&m, id);
        let held = prefund.max(listed_rent);
        assert_eq!(env.lamports(&l), held, "{name}");
        let state = Listed::try_deserialize_listed(&env.svm.get_account(&l).unwrap().data);
        assert_eq!(state, offer, "{name}: Listed points elsewhere");
        let paid = before - env.lamports(&m);
        assert_eq!(
            paid,
            fee_of(&res)
                + env.lamports(&offer)
                + listed_rent.saturating_sub(prefund)
                + if i == 0 { env.maker_state_rent() } else { 0 },
            "{name}: the maker paid more than the top-up"
        );

        let before = env.lamports(&m);
        let res = env.cancel(&maker, &offer).res;
        assert!(res.is_ok(), "{name}: {res:#?}");
        let offer_rent = env
            .svm
            .minimum_balance_for_rent_exemption(8 + mud_trade_book::state::Offer::INIT_SPACE);
        assert_eq!(
            env.lamports(&m),
            before + offer_rent + held - fee_of(&res),
            "{name}: refund"
        );
        println!(
            "l04 prefunded {name} ({prefund}): maker topped up {}",
            listed_rent.saturating_sub(prefund)
        );
    }
}

#[test]
fn l03_offer_id_is_never_reused() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, _taker, _stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, 1, 1);
    let b = env.mint(&t, 9, 1);
    assert_eq!(env.last_offer_id(&m), 0);

    // The first id is 1: 0 and 2 are refused and nothing is created.
    for id in [0, 2] {
        env.add_delegate(a, &maker, &env.offer_pda(&m, id));
        let res = env
            .create_offer(&maker, id, &[a], &[Slot::Asset(b)], week(&env))
            .res;
        assert_trade_err(&res, TradeError::OfferIdNotNext);
        assert!(!env.exists(&env.offer_pda(&m, id)));
        env.revoke_delegate(a, &maker);
    }
    assert!(!env.exists(&env.maker_state_pda(&m)));

    let offer = env.list(&maker, 1, &[a], &[Slot::Asset(b)], week(&env));
    assert_eq!(env.last_offer_id(&m), 1);
    assert!(env.cancel(&maker, &offer).res.is_ok());

    // Cancelled, the address is free, but id 1 is never created again: not in the same slot,
    // not later.
    env.add_delegate(a, &maker, &offer);
    for _ in 0..2 {
        let res = env
            .create_offer(&maker, 1, &[a], &[Slot::Asset(b)], week(&env))
            .res;
        assert_trade_err(&res, TradeError::OfferIdNotNext);
        assert!(!env.exists(&offer));
        env.warp_to(env.now() + 1);
    }
    env.revoke_delegate(a, &maker);

    let next = env.list(&maker, 2, &[a], &[Slot::Asset(b)], week(&env));
    assert_ne!(next, offer, "a new id is a new address and a new delegate");
    assert_eq!(env.last_offer_id(&m), 2);

    // Each maker counts on their own.
    let stranger = env.stranger.insecure_clone();
    let s = stranger.pubkey();
    let c = env.mint(&s, 2, 1);
    env.list(&stranger, 1, &[c], &[Slot::Asset(b)], week(&env));
    assert_eq!((env.last_offer_id(&m), env.last_offer_id(&s)), (2, 1));
}

/// The audit's same-slot case inside one transaction: create, cancel, and create again with the
/// same id. The second create is refused, so the whole transaction rolls back.
#[test]
fn l03_same_transaction_recreate_is_refused() {
    let mut env = Env::new(RuleSet::None);
    let (_admin, maker, _taker, _stranger) = keys(&env);
    let (m, t) = (maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, 1, 1);
    let b = env.mint(&t, 9, 1);
    let exp = week(&env);
    let offer = env.offer_pda(&m, 1);
    let listed = env.listed_pda(&a);
    // A revoked plugin already on the card, so both approvals below are Approve, not Add.
    env.add_delegate(a, &maker, &offer);
    env.revoke_delegate(a, &maker);
    let ixs = vec![
        env.delegate_ix(a, &m, &offer),
        env.create_offer_ix(&m, 1, &[a], &[Slot::Asset(b)], exp),
        env.cancel_ix_with(&m, &offer, &[a], &[listed]),
        env.delegate_ix(a, &m, &offer),
        env.create_offer_ix(&m, 1, &[a], &[Slot::Asset(b)], exp),
    ];
    let res = env.send(&ixs, &maker, &[]).res;
    assert_trade_err(&res, TradeError::OfferIdNotNext);
    assert!(!env.exists(&offer));
    assert_eq!(env.last_offer_id(&m), 0);
}

trait ListedBytes {
    fn try_deserialize_listed(data: &[u8]) -> Pubkey;
}

impl ListedBytes for Listed {
    fn try_deserialize_listed(data: &[u8]) -> Pubkey {
        use anchor_lang::AccountDeserialize;
        Listed::try_deserialize(&mut &data[..])
            .expect("Listed decodes")
            .offer
    }
}

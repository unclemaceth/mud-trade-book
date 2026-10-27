//! The 23 core cases, each its own test, against the real Metaplex Core binary.
//! Every test asserts the final owner of every asset it involves. Failed transactions are
//! checked to move no lamports beyond the payer's fee; successful ones check the balances of
//! the wallets whose money the case is about, not every wallet. Governance, revoke, rollback
//! and substitution cases live in `remediation.rs`.

mod common;

use anchor_lang::prelude::Pubkey;
use common::*;
use mpl_core::types::RuleSet;
use mud_trade_book::{error::TradeError, state::Slot, FINISH_ALL, FINISH_BASE, FINISH_FOIL};
use solana_signer::Signer;

fn base(n: u32) -> u32 {
    n - 1
}

fn foil(n: u32) -> u32 {
    n + 119
}

fn env() -> Env {
    Env::new(RuleSet::None)
}

fn in_a_week(env: &Env) -> i64 {
    env.now() + 7 * DAY
}

/// A fill succeeded: give went to the taker, got went to the maker, the offer and every Listed
/// PDA are closed, the maker got exactly the rent back and the taker paid only the fee.
#[track_caller]
fn assert_filled(
    env: &Env,
    offer: &Pubkey,
    give: &[Pubkey],
    got: &[Pubkey],
    before: &Snap,
    rent: u64,
    res: &litesvm::types::TransactionResult,
) {
    assert!(res.is_ok(), "fill failed: {res:#?}");
    let (maker, taker) = (env.maker.pubkey(), env.taker.pubkey());
    for a in give {
        assert_eq!(env.owner_of(a), taker, "give card {a} not with taker");
        assert!(
            !env.read(a).delegate_is(&env.delegate),
            "delegate survived the transfer"
        );
        assert!(!env.exists(&env.listed_pda(a)), "Listed PDA not closed");
    }
    for a in got {
        assert_eq!(env.owner_of(a), maker, "taker card {a} not with maker");
    }
    assert!(!env.exists(offer), "offer not closed");
    let lam = |k: &Pubkey| before.lamports.iter().find(|(w, _)| w == k).unwrap().1;
    assert_eq!(
        env.lamports(&maker),
        lam(&maker) + rent,
        "maker rent refund"
    );
    assert_eq!(
        env.lamports(&taker),
        lam(&taker) - fee_of(res),
        "taker paid more than the fee"
    );
    assert_eq!(
        env.lamports(&env.stranger.pubkey()),
        lam(&env.stranger.pubkey())
    );
}

fn wallets(env: &Env) -> Vec<Pubkey> {
    vec![
        env.maker.pubkey(),
        env.taker.pubkey(),
        env.stranger.pubkey(),
    ]
}

// ---------------------------------------------------------------- happy paths

#[test]
fn t01_exact_for_exact() {
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(32), 303);
    let b = env.mint(&t, base(10), 7);

    let before_list = env.snap(&wallets(&env), &[a, b]);
    let asset_lamports = env.lamports(&a);
    let exp = in_a_week(&env);
    let maker = env.maker.insecure_clone();
    let mut ixs = vec![env.delegate_ix(a, &m)];
    ixs.push(env.create_offer_ix(&m, 1, &[a], &[Slot::Asset(b)], exp));
    let listed = env.send(&ixs, &maker, &[]).res;
    assert!(listed.is_ok(), "{listed:#?}");
    let offer = env.offer_pda(&m, 1);
    let rent = env.offer_rent(&offer);
    let plugin_rent = env.lamports(&a) - asset_lamports;
    assert_eq!(
        env.lamports(&m),
        before_list.lamports[0].1 - fee_of(&listed) - rent - plugin_rent,
        "listing cost = fee + offer/listed rent + plugin rent"
    );
    assert!(env.read(&a).delegate_is(&env.delegate));
    assert_eq!(env.owner_of(&a), m, "listing must not move the card");

    let before = env.snap(&wallets(&env), &[a, b]);
    let taker = env.taker.insecure_clone();
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_filled(&env, &offer, &[a], &[b], &before, rent, &res);

    // The card keeps its TransferDelegate plugin (authority back to the owner), so the new
    // owner re-lists it with Approve, and its Listed PDA can be created again.
    assert!(env.read(&a).transfer_delegate.is_some());
    let exp = in_a_week(&env);
    let again = env.list(&taker, 1, &[a], &[cards_slot(&[1], FINISH_ALL)], exp);
    assert!(env.exists(&again) && env.exists(&env.listed_pda(&a)));
    assert_eq!(env.owner_of(&a), t);
}

#[test]
fn t02_one_of_a_set() {
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(32), 1);
    let b = env.mint(&t, base(14), 2);
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(
        &maker,
        1,
        &[a],
        &[cards_slot(&[1, 14, 77], FINISH_ALL)],
        exp,
    );
    let rent = env.offer_rent(&offer);
    let before = env.snap(&wallets(&env), &[a, b]);
    let taker = env.taker.insecure_clone();
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_filled(&env, &offer, &[a], &[b], &before, rent, &res);
}

#[test]
fn t03_any_of_a_rarity() {
    // The Mutant band (#063..=#084), Base or Foil; filled with a Foil #070.
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(101), 1);
    let b = env.mint(&t, foil(70), 9);
    let mutants: Vec<u8> = (63..=84).collect();
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(&maker, 1, &[a], &[cards_slot(&mutants, FINISH_ALL)], exp);
    let rent = env.offer_rent(&offer);
    let before = env.snap(&wallets(&env), &[a, b]);
    let taker = env.taker.insecure_clone();
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_filled(&env, &offer, &[a], &[b], &before, rent, &res);
}

#[test]
fn t04_n_of_a_set() {
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(120), 1);
    let set = cards_slot(&[1, 2, 3, 4, 5], FINISH_BASE);
    let b: Vec<Pubkey> = [1, 3, 5]
        .iter()
        .map(|n| env.mint(&t, base(*n), 40 + n))
        .collect();
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(&maker, 1, &[a], &[set, set, set], exp);
    let rent = env.offer_rent(&offer);
    let mut all = vec![a];
    all.extend(&b);
    let before = env.snap(&wallets(&env), &all);
    let taker = env.taker.insecure_clone();
    let res = env.fill(&taker, &offer, &b).res;
    assert_filled(&env, &offer, &[a], &b, &before, rent, &res);
}

#[test]
fn t05_four_give_for_one_want() {
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let give: Vec<Pubkey> = [2, 3, 4, 5]
        .iter()
        .map(|n| env.mint(&m, base(*n), *n))
        .collect();
    let b = env.mint(&t, foil(100), 1);
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(&maker, 1, &give, &[Slot::Asset(b)], exp);
    let rent = env.offer_rent(&offer);
    let mut all = give.clone();
    all.push(b);
    let before = env.snap(&wallets(&env), &all);
    let taker = env.taker.insecure_clone();
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_filled(&env, &offer, &give, &[b], &before, rent, &res);
}

#[test]
fn t06_finish_filter_foil_only() {
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(50), 1);
    let base_copy = env.mint(&t, base(14), 1);
    let foil_copy = env.mint(&t, foil(14), 1);
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(&maker, 1, &[a], &[cards_slot(&[14], FINISH_FOIL)], exp);
    let rent = env.offer_rent(&offer);
    let taker = env.taker.insecure_clone();

    let before = env.snap(&wallets(&env), &[a, base_copy, foil_copy]);
    let res = env.fill(&taker, &offer, &[base_copy]).res;
    assert_trade_err(&res, TradeError::SlotMismatch);
    env.assert_nothing_moved(&before, &taker.pubkey(), &res);

    let before = env.snap(&wallets(&env), &[a, base_copy, foil_copy]);
    let res = env.fill(&taker, &offer, &[foil_copy]).res;
    assert_filled(&env, &offer, &[a], &[foil_copy], &before, rent, &res);
    assert_eq!(
        env.owner_of(&base_copy),
        t,
        "the Base copy stays with the taker"
    );
}

// ---------------------------------------------------------------- must fail

/// Lists `a` (maker) for exact `b` (taker) and returns the offer.
fn simple(env: &mut Env, exp: i64) -> (Pubkey, Pubkey, Pubkey) {
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(32), 1);
    let b = env.mint(&t, base(10), 2);
    let maker = env.maker.insecure_clone();
    let offer = env.list(&maker, 1, &[a], &[Slot::Asset(b)], exp);
    (offer, a, b)
}

#[test]
fn t07_expired_offer() {
    let mut env = env();
    let exp = env.now() + DAY;
    let (offer, a, b) = simple(&mut env, exp);
    let taker = env.taker.insecure_clone();

    env.warp_to(exp); // now == expires_at
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_trade_err(&res, TradeError::Expired);
    env.assert_nothing_moved(&before, &taker.pubkey(), &res);
    assert!(env.exists(&offer));

    // One second earlier the same shape fills.
    let mut env = Env::new(RuleSet::None);
    let exp = env.now() + DAY;
    let (offer, a, b) = simple(&mut env, exp);
    env.warp_to(exp - 1);
    let rent = env.offer_rent(&offer);
    let before = env.snap(&wallets(&env), &[a, b]);
    let taker = env.taker.insecure_clone();
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_filled(&env, &offer, &[a], &[b], &before, rent, &res);
}

#[test]
fn t08_paused_create_and_fill() {
    let mut env = env();
    let exp = in_a_week(&env);
    let (offer, a, b) = simple(&mut env, exp);
    let admin = env.admin.insecure_clone();
    let stranger = env.stranger.insecure_clone();
    assert_trade_err(&env.set_paused(&stranger, true).res, TradeError::NotAdmin);
    assert!(env.set_paused(&admin, true).res.is_ok());

    let m = env.maker.pubkey();
    let c = env.mint(&m, base(33), 1);
    let maker = env.maker.insecure_clone();
    env.add_delegate(c, &maker);
    let before = env.snap(&wallets(&env), &[a, b, c]);
    let res = env
        .create_offer(&maker, 2, &[c], &[Slot::Asset(b)], exp)
        .res;
    assert_trade_err(&res, TradeError::Paused);
    env.assert_nothing_moved(&before, &m, &res);
    assert!(!env.exists(&env.offer_pda(&m, 2)));

    let taker = env.taker.insecure_clone();
    let before = env.snap(&wallets(&env), &[a, b, c]);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_trade_err(&res, TradeError::Paused);
    env.assert_nothing_moved(&before, &taker.pubkey(), &res);

    // Unpaused, the same fill goes through.
    assert!(env.set_paused(&admin, false).res.is_ok());
    let rent = env.offer_rent(&offer);
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_filled(&env, &offer, &[a], &[b], &before, rent, &res);
}

#[test]
fn t09_maker_moved_give_card() {
    let mut env = env();
    let exp = in_a_week(&env);
    let (offer, a, b) = simple(&mut env, exp);
    let maker = env.maker.insecure_clone();
    let s = env.stranger.pubkey();
    env.core_transfer(a, &maker, &s);
    assert_eq!(env.owner_of(&a), s);

    let taker = env.taker.insecure_clone();
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_trade_err(&res, TradeError::NotOwner);
    env.assert_nothing_moved(&before, &taker.pubkey(), &res);

    // The new owner cannot list the card while the stale Listed PDA exists.
    let stranger = env.stranger.insecure_clone();
    env.add_delegate(a, &stranger);
    let res = env
        .create_offer(&stranger, 1, &[a], &[Slot::Asset(b)], exp)
        .res;
    assert_trade_err(&res, TradeError::AlreadyListed);

    // Anyone closes the stale offer; rent goes to the maker, never the caller.
    let rent = env.offer_rent(&offer);
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.close(&stranger, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
    assert!(!env.exists(&offer) && !env.exists(&env.listed_pda(&a)));
    assert_eq!(env.lamports(&maker.pubkey()), before.lamports[0].1 + rent);
    assert_eq!(env.lamports(&s), before.lamports[2].1 - fee_of(&res));
    assert_eq!(env.owner_of(&a), s);
    assert_eq!(env.owner_of(&b), env.taker.pubkey());

    // Now the new owner can list it.
    let res = env
        .create_offer(&stranger, 1, &[a], &[Slot::Asset(b)], exp)
        .res;
    assert!(res.is_ok(), "{res:#?}");
}

#[test]
fn t10_maker_revoked_delegate() {
    let mut env = env();
    let exp = in_a_week(&env);
    let (offer, a, b) = simple(&mut env, exp);
    let maker = env.maker.insecure_clone();
    env.revoke_delegate(a, &maker);
    assert!(!env.read(&a).delegate_is(&env.delegate));

    let taker = env.taker.insecure_clone();
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_trade_err(&res, TradeError::DelegateMissing);
    env.assert_nothing_moved(&before, &taker.pubkey(), &res);

    let stranger = env.stranger.insecure_clone();
    let rent = env.offer_rent(&offer);
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.close(&stranger, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
    assert!(!env.exists(&offer));
    assert_eq!(env.lamports(&maker.pubkey()), before.lamports[0].1 + rent);
    assert_eq!(
        env.lamports(&stranger.pubkey()),
        before.lamports[2].1 - fee_of(&res)
    );
    assert_eq!(env.owner_of(&a), maker.pubkey());
    assert_eq!(env.owner_of(&b), taker.pubkey());
}

#[test]
fn t11_taker_asset_from_another_collection() {
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(32), 1);
    let other = env.create_collection(RuleSet::None);
    let foreign = env.mint_in(other, &t, &uri(base(10), 1));
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(&maker, 1, &[a], &[cards_slot(&[10], FINISH_ALL)], exp);

    let taker = env.taker.insecure_clone();
    let before = env.snap(&wallets(&env), &[a, foreign]);
    let res = env.fill(&taker, &offer, &[foreign]).res;
    assert_trade_err(&res, TradeError::WrongCollection);
    env.assert_nothing_moved(&before, &t, &res);

    // Passing the other collection as the collection account is refused too.
    let mut ix = env.fill_ix_with(&t, &offer, &[a], &[foreign], CORE_ID);
    ix.accounts[6].pubkey = other;
    let res = env.send(&[ix], &taker, &[]).res;
    assert_trade_err(&res, TradeError::WrongCollection);

    // A maker cannot list a card from another collection.
    let foreign_give = env.mint_in(other, &m, &uri(base(5), 1));
    let mut ixs = vec![mpl_core::instructions::AddPluginV1Builder::new()
        .asset(foreign_give)
        .collection(Some(other))
        .payer(m)
        .authority(Some(m))
        .plugin(mpl_core::types::Plugin::TransferDelegate(
            mpl_core::types::TransferDelegate {},
        ))
        .init_authority(mpl_core::types::PluginAuthority::Address {
            address: env.delegate,
        })
        .instruction()];
    ixs.push(env.create_offer_ix(&m, 2, &[foreign_give], &[Slot::Asset(a)], exp));
    let res = env.send(&ixs, &maker, &[]).res;
    assert_trade_err(&res, TradeError::WrongCollection);
    assert_eq!(env.owner_of(&foreign_give), m);
}

#[test]
fn t12_same_taker_asset_twice() {
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(32), 1);
    let b = env.mint(&t, base(10), 1);
    let set = cards_slot(&[10], FINISH_ALL);
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(&maker, 1, &[a], &[set, set], exp);
    let taker = env.taker.insecure_clone();
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b, b]).res;
    assert_trade_err(&res, TradeError::DuplicateAsset);
    env.assert_nothing_moved(&before, &t, &res);
}

#[test]
fn t13_taker_passes_makers_give_asset() {
    let mut env = env();
    let m = env.maker.pubkey();
    let a = env.mint(&m, base(32), 1);
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(&maker, 1, &[a], &[cards_slot(&[32], FINISH_ALL)], exp);
    let taker = env.taker.insecure_clone();
    let before = env.snap(&wallets(&env), &[a]);
    let res = env.fill(&taker, &offer, &[a]).res;
    assert_trade_err(&res, TradeError::DuplicateAsset);
    env.assert_nothing_moved(&before, &taker.pubkey(), &res);
}

#[test]
fn t14_wrong_card_for_slot() {
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(32), 1);
    let wrong = env.mint(&t, base(11), 1);
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(&maker, 1, &[a], &[cards_slot(&[10], FINISH_ALL)], exp);
    let taker = env.taker.insecure_clone();
    let before = env.snap(&wallets(&env), &[a, wrong]);
    let res = env.fill(&taker, &offer, &[wrong]).res;
    assert_trade_err(&res, TradeError::SlotMismatch);
    env.assert_nothing_moved(&before, &t, &res);

    // An exact slot refuses any other asset, even the same card number.
    let b = env.mint(&t, base(10), 1);
    let twin = env.mint(&t, base(10), 2);
    let a2 = env.mint(&m, base(33), 1);
    let offer2 = env.list(&maker, 2, &[a2], &[Slot::Asset(b)], exp);
    let before = env.snap(&wallets(&env), &[a2, b, twin]);
    let res = env.fill(&taker, &offer2, &[twin]).res;
    assert_trade_err(&res, TradeError::SlotMismatch);
    env.assert_nothing_moved(&before, &t, &res);
}

#[test]
fn t15_uri_not_in_scheme() {
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(32), 1);
    let every_card: Vec<u8> = (1..=120).collect();
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(&maker, 1, &[a], &[cards_slot(&every_card, FINISH_ALL)], exp);
    let taker = env.taker.insecure_clone();
    let bad = [
        "https://assets.mudwater.bayc.co/m/c/31-303", // wrong host
        "https://evil.example/m/c/31-303",            // wrong host
        "http://assets.mudwater.bayc.com/m/c/31-303", // wrong scheme
        "https://assets.mudwater.bayc.com/m/c/240-1", // idx out of range
        "https://assets.mudwater.bayc.com/m/c/01-1",  // leading zero
        "https://assets.mudwater.bayc.com/m/c/1-",    // empty serial
        "https://assets.mudwater.bayc.com/m/c/1-2x",  // trailing junk
        "https://assets.mudwater.bayc.com/m/c/-2",    // empty idx
        "https://assets.mudwater.bayc.com/m/c/1-4294967296", // serial over u32
        "https://assets.mudwater.bayc.com/m/c/31-303.json",
        "https://assets.mudwater.bayc.com/m/c/+31-303",
        "assets.mudwater.bayc.com/m/c/31-303", // missing prefix
        "/m/c/31-303",                         // missing prefix
        "31-303",                              // missing prefix
    ];
    let admin = env.admin.insecure_clone();
    for u in bad {
        let b = env.mint_in(env.collection, &t, u);
        // Registration reads the URI, so a card outside the scheme can never get a record,
        // even under a root that lists it.
        let (root_index, proofs) = env.publish_root(&[(b, 31, FINISH_BASE)]);
        let ix = env.register_card_ix(&admin.pubkey(), root_index, &b, 31, FINISH_BASE, proofs[0].clone());
        assert_trade_err(&env.send(&[ix], &admin, &[]).res, TradeError::UnrecognisedCard);
        // Without a record, a Cards slot refuses it.
        let before = env.snap(&wallets(&env), &[a, b]);
        let res = env.fill(&taker, &offer, &[b]).res;
        assert_trade_err(&res, TradeError::BadCardRecord);
        env.assert_nothing_moved(&before, &t, &res);
        println!("t15 refused {u:?}");
    }
}

#[test]
fn t16_maker_is_taker() {
    let mut env = env();
    let m = env.maker.pubkey();
    let a = env.mint(&m, base(32), 1);
    let own = env.mint(&m, base(10), 1);
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let offer = env.list(&maker, 1, &[a], &[Slot::Asset(own)], exp);
    let before = env.snap(&wallets(&env), &[a, own]);
    let res = env.fill(&maker, &offer, &[own]).res;
    assert_trade_err(&res, TradeError::SelfFill);
    env.assert_nothing_moved(&before, &m, &res);
}

#[test]
fn t17_same_card_in_two_offers() {
    let mut env = env();
    let exp = in_a_week(&env);
    let (_offer, a, b) = simple(&mut env, exp);
    let maker = env.maker.insecure_clone();
    let m = maker.pubkey();
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env
        .create_offer(&maker, 2, &[a], &[cards_slot(&[1], FINISH_ALL)], exp)
        .res;
    assert_trade_err(&res, TradeError::AlreadyListed);
    env.assert_nothing_moved(&before, &m, &res);
    assert!(!env.exists(&env.offer_pda(&m, 2)));

    // Twice in the same offer is refused before any PDA is touched.
    let c = env.mint(&m, base(40), 1);
    env.add_delegate(c, &maker);
    let res = env
        .create_offer(&maker, 3, &[c, c], &[Slot::Asset(b)], exp)
        .res;
    assert_trade_err(&res, TradeError::DuplicateAsset);
}

#[test]
fn t18_close_live_offer_by_stranger() {
    let mut env = env();
    let exp = in_a_week(&env);
    let (offer, a, b) = simple(&mut env, exp);
    let stranger = env.stranger.insecure_clone();
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.close(&stranger, &offer).res;
    assert_trade_err(&res, TradeError::NotExpiredOrStale);
    env.assert_nothing_moved(&before, &stranger.pubkey(), &res);
    assert!(env.exists(&offer));
}

#[test]
fn t19_cancel_by_non_maker() {
    let mut env = env();
    let exp = in_a_week(&env);
    let (offer, a, b) = simple(&mut env, exp);
    for k in [env.stranger.insecure_clone(), env.taker.insecure_clone()] {
        let before = env.snap(&wallets(&env), &[a, b]);
        let res = env.cancel(&k, &offer).res;
        assert_trade_err(&res, TradeError::NotMaker);
        env.assert_nothing_moved(&before, &k.pubkey(), &res);
        assert!(env.exists(&offer));
    }

    // The maker can, rent comes back, the card never moved and its delegation is gone.
    let maker = env.maker.insecure_clone();
    let rent = env.offer_rent(&offer);
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.cancel(&maker, &offer).res;
    assert!(res.is_ok(), "{res:#?}");
    assert!(!env.exists(&offer) && !env.exists(&env.listed_pda(&a)));
    assert!(matches!(
        env.read(&a).transfer_delegate,
        Some(mpl_core::types::PluginAuthority::Owner)
    ));
    assert_eq!(
        env.lamports(&maker.pubkey()),
        before.lamports[0].1 + rent - fee_of(&res)
    );
    assert_eq!(env.owner_of(&a), maker.pubkey());
}

#[test]
fn t20_create_without_ownership_or_delegate() {
    let mut env = env();
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let maker = env.maker.insecure_clone();
    let exp = in_a_week(&env);
    let want = [cards_slot(&[1], FINISH_ALL)];

    // Not the maker's card (the taker's, delegated by the taker to our PDA).
    let not_mine = env.mint(&t, base(10), 1);
    let taker = env.taker.insecure_clone();
    env.add_delegate(not_mine, &taker);
    let before = env.snap(&wallets(&env), &[not_mine]);
    let res = env.create_offer(&maker, 1, &[not_mine], &want, exp).res;
    assert_trade_err(&res, TradeError::NotOwner);
    env.assert_nothing_moved(&before, &m, &res);

    // Maker's card, no TransferDelegate plugin.
    let bare = env.mint(&m, base(11), 1);
    let before = env.snap(&wallets(&env), &[bare]);
    let res = env.create_offer(&maker, 1, &[bare], &want, exp).res;
    assert_trade_err(&res, TradeError::DelegateMissing);
    env.assert_nothing_moved(&before, &m, &res);

    // Delegate plugin pointing at someone else.
    let wrong = env.mint(&m, base(12), 1);
    let ix = mpl_core::instructions::AddPluginV1Builder::new()
        .asset(wrong)
        .collection(Some(env.collection))
        .payer(m)
        .authority(Some(m))
        .plugin(mpl_core::types::Plugin::TransferDelegate(
            mpl_core::types::TransferDelegate {},
        ))
        .init_authority(mpl_core::types::PluginAuthority::Address {
            address: env.stranger.pubkey(),
        })
        .instruction();
    assert!(env.send(&[ix], &maker, &[]).res.is_ok());
    let res = env.create_offer(&maker, 1, &[wrong], &want, exp).res;
    assert_trade_err(&res, TradeError::DelegateMissing);
    assert!(!env.exists(&env.offer_pda(&m, 1)));
}

#[test]
fn t21_counts_and_expiry() {
    let mut env = env();
    let m = env.maker.pubkey();
    let maker = env.maker.insecure_clone();
    let give: Vec<Pubkey> = (1..=5).map(|n| env.mint(&m, base(n), 1)).collect();
    for a in &give {
        env.add_delegate(*a, &maker);
    }
    let one = [cards_slot(&[1], FINISH_ALL)];
    let five: Vec<Slot> = (1..=5u8).map(|n| cards_slot(&[n], FINISH_ALL)).collect();
    let now = env.now();
    let exp = now + DAY;
    let before = env.snap(&wallets(&env), &give);

    type Case = (&'static str, Vec<Pubkey>, Vec<Slot>, i64, TradeError);
    let cases: Vec<Case> = vec![
        ("give 0", vec![], one.to_vec(), exp, TradeError::EmptyGive),
        (
            "give 5",
            give.clone(),
            one.to_vec(),
            exp,
            TradeError::TooMany,
        ),
        (
            "want 0",
            give[..1].to_vec(),
            vec![],
            exp,
            TradeError::BadWant,
        ),
        ("want 5", give[..1].to_vec(), five, exp, TradeError::TooMany),
        (
            "empty bitset",
            give[..1].to_vec(),
            vec![Slot::Cards {
                cards: [0; 15],
                finish_mask: 3,
            }],
            exp,
            TradeError::BadWant,
        ),
        (
            "zero finish",
            give[..1].to_vec(),
            vec![cards_slot(&[1], 0)],
            exp,
            TradeError::BadWant,
        ),
        (
            "reserved finish bit",
            give[..1].to_vec(),
            vec![cards_slot(&[1], 0b111)],
            exp,
            TradeError::BadWant,
        ),
        (
            "want own give",
            give[..1].to_vec(),
            vec![Slot::Asset(give[0])],
            exp,
            TradeError::BadWant,
        ),
        (
            "expiry now",
            give[..1].to_vec(),
            one.to_vec(),
            now,
            TradeError::BadExpiry,
        ),
        (
            "expiry past",
            give[..1].to_vec(),
            one.to_vec(),
            now - 1,
            TradeError::BadExpiry,
        ),
        (
            "expiry 30d+1s",
            give[..1].to_vec(),
            one.to_vec(),
            now + 30 * DAY + 1,
            TradeError::BadExpiry,
        ),
    ];
    for (i, (name, g, w, e, err)) in cases.into_iter().enumerate() {
        let id = 100 + i as u64;
        let res = env.create_offer(&maker, id, &g, &w, e).res;
        assert_trade_err(&res, err);
        assert!(
            !env.exists(&env.offer_pda(&m, id)),
            "{name} created an offer"
        );
        println!("t21 {name}: {err:?}");
    }
    for (a, owner) in &before.owners {
        assert_eq!(env.owner_of(a), *owner);
    }

    // Exactly 30 days is allowed; 4 give + 4 want is allowed.
    let four: Vec<Slot> = (1..=4u8).map(|n| cards_slot(&[n], FINISH_ALL)).collect();
    let res = env
        .create_offer(&maker, 1, &give[..4], &four, now + 30 * DAY)
        .res;
    assert!(res.is_ok(), "{res:#?}");
}

#[test]
fn t22_wrong_core_program() {
    let mut env = env();
    let exp = in_a_week(&env);
    let (offer, a, b) = simple(&mut env, exp);
    let t = env.taker.pubkey();
    let taker = env.taker.insecure_clone();
    for fake in [
        anchor_lang::solana_program::system_program::ID,
        env.program_id,
        Pubkey::new_unique(),
    ] {
        let ix = env.fill_ix_with(&t, &offer, &[a], &[b], fake);
        let before = env.snap(&wallets(&env), &[a, b]);
        let res = env.send(&[ix], &taker, &[]).res;
        assert_trade_err(&res, TradeError::WrongProgram);
        env.assert_nothing_moved(&before, &t, &res);
    }

    // Wrong maker account and wrong give account are refused too.
    let mut ix = env.fill_ix_with(&t, &offer, &[a], &[b], CORE_ID);
    ix.accounts[3].pubkey = env.stranger.pubkey();
    assert_trade_err(&env.send(&[ix], &taker, &[]).res, TradeError::WrongAccount);
    let decoy = env.mint(&env.maker.pubkey(), base(32), 99);
    let ix = env.fill_ix_with(&t, &offer, &[decoy], &[b], CORE_ID);
    assert_trade_err(&env.send(&[ix], &taker, &[]).res, TradeError::WrongAccount);
    assert_eq!(env.owner_of(&a), env.maker.pubkey());
    assert_eq!(env.owner_of(&b), t);
}

#[test]
fn t23_allowlist_collection_fails_closed() {
    let other = Pubkey::new_unique();
    let mut env = Env::new(RuleSet::ProgramAllowList(vec![other]));
    let exp = in_a_week(&env);
    let (offer, a, b) = simple(&mut env, exp);
    let taker = env.taker.insecure_clone();
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b]).res;
    println!(
        "t23 fill result: {:?}",
        res.as_ref()
            .map(|m| m.compute_units_consumed)
            .map_err(|f| &f.err)
    );
    if let Err(f) = &res {
        for l in &f.meta.logs {
            println!("t23 log: {l}");
        }
    }
    assert!(
        res.is_err(),
        "Core let a transfer through an allowlist that excludes us"
    );
    // It must be Core's Royalties rule that refused, inside our CPI, not one of our own checks.
    let logs = &res.as_ref().unwrap_err().meta.logs;
    assert!(logs
        .iter()
        .any(|l| l.contains("royalties.rs") && l.ends_with("Reject")));
    assert!(logs
        .iter()
        .any(|l| l == &format!("Program {CORE_ID} invoke [2]")));
    assert!(
        custom_error(&res).is_some_and(|(_, c)| c < 6000),
        "expected a Core error code"
    );
    env.assert_nothing_moved(&before, &taker.pubkey(), &res);
    assert!(env.exists(&offer));
}

//! The taker permit and the fixed card registry behind `Cards` wants.
//! Real Core binary.

mod common;

use anchor_lang::{
    prelude::Pubkey,
    solana_program::instruction::{AccountMeta, Instruction},
};
use common::*;
use mpl_core::{instructions::UpdateV1Builder, types::RuleSet};
use mud_trade_book::{error::TradeError, state::Slot, FINISH_ALL, FINISH_BASE, FINISH_FOIL};
use solana_signer::Signer;

/// URI index for card `n` (1..=120), base finish.
fn base(n: u32) -> u32 {
    n - 1
}

/// URI index for card `n`, foil finish.
fn foil(n: u32) -> u32 {
    n + 119
}

fn week(env: &Env) -> i64 {
    env.now() + 7 * DAY
}

fn wallets(env: &Env) -> Vec<Pubkey> {
    vec![
        env.admin.pubkey(),
        env.maker.pubkey(),
        env.taker.pubkey(),
        env.stranger.pubkey(),
    ]
}

fn set_uri(env: &mut Env, asset: Pubkey, new_uri: &str) {
    let admin = env.admin.insecure_clone();
    let ix = UpdateV1Builder::new()
        .asset(asset)
        .collection(Some(env.collection))
        .payer(admin.pubkey())
        .authority(Some(admin.pubkey()))
        .new_uri(new_uri.into())
        .instruction();
    let res = env.send(&[ix], &admin, &[]).res;
    assert!(res.is_ok(), "update uri: {res:#?}");
}

// ---------------------------------------------------------------- taker permit

#[test]
fn p01_taker_needs_a_permit_until_public() {
    let mut env = Env::raw(RuleSet::None);
    let admin = env.admin.insecure_clone();
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let (m, t) = (maker.pubkey(), taker.pubkey());
    assert!(env.set_paused(&admin, false).res.is_ok());
    assert!(env.grant_permit(&admin, m).res.is_ok());

    let a = env.mint(&m, base(32), 1);
    let b = env.mint(&t, base(10), 1);
    let offer = env.list(&maker, 1, &[a], &[Slot::Asset(b)], week(&env));

    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_trade_err(&res, TradeError::NotPermitted);
    env.assert_nothing_moved(&before, &t, &res);

    // A permit granted and then revoked does not count.
    assert!(env.grant_permit(&admin, t).res.is_ok());
    assert!(env.revoke_permit(&admin, t).res.is_ok());
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_trade_err(&res, TradeError::NotPermitted);

    // A lamport-funded account at the permit address is not a permit.
    let permit = env.permit_pda(&t);
    env.svm.airdrop(&permit, SOL).unwrap();
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_trade_err(&res, TradeError::NotPermitted);

    assert!(env.grant_permit(&admin, t).res.is_ok());
    let res = env.fill(&taker, &offer, &[b]).res;
    assert!(res.is_ok(), "permitted taker fills: {res:#?}");
    assert_eq!(env.owner_of(&a), t);
    assert_eq!(env.owner_of(&b), m);
}

#[test]
fn p02_someone_elses_permit_does_not_count() {
    let mut env = Env::raw(RuleSet::None);
    let admin = env.admin.insecure_clone();
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let (m, t) = (maker.pubkey(), taker.pubkey());
    assert!(env.set_paused(&admin, false).res.is_ok());
    assert!(env.grant_permit(&admin, m).res.is_ok());
    let a = env.mint(&m, base(32), 1);
    let b = env.mint(&t, base(10), 1);
    let offer = env.list(&maker, 1, &[a], &[Slot::Asset(b)], week(&env));

    // The maker's permit passed in the taker's slot fails the seeds check.
    let mut ix = env.fill_ix_with(&t, &offer, &[a], &[b], CORE_ID);
    ix.accounts[1].pubkey = env.permit_pda(&m);
    let res = env.send(&[ix], &taker, &[]).res;
    assert!(res.is_err(), "a foreign permit is refused");
    assert_eq!(env.owner_of(&a), m);
    assert_eq!(env.owner_of(&b), t);
}

#[test]
fn p03_public_needs_no_permit_and_cancel_ignores_permits() {
    let mut env = Env::raw(RuleSet::None);
    let admin = env.admin.insecure_clone();
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let (m, t) = (maker.pubkey(), taker.pubkey());
    assert!(env.set_paused(&admin, false).res.is_ok());
    assert!(env.grant_permit(&admin, m).res.is_ok());
    let a = env.mint(&m, base(32), 1);
    let b = env.mint(&t, base(10), 1);
    let offer = env.list(&maker, 1, &[a], &[Slot::Asset(b)], week(&env));

    // Revoking the maker's permit never traps a listed card.
    assert!(env.revoke_permit(&admin, m).res.is_ok());
    assert!(env.cancel(&maker, &offer).res.is_ok());
    assert_eq!(env.owner_of(&a), m);

    assert!(env.open_public(&admin).res.is_ok());
    let offer = env.list(&maker, 2, &[a], &[Slot::Asset(b)], week(&env));
    let res = env.fill(&taker, &offer, &[b]).res;
    assert!(res.is_ok(), "public fill: {res:#?}");
    assert_eq!(env.owner_of(&a), t);
}

// ---------------------------------------------------------------- roots

#[test]
fn g01_add_card_root_is_admin_only_and_append_only() {
    let mut env = Env::new(RuleSet::None);
    let admin = env.admin.insecure_clone();
    let stranger = env.stranger.insecure_clone();
    assert_eq!(env.config().root_count, 0);

    let res = env.add_card_root(&stranger, [7; 32], 10).res;
    assert_trade_err(&res, TradeError::NotAdmin);
    assert_trade_err(&env.add_card_root(&admin, [0; 32], 10).res, TradeError::BadRoot);
    assert_trade_err(&env.add_card_root(&admin, [7; 32], 0).res, TradeError::BadRoot);
    assert_eq!(env.config().root_count, 0);

    assert!(env.add_card_root(&admin, [7; 32], 10).res.is_ok());
    assert!(env.add_card_root(&admin, [8; 32], 20).res.is_ok());
    assert_eq!(env.config().root_count, 2);

    // Only the next index can be created; an earlier one already exists, a later one is
    // off-seed.
    for index in [0u16, 1, 3] {
        let ix = env.add_card_root_ix(&admin.pubkey(), index, [9; 32], 5);
        assert!(env.send(&[ix], &admin, &[]).res.is_err(), "index {index}");
    }
    assert_eq!(env.config().root_count, 2);
    let acc = env.svm.get_account(&env.root_pda(1)).unwrap();
    let root = <mud_trade_book::state::CardRoot as anchor_lang::AccountDeserialize>::try_deserialize(
        &mut &acc.data[..],
    )
    .unwrap();
    assert_eq!((root.root, root.index, root.leaves), ([8; 32], 1, 20));
}

// ---------------------------------------------------------------- register

#[test]
fn g02_register_card_happy_path_and_once_only() {
    let mut env = Env::new(RuleSet::None);
    let t = env.taker.pubkey();
    let stranger = env.stranger.insecure_clone();
    let b = env.mint(&t, foil(14), 3);
    let c = env.mint(&t, base(1), 4);
    let (index, proofs) = env.publish_root(&[(b, 14, FINISH_FOIL), (c, 1, FINISH_BASE)]);

    // Anyone can register, not only the owner.
    let ix = env.register_card_ix(&stranger.pubkey(), index, &b, 14, FINISH_FOIL, proofs[0].clone());
    let res = env.send(std::slice::from_ref(&ix), &stranger, &[]).res;
    assert!(res.is_ok(), "{res:#?}");
    let rec = env.card_record(&b).unwrap();
    assert_eq!((rec.card_no, rec.finish, rec.root_index), (14, FINISH_FOIL, index));

    // A second registration, under any root, is refused (the record already exists).
    let admin = env.admin.insecure_clone();
    let ix = env.register_card_ix(&admin.pubkey(), index, &b, 14, FINISH_FOIL, proofs[0].clone());
    assert!(env.send(&[ix], &admin, &[]).res.is_err());
    let (index2, proofs2) = env.publish_root(&[(b, 14, FINISH_FOIL)]);
    let ix = env.register_card_ix(&stranger.pubkey(), index2, &b, 14, FINISH_FOIL, proofs2[0].clone());
    assert!(env.send(&[ix], &stranger, &[]).res.is_err());
    assert_eq!(env.card_record(&b).unwrap().root_index, index);
}

#[test]
fn g03_register_refuses_every_disagreement() {
    let mut env = Env::new(RuleSet::None);
    let t = env.taker.pubkey();
    let payer = env.stranger.insecure_clone();
    let p = payer.pubkey();
    let b = env.mint(&t, base(10), 1);
    let c = env.mint(&t, base(11), 1);
    let unlisted = env.mint(&t, base(10), 2);
    let other = env.create_collection(RuleSet::None);
    let foreign = env.mint_in(other, &t, &uri(base(10), 3));
    let (index, proofs) = env.publish_root(&[
        (b, 10, FINISH_BASE),
        (c, 11, FINISH_BASE),
        (foreign, 10, FINISH_BASE),
    ]);
    let pb = proofs[0].clone();

    let cfg = env.config;
    type Case = (&'static str, Instruction, TradeError);
    let cases: Vec<Case> = vec![
        ("wrong card number", env.register_card_ix(&p, index, &b, 11, FINISH_BASE, pb.clone()), TradeError::CardMismatch),
        ("wrong finish", env.register_card_ix(&p, index, &b, 10, FINISH_FOIL, pb.clone()), TradeError::CardMismatch),
        ("finish not a single bit", env.register_card_ix(&p, index, &b, 10, FINISH_ALL, pb.clone()), TradeError::CardMismatch),
        ("another leaf's proof", env.register_card_ix(&p, index, &b, 10, FINISH_BASE, proofs[1].clone()), TradeError::BadProof),
        ("asset not in the root", env.register_card_ix(&p, index, &unlisted, 10, FINISH_BASE, pb.clone()), TradeError::BadProof),
        ("proof too long", env.register_card_ix(&p, index, &b, 10, FINISH_BASE, vec![[1; 32]; 25]), TradeError::ProofTooLong),
        ("another collection", env.register_card_ix(&p, index, &foreign, 10, FINISH_BASE, proofs[2].clone()), TradeError::WrongCollection),
        ("not a Core asset", env.register_card_ix(&p, index, &cfg, 10, FINISH_BASE, pb.clone()), TradeError::NotCoreAsset),
    ];
    for (name, ix, err) in cases {
        let res = env.send(&[ix], &payer, &[]).res;
        assert_trade_err(&res, err);
        println!("g03 {name}: {err:?}");
    }

    // A proof against the wrong root index fails; so does a root index that does not exist.
    let (index2, _) = env.publish_root(&[(unlisted, 10, FINISH_BASE)]);
    let ix = env.register_card_ix(&p, index2, &b, 10, FINISH_BASE, pb.clone());
    assert_trade_err(&env.send(&[ix], &payer, &[]).res, TradeError::BadProof);
    let ix = env.register_card_ix(&p, 9, &b, 10, FINISH_BASE, pb);
    assert!(env.send(&[ix], &payer, &[]).res.is_err());
    for k in [b, c, unlisted, foreign] {
        assert!(env.card_record(&k).is_none());
    }
}

#[test]
fn g04_root_and_uri_must_agree() {
    let mut env = Env::new(RuleSet::None);
    let t = env.taker.pubkey();
    let admin = env.admin.insecure_clone();
    // The admin publishes a root that lies about a card: card 10 listed as card 120.
    let b = env.mint(&t, base(10), 1);
    let (index, proofs) = env.publish_root(&[(b, 120, FINISH_BASE)]);
    let ix = env.register_card_ix(&admin.pubkey(), index, &b, 120, FINISH_BASE, proofs[0].clone());
    assert_trade_err(&env.send(&[ix], &admin, &[]).res, TradeError::CardMismatch);
    let ix = env.register_card_ix(&admin.pubkey(), index, &b, 10, FINISH_BASE, proofs[0].clone());
    assert_trade_err(&env.send(&[ix], &admin, &[]).res, TradeError::BadProof);
    assert!(env.card_record(&b).is_none());
}

// ---------------------------------------------------------------- fills read the record

#[test]
fn g05_record_fixes_identity_against_later_uri_changes() {
    let mut env = Env::new(RuleSet::None);
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let b = env.mint(&t, base(10), 1);
    env.register_cards(&[b]);
    // The URI later says card 120. The record still says card 10.
    set_uri(&mut env, b, &uri(base(120), 1));

    let a = env.mint(&m, base(32), 1);
    let wants_120 = env.list(&maker, 1, &[a], &[cards_slot(&[120], FINISH_ALL)], week(&env));
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &wants_120, &[b]).res;
    assert_trade_err(&res, TradeError::SlotMismatch);
    env.assert_nothing_moved(&before, &t, &res);
    assert!(env.cancel(&maker, &wants_120).res.is_ok());

    let wants_10 = env.list(&maker, 2, &[a], &[cards_slot(&[10], FINISH_BASE)], week(&env));
    let res = env.fill(&taker, &wants_10, &[b]).res;
    assert!(res.is_ok(), "{res:#?}");
    assert_eq!(env.owner_of(&a), t);
    assert_eq!(env.owner_of(&b), m);
}

#[test]
fn g06_fill_refuses_bad_records() {
    let mut env = Env::new(RuleSet::None);
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let a = env.mint(&m, base(32), 1);
    let b = env.mint(&t, base(10), 1);
    let twin = env.mint(&t, base(10), 2);
    let unregistered = env.mint(&t, base(10), 3);
    env.register_cards(&[b, twin]);
    let offer = env.list(&maker, 1, &[a], &[cards_slot(&[10], FINISH_ALL)], week(&env));
    // 9 named accounts, then give, taker asset, Listed, then the record.
    let record_at = 12;

    let cases: Vec<(&str, Pubkey, Pubkey)> = vec![
        ("another card's record", b, env.card_pda(&twin)),
        ("unregistered card", unregistered, env.card_pda(&unregistered)),
        ("program account of another type", b, env.offer_pda(&m, 1)),
        ("Config", b, env.config),
        ("system account", b, t),
        ("the asset itself", b, b),
    ];
    for (name, asset, record) in cases {
        let mut ix = env.fill_ix_with(&t, &offer, &[a], &[asset], CORE_ID);
        ix.accounts[record_at] = AccountMeta::new_readonly(record, false);
        let before = env.snap(&wallets(&env), &[a, b, twin, unregistered]);
        let res = env.send(&[ix], &taker, &[]).res;
        assert_trade_err(&res, TradeError::BadCardRecord);
        env.assert_nothing_moved(&before, &t, &res);
        println!("g06 {name}: refused");
    }

    // Record missing or one too many: the count is checked first.
    let mut ix = env.fill_ix_with(&t, &offer, &[a], &[b], CORE_ID);
    ix.accounts.pop();
    assert_trade_err(&env.send(&[ix], &taker, &[]).res, TradeError::AccountCount);
    let mut ix = env.fill_ix_with(&t, &offer, &[a], &[b], CORE_ID);
    ix.accounts.push(AccountMeta::new_readonly(env.card_pda(&twin), false));
    assert_trade_err(&env.send(&[ix], &taker, &[]).res, TradeError::AccountCount);

    let res = env.fill(&taker, &offer, &[b]).res;
    assert!(res.is_ok(), "{res:#?}");
    assert_eq!(env.owner_of(&b), m);
}

#[test]
fn g07_records_follow_slot_order_with_mixed_slots() {
    let mut env = Env::new(RuleSet::None);
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let a = env.mint(&m, base(32), 1);
    let x = env.mint(&t, base(50), 1);
    let c1 = env.mint(&t, base(1), 1);
    let c2 = env.mint(&t, foil(2), 1);
    let c3 = env.mint(&t, base(3), 1);
    env.register_cards(&[c1, c2, c3]);
    let want = [
        cards_slot(&[1], FINISH_BASE),
        Slot::Asset(x),
        cards_slot(&[2], FINISH_FOIL),
        cards_slot(&[3], FINISH_ALL),
    ];
    let offer = env.list(&maker, 1, &[a], &want, week(&env));

    // Records swapped between two Cards slots: each check reads the wrong card's record.
    let mut ix = env.fill_ix_with(&t, &offer, &[a], &[c1, x, c2, c3], CORE_ID);
    let n = ix.accounts.len();
    ix.accounts.swap(n - 3, n - 2);
    let before = env.snap(&wallets(&env), &[a, x, c1, c2, c3]);
    let res = env.send(&[ix], &taker, &[]).res;
    assert_trade_err(&res, TradeError::BadCardRecord);
    env.assert_nothing_moved(&before, &t, &res);

    let res = env.fill(&taker, &offer, &[c1, x, c2, c3]).res;
    assert!(res.is_ok(), "{res:#?}");
    for k in [x, c1, c2, c3] {
        assert_eq!(env.owner_of(&k), m);
    }
    assert_eq!(env.owner_of(&a), t);
}

#[test]
fn g08_registration_survives_owner_change_and_records_are_permanent() {
    let mut env = Env::new(RuleSet::None);
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let b = env.mint(&t, base(10), 1);
    env.register_cards(&[b]);
    let record = env.card_pda(&b);
    let rent = env.lamports(&record);
    assert!(rent > 0);

    // The card trades into the maker's wallet and back out again on another offer: one record
    // serves every fill.
    let a = env.mint(&m, base(32), 1);
    let offer = env.list(&maker, 1, &[a], &[cards_slot(&[10], FINISH_ALL)], week(&env));
    assert!(env.fill(&taker, &offer, &[b]).res.is_ok());
    let c = env.mint(&t, base(33), 1);
    let offer = env.list(&taker, 1, &[c], &[cards_slot(&[10], FINISH_ALL)], week(&env));
    let stranger = env.stranger.insecure_clone();
    let s = stranger.pubkey();
    env.core_transfer(b, &maker, &s);
    assert!(env.fill(&stranger, &offer, &[b]).res.is_ok());
    assert_eq!(env.owner_of(&b), t);
    assert_eq!(env.lamports(&record), rent);
}

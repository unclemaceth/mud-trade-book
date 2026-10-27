//! v2 regressions: per-offer delegates (external audit M-01), cleanup that revokes whatever
//! collection a card is in now (M-02), the taker-paid trade fee and the Serialized finish.
//! Real Core binary.

mod common;

use anchor_lang::{prelude::Pubkey, solana_program::instruction::AccountMeta};
use common::*;
use mpl_core::{
    instructions::UpdateV2Builder,
    types::{PluginAuthority, RuleSet, UpdateAuthority},
};
use mud_trade_book::{
    error::TradeError, state::Slot, FINISH_ALL, FINISH_BASE, FINISH_FOIL, FINISH_SERIALIZED,
    MAX_FEE_LAMPORTS,
};
use solana_signer::Signer;

fn base(n: u32) -> u32 {
    n - 1
}

fn foil(n: u32) -> u32 {
    n + 119
}

fn serialized(n: u32) -> u32 {
    n + 239
}

fn week(env: &Env) -> i64 {
    env.now() + 7 * DAY
}

fn wallets(env: &Env) -> Vec<Pubkey> {
    vec![
        env.maker.pubkey(),
        env.taker.pubkey(),
        env.stranger.pubkey(),
        env.admin.pubkey(),
        env.fee_recipient,
    ]
}

#[track_caller]
fn assert_revoked(env: &Env, a: &Pubkey) {
    assert!(
        matches!(env.read(a).transfer_delegate, Some(PluginAuthority::Owner)),
        "delegation on {a} not returned to the owner: {:?}",
        env.read(a).transfer_delegate
    );
}

#[track_caller]
fn assert_ok(res: &litesvm::types::TransactionResult, what: &str) {
    assert!(res.is_ok(), "{what}: {res:#?}");
}

/// The configured collection's authority moves `a` into `other`.
fn move_to(env: &mut Env, a: Pubkey, other: Pubkey) {
    let admin = env.admin.insecure_clone();
    let ix = UpdateV2Builder::new()
        .asset(a)
        .collection(Some(env.collection))
        .payer(admin.pubkey())
        .authority(Some(admin.pubkey()))
        .new_collection(Some(other))
        .new_update_authority(UpdateAuthority::Collection(other))
        .instruction();
    assert_ok(&env.send(&[ix], &admin, &[]).res, "move collection");
}

/// The configured collection's authority takes `a` out of every collection.
fn remove_from_collection(env: &mut Env, a: Pubkey) {
    let admin = env.admin.insecure_clone();
    let ix = UpdateV2Builder::new()
        .asset(a)
        .collection(Some(env.collection))
        .payer(admin.pubkey())
        .authority(Some(admin.pubkey()))
        .new_update_authority(UpdateAuthority::Address(admin.pubkey()))
        .instruction();
    assert_ok(&env.send(&[ix], &admin, &[]).res, "remove from collection");
}

/// One exact offer: maker's base #32 for taker's base #10.
fn exact(env: &mut Env, offer_id: u64) -> (Pubkey, Pubkey, Pubkey) {
    let maker = env.maker.insecure_clone();
    let (m, t) = (maker.pubkey(), env.taker.pubkey());
    let a = env.mint(&m, base(32), offer_id as u32);
    let b = env.mint(&t, base(10), offer_id as u32);
    let exp = week(env);
    let offer = env.list(&maker, offer_id, &[a], &[Slot::Asset(b)], exp);
    (offer, a, b)
}

// ---------------------------------------------------------------- M-01: per-offer delegates

/// The external audit's reproduction, inverted: re-approving the old shared delegate no longer
/// revives the offer.
#[test]
fn m01_reapproving_the_legacy_shared_delegate_does_not_revive() {
    let mut env = Env::new(RuleSet::None);
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let (offer, a, b) = exact(&mut env, 1);
    env.revoke_delegate(a, &maker);
    assert_trade_err(
        &env.fill(&taker, &offer, &[b]).res,
        TradeError::DelegateMissing,
    );

    let legacy = env.legacy_delegate;
    let ix = env.approve_ix(a, &maker.pubkey(), legacy);
    assert_ok(&env.send(&[ix], &maker, &[]).res, "approve legacy");
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_trade_err(&res, TradeError::DelegateMissing);
    env.assert_nothing_moved(&before, &taker.pubkey(), &res);
}

/// Approving a card for a different offer's delegate does not revive this one.
#[test]
fn m01_another_offers_delegate_does_not_revive() {
    let mut env = Env::new(RuleSet::None);
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let m = maker.pubkey();
    let (offer, a, b) = exact(&mut env, 1);
    let ixs = env.redelegate_ixs(a, &m, env.delegate_of(&env.offer_pda(&m, 2)));
    assert_ok(&env.send(&ixs, &maker, &[]).res, "approve offer 2");
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_trade_err(&res, TradeError::DelegateMissing);
    env.assert_nothing_moved(&before, &taker.pubkey(), &res);

    // The stale offer can be closed by anyone; offer 2's approval survives the cleanup.
    let stranger = env.stranger.insecure_clone();
    assert_ok(&env.close(&stranger, &offer).res, "close stale");
    assert!(env.delegated_to(&a, &env.offer_pda(&m, 2)));
}

/// Residual, by design: approving this exact offer's own delegate again is the maker consenting
/// to that offer again. Nothing in the supported client does this.
#[test]
fn m01_reapproving_the_same_offers_delegate_is_explicit_consent() {
    let mut env = Env::new(RuleSet::None);
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let (offer, a, b) = exact(&mut env, 1);
    env.revoke_delegate(a, &maker);
    assert_trade_err(
        &env.fill(&taker, &offer, &[b]).res,
        TradeError::DelegateMissing,
    );
    env.add_delegate(a, &maker, &offer);
    assert_ok(
        &env.fill(&taker, &offer, &[b]).res,
        "fill after same-offer approval",
    );
    assert_eq!(env.owner_of(&a), taker.pubkey());
}

/// The card leaves the maker and comes back. Core resets the delegation on transfer, the old
/// offer stays dead, and a relist goes through close-then-list.
#[test]
fn m01_ownership_round_trip_leaves_the_old_offer_dead() {
    let mut env = Env::new(RuleSet::None);
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let stranger = env.stranger.insecure_clone();
    let m = maker.pubkey();
    let (offer, a, b) = exact(&mut env, 1);
    env.core_transfer(a, &maker, &stranger.pubkey());
    env.core_transfer(a, &stranger, &m);
    assert_revoked(&env, &a);
    assert_trade_err(
        &env.fill(&taker, &offer, &[b]).res,
        TradeError::DelegateMissing,
    );

    // The old Listed PDA blocks a new listing until the stale offer is closed.
    let o2 = env.offer_pda(&m, 2);
    let mut ixs = vec![env.delegate_ix(a, &m, &o2)];
    ixs.push(env.create_offer_ix(&m, 2, &[a], &[Slot::Asset(b)], week(&env)));
    assert_trade_err(&env.send(&ixs, &maker, &[]).res, TradeError::AlreadyListed);

    assert_ok(&env.close(&stranger, &offer).res, "close stale");
    let ixs = vec![
        env.delegate_ix(a, &m, &o2),
        env.create_offer_ix(&m, 2, &[a], &[Slot::Asset(b)], week(&env)),
    ];
    assert_ok(&env.send(&ixs, &maker, &[]).res, "relist");
    assert_ok(&env.fill(&taker, &o2, &[b]).res, "fill relist");
    assert_eq!(env.owner_of(&a), taker.pubkey());
    assert_eq!(env.owner_of(&b), m);
}

/// A cancelled offer's id is never created again, so its delegate address never names a second
/// offer: an approval for it, old or new, can only ever satisfy the cancelled one, which is gone.
#[test]
fn m01_cancelled_offer_id_is_never_reused() {
    let mut env = Env::new(RuleSet::None);
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let (m, t) = (maker.pubkey(), taker.pubkey());
    let (offer, a, b) = exact(&mut env, 1);
    assert_ok(&env.cancel(&maker, &offer).res, "cancel");
    assert_revoked(&env, &a);

    let c = env.mint(&t, base(11), 1);
    env.add_delegate(a, &maker, &offer);
    let res = env
        .create_offer(&maker, 1, &[a], &[Slot::Asset(c)], week(&env))
        .res;
    assert_trade_err(&res, TradeError::OfferIdNotNext);
    assert!(!env.exists(&offer));
    env.revoke_delegate(a, &maker);

    let next = env.list(&maker, 2, &[a], &[Slot::Asset(c)], week(&env));
    assert_ne!(next, offer);
    let before = env.snap(&wallets(&env), &[a, b, c]);
    let res = env.fill(&taker, &next, &[b]).res;
    assert_trade_err(&res, TradeError::SlotMismatch);
    env.assert_nothing_moved(&before, &t, &res);
    assert_ok(&env.fill(&taker, &next, &[c]).res, "fill new terms");
    assert_eq!(env.owner_of(&c), m);
}

/// Multi-card: one card's delegation moved to another offer. Fill fails, and cleanup revokes
/// only this offer's delegations.
#[test]
fn m01_multi_card_cleanup_touches_only_its_own_delegate() {
    let mut env = Env::new(RuleSet::None);
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let (m, t) = (maker.pubkey(), taker.pubkey());
    let a = env.mint(&m, base(1), 1);
    let c = env.mint(&m, base(2), 1);
    let b = env.mint(&t, base(9), 1);
    let offer = env.list(&maker, 1, &[a, c], &[Slot::Asset(b)], week(&env));
    let other = env.offer_pda(&m, 7);
    let ixs = env.redelegate_ixs(a, &m, env.delegate_of(&other));
    assert_ok(&env.send(&ixs, &maker, &[]).res, "redelegate a");
    assert_trade_err(
        &env.fill(&taker, &offer, &[b]).res,
        TradeError::DelegateMissing,
    );

    assert_ok(&env.cancel(&maker, &offer).res, "cancel");
    assert_revoked(&env, &c);
    assert!(
        env.delegated_to(&a, &other),
        "cleanup touched another offer's delegate"
    );
    assert!(!env.exists(&offer));
}

// ------------------------------------------------- M-02: revoke whatever collection a card is in

#[test]
fn m02_close_revokes_a_card_moved_to_another_collection() {
    let mut env = Env::new(RuleSet::None);
    let stranger = env.stranger.insecure_clone();
    let (offer, a, _b) = exact(&mut env, 1);
    let other = env.create_collection(RuleSet::None);
    move_to(&mut env, a, other);

    // Without the card's new collection the close refuses, so no delegation is left behind.
    let res = env.close(&stranger, &offer).res;
    assert_trade_err(&res, TradeError::MissingCollection);
    assert!(env.exists(&offer));

    let mut ix = env.close_ix(&stranger.pubkey(), &offer);
    ix.accounts.push(AccountMeta::new(other, false));
    assert_ok(
        &env.send(&[ix], &stranger, &[]).res,
        "close with collection",
    );
    assert_revoked(&env, &a);
    assert!(!env.exists(&offer) && !env.exists(&env.listed_pda(&a)));
}

#[test]
fn m02_cancel_revokes_a_card_moved_to_another_collection() {
    let mut env = Env::new(RuleSet::None);
    let maker = env.maker.insecure_clone();
    let (offer, a, _b) = exact(&mut env, 1);
    let other = env.create_collection(RuleSet::None);
    move_to(&mut env, a, other);
    assert_trade_err(
        &env.cancel(&maker, &offer).res,
        TradeError::MissingCollection,
    );

    let mut ix = env.cancel_ix(&maker.pubkey(), &offer);
    ix.accounts.push(AccountMeta::new(other, false));
    assert_ok(&env.send(&[ix], &maker, &[]).res, "cancel with collection");
    assert_revoked(&env, &a);
}

#[test]
fn m02_cleanup_revokes_a_card_removed_from_its_collection() {
    let mut env = Env::new(RuleSet::None);
    let stranger = env.stranger.insecure_clone();
    let (offer, a, _b) = exact(&mut env, 1);
    remove_from_collection(&mut env, a);
    assert_eq!(env.read(&a).collection, None);
    assert_ok(&env.close(&stranger, &offer).res, "close");
    assert_revoked(&env, &a);
}

/// A decoy among the extra collections changes nothing, the list is capped at four, and a
/// card that left the maker is still skipped (the new owner's delegation is theirs).
#[test]
fn m02_extra_collections_are_bounded_and_never_hit_a_new_owner() {
    let mut env = Env::new(RuleSet::None);
    let maker = env.maker.insecure_clone();
    let stranger = env.stranger.insecure_clone();
    let (m, s) = (maker.pubkey(), stranger.pubkey());
    let a = env.mint(&m, base(1), 1);
    let c = env.mint(&m, base(2), 1);
    let b = env.mint(&env.taker.pubkey(), base(9), 1);
    let offer = env.list(&maker, 1, &[a, c], &[Slot::Asset(b)], week(&env));
    let other = env.create_collection(RuleSet::None);
    move_to(&mut env, a, other);
    env.core_transfer(c, &maker, &s);
    env.add_delegate(c, &stranger, &env.offer_pda(&s, 1));

    let mut ix = env.close_ix(&s, &offer);
    for _ in 0..5 {
        ix.accounts.push(AccountMeta::new(other, false));
    }
    assert_trade_err(
        &env.send(&[ix], &stranger, &[]).res,
        TradeError::AccountCount,
    );

    let decoy = env.create_collection(RuleSet::None);
    let mut ix = env.close_ix(&s, &offer);
    ix.accounts.push(AccountMeta::new(decoy, false));
    ix.accounts.push(AccountMeta::new(other, false));
    assert_ok(&env.send(&[ix], &stranger, &[]).res, "close");
    assert_revoked(&env, &a);
    assert!(
        env.delegated_to(&c, &env.offer_pda(&s, 1)),
        "new owner's delegation revoked"
    );
}

// ------------------------------------------------------------------------ the trade fee

fn priced(env: &mut Env, lamports: u64) {
    let admin = env.admin.insecure_clone();
    let recipient = env.fee_recipient;
    assert_ok(&env.set_fee(&admin, recipient, lamports).res, "set_fee");
}

#[test]
fn fee_taker_pays_exactly_the_fee_to_the_recipient() {
    let mut env = Env::new(RuleSet::None);
    priced(&mut env, 1_000_000);
    let taker = env.taker.insecure_clone();
    let (m, t) = (env.maker.pubkey(), taker.pubkey());
    let (offer, a, b) = exact(&mut env, 1);
    let rent = env.offer_rent(&offer);
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_ok(&res, "fill");
    assert_eq!(
        env.lamports(&t),
        before.lamports[1].1 - fee_of(&res) - 1_000_000
    );
    assert_eq!(
        env.lamports(&env.fee_recipient),
        before.lamports[4].1 + 1_000_000
    );
    assert_eq!(env.lamports(&m), before.lamports[0].1 + rent);
}

#[test]
fn fee_zero_moves_no_lamports() {
    let mut env = Env::new(RuleSet::None);
    let taker = env.taker.insecure_clone();
    let (offer, _a, b) = exact(&mut env, 1);
    assert_eq!(env.fee_config().unwrap().lamports, 0);
    let before = env.lamports(&env.fee_recipient);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_ok(&res, "fill");
    assert_eq!(env.lamports(&env.fee_recipient), before);
}

/// The admin raises the fee after the taker signed: the fill fails instead of overcharging.
#[test]
fn fee_raise_after_signing_is_refused() {
    let mut env = Env::new(RuleSet::None);
    priced(&mut env, 1_000_000);
    let taker = env.taker.insecure_clone();
    let t = taker.pubkey();
    let (offer, a, b) = exact(&mut env, 1);
    let give = env.offer(&offer).give;
    let signed = env.fill_ix_fee(&t, &offer, &give, &[b], CORE_ID, 1_000_000);
    priced(&mut env, 2_000_000);
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.send(&[signed], &taker, &[]).res;
    assert_trade_err(&res, TradeError::FeeAboveMax);
    env.assert_nothing_moved(&before, &t, &res);
}

/// The admin redirects the fee after the taker signed, or the taker names another wallet:
/// both are refused.
#[test]
fn fee_recipient_is_pinned() {
    let mut env = Env::new(RuleSet::None);
    priced(&mut env, 1_000_000);
    let taker = env.taker.insecure_clone();
    let t = taker.pubkey();
    let (offer, a, b) = exact(&mut env, 1);
    let give = env.offer(&offer).give;
    let stranger = env.stranger.pubkey();
    let ix = env.fill_ix_full(&t, &offer, &give, &[b], CORE_ID, 1_000_000, stranger);
    let before = env.snap(&wallets(&env), &[a, b]);
    let res = env.send(&[ix], &taker, &[]).res;
    assert_trade_err(&res, TradeError::WrongFeeRecipient);
    env.assert_nothing_moved(&before, &t, &res);

    let signed = env.fill_ix_fee(&t, &offer, &give, &[b], CORE_ID, 1_000_000);
    let admin = env.admin.insecure_clone();
    assert_ok(&env.set_fee(&admin, stranger, 1_000_000).res, "redirect");
    assert_trade_err(
        &env.send(&[signed], &taker, &[]).res,
        TradeError::WrongFeeRecipient,
    );
}

#[test]
fn fee_bounds_and_admin_only() {
    let mut env = Env::new(RuleSet::None);
    let admin = env.admin.insecure_clone();
    let stranger = env.stranger.insecure_clone();
    let r = env.fee_recipient;
    assert_trade_err(
        &env.set_fee(&admin, r, MAX_FEE_LAMPORTS + 1).res,
        TradeError::BadFee,
    );
    assert_trade_err(
        &env.set_fee(&admin, Pubkey::default(), 0).res,
        TradeError::BadFeeRecipient,
    );
    assert_trade_err(&env.set_fee(&stranger, r, 1).res, TradeError::NotAdmin);
    assert_ok(&env.set_fee(&admin, r, MAX_FEE_LAMPORTS).res, "fee at cap");
    let f = env.fee_config().unwrap();
    assert_eq!((f.recipient, f.lamports), (r, MAX_FEE_LAMPORTS));

    // init runs once.
    let res = env.init_fee_config(&admin, r, 0).res;
    assert!(res.is_err(), "second init_fee_config succeeded");
    assert_eq!(env.fee_config().unwrap().lamports, MAX_FEE_LAMPORTS);
}

/// Before the fee account exists, fills fail and cancel stays open. Only the admin can create it.
#[test]
fn fee_account_missing_blocks_fills_not_cancels() {
    let mut env = Env::bare(RuleSet::None);
    let admin = env.admin.insecure_clone();
    let stranger = env.stranger.insecure_clone();
    let col = env.collection;
    for res in [
        env.init_config(&admin, col).res,
        env.set_paused(&admin, false).res,
        env.open_public(&admin).res,
    ] {
        assert_ok(&res, "config");
    }
    let r = env.fee_recipient;
    assert_trade_err(
        &env.init_fee_config(&stranger, r, 0).res,
        TradeError::NotAdmin,
    );
    assert_trade_err(
        &env.init_fee_config(&admin, r, MAX_FEE_LAMPORTS + 1).res,
        TradeError::BadFee,
    );
    assert!(env.fee_config().is_none());

    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let (offer, a, b) = exact(&mut env, 1);
    let res = env.fill(&taker, &offer, &[b]).res;
    assert_eq!(custom_error(&res).map(|(_, c)| c), Some(3012), "{res:#?}");
    assert_eq!(env.owner_of(&a), maker.pubkey());

    let (offer2, _, _) = exact(&mut env, 2);
    assert_ok(
        &env.cancel(&maker, &offer2).res,
        "cancel without fee account",
    );
    assert_ok(&env.init_fee_config(&admin, r, 0).res, "init");
    assert_ok(&env.fill(&taker, &offer, &[b]).res, "fill after init");
}

// --------------------------------------------------------------------- Serialized finish

#[test]
fn serialized_cards_register_with_their_live_metadata_numbers() {
    let mut env = Env::new(RuleSet::None);
    let t = env.taker.pubkey();
    let cases = [(240, 0, 1u8), (300, 1, 61), (359, 0, 120)];
    let assets: Vec<Pubkey> = cases.iter().map(|(i, s, _)| env.mint(&t, *i, *s)).collect();
    env.register_cards(&assets);
    for (a, (_, _, n)) in assets.iter().zip(cases) {
        let r = env.card_record(a).expect("registered");
        assert_eq!((r.card_no, r.finish), (n, FINISH_SERIALIZED), "{a}");
    }

    let over = env.mint(&t, 360, 0);
    let (root, proofs) = env.publish_root(&[(over, 1, FINISH_SERIALIZED)]);
    let admin = env.admin.insecure_clone();
    let ix = env.register_card_ix(
        &admin.pubkey(),
        root,
        &over,
        1,
        FINISH_SERIALIZED,
        proofs[0].clone(),
    );
    assert_trade_err(
        &env.send(&[ix], &admin, &[]).res,
        TradeError::UnrecognisedCard,
    );
}

#[test]
fn register_refuses_a_finish_the_uri_does_not_carry() {
    let mut env = Env::new(RuleSet::None);
    let t = env.taker.pubkey();
    let admin = env.admin.insecure_clone();
    let b = env.mint(&t, base(1), 1);
    for finish in [FINISH_SERIALIZED, FINISH_BASE | FINISH_FOIL, FINISH_ALL, 0] {
        let (root, proofs) = env.publish_root(&[(b, 1, finish)]);
        let ix = env.register_card_ix(&admin.pubkey(), root, &b, 1, finish, proofs[0].clone());
        assert_trade_err(&env.send(&[ix], &admin, &[]).res, TradeError::CardMismatch);
    }
    assert!(env.card_record(&b).is_none());
}

/// Finish masks: Serialized-only refuses base and foil; base|foil refuses Serialized; four base
/// cards for one serialized card fills.
#[test]
fn serialized_finish_mask_matching() {
    let mut env = Env::new(RuleSet::None);
    let maker = env.maker.insecure_clone();
    let taker = env.taker.insecure_clone();
    let (m, t) = (maker.pubkey(), taker.pubkey());
    let give: Vec<Pubkey> = (1..=4).map(|n| env.mint(&m, base(n), 1)).collect();
    let b1 = env.mint(&t, base(1), 1);
    let f1 = env.mint(&t, foil(1), 1);
    let s1 = env.mint(&t, serialized(1), 7);

    let only_serial = env.list(
        &maker,
        1,
        &give,
        &[cards_slot(&[1], FINISH_SERIALIZED)],
        week(&env),
    );
    env.register_cards(&[b1, f1, s1]);
    for wrong in [b1, f1] {
        let before = env.snap(&wallets(&env), &[wrong]);
        let res = env.fill(&taker, &only_serial, &[wrong]).res;
        assert_trade_err(&res, TradeError::SlotMismatch);
        env.assert_nothing_moved(&before, &t, &res);
    }
    assert_ok(&env.cancel(&maker, &only_serial).res, "cancel");

    let no_serial = env.list(
        &maker,
        2,
        &give,
        &[cards_slot(&[1], FINISH_BASE | FINISH_FOIL)],
        week(&env),
    );
    assert_trade_err(
        &env.fill(&taker, &no_serial, &[s1]).res,
        TradeError::SlotMismatch,
    );
    assert_ok(&env.cancel(&maker, &no_serial).res, "cancel");

    let four_for_one = env.list(
        &maker,
        3,
        &give,
        &[cards_slot(&[1], FINISH_SERIALIZED)],
        week(&env),
    );
    assert_ok(
        &env.fill(&taker, &four_for_one, &[s1]).res,
        "four base for one serialized",
    );
    assert_eq!(env.owner_of(&s1), m);
    for a in &give {
        assert_eq!(env.owner_of(a), t);
    }
}

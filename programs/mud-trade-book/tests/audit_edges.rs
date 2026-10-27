//! External audit 2: its four reproduction tests, kept and turned to
//! the fixed behaviour, plus the fee-recipient (L-03) and unreadable-asset (I-01) edge cases it
//! asked for. Real Core binary.

mod common;

use anchor_lang::{
    prelude::Pubkey,
    solana_program::{instruction::AccountMeta, system_program},
};
use common::*;
use mpl_core::types::RuleSet;
use mud_trade_book::{error::TradeError, state::Slot};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;

const CORE_ID: Pubkey = mpl_core::ID;

#[track_caller]
fn assert_ok(res: &litesvm::types::TransactionResult, what: &str) {
    assert!(res.is_ok(), "{what}: {res:#?}");
}

/// One exact offer from the maker (card #2 for the taker's #3), and both cards.
fn setup() -> (Env, Pubkey, Pubkey, Pubkey) {
    let mut e = Env::new(RuleSet::None);
    let maker = e.maker.insecure_clone();
    let a = e.mint(&maker.pubkey(), 1, 1);
    let b = e.mint(&e.taker.pubkey(), 2, 1);
    let exp = e.now() + DAY;
    let o = e.list(&maker, 1, &[a], &[Slot::Asset(b)], exp);
    (e, o, a, b)
}

/// A funded system wallet, as a fee recipient must be.
fn wallet(e: &mut Env, lamports: u64) -> Keypair {
    let k = Keypair::new();
    e.svm.airdrop(&k.pubkey(), lamports).expect("airdrop");
    k
}

// ---------------------------------------------------------------- the audit's four tests

#[test]
fn fee_transfer_rolls_back_on_late_core_failure() {
    let (mut e, o, a, b) = setup();
    let admin = e.admin.insecure_clone();
    let maker = e.maker.insecure_clone();
    let taker = e.taker.insecure_clone();
    let r = e.fee_recipient;
    assert_ok(&e.set_fee(&admin, r, 1_000_000).res, "fee");
    e.freeze(a, &maker);
    let before = e.lamports(&r);
    assert!(e.fill(&taker, &o, &[b]).res.is_err());
    assert_eq!(e.lamports(&r), before);
    assert_eq!(e.owner_of(&a), maker.pubkey());
    assert_eq!(e.owner_of(&b), taker.pubkey());
}

/// Was: the fee could be paid to the live Offer account and come back to the maker on close.
/// Now a program-owned recipient is refused when the admin sets it.
#[test]
fn fee_to_offer_account_is_refused() {
    let (mut e, o, _a, _b) = setup();
    let admin = e.admin.insecure_clone();
    let res = e.set_fee(&admin, o, 1_000_000).res;
    assert_trade_err(&res, TradeError::BadFeeRecipient);
    assert_eq!(e.fee_config().unwrap().recipient, e.fee_recipient);
}

/// Was: an unfunded recipient was accepted. Now it must hold at least the rent-exempt minimum
/// for an empty account, so a later payment can never fail on rent.
#[test]
fn unfunded_or_below_rent_recipient_is_refused() {
    let (mut e, _o, _a, _b) = setup();
    let admin = e.admin.insecure_clone();
    let empty_rent = e.svm.minimum_balance_for_rent_exemption(0);
    let unfunded = Keypair::new().pubkey();
    assert_trade_err(
        &e.set_fee(&admin, unfunded, 1).res,
        TradeError::BadFeeRecipient,
    );
    let short = wallet(&mut e, empty_rent - 1).pubkey();
    assert_trade_err(
        &e.set_fee(&admin, short, 1).res,
        TradeError::BadFeeRecipient,
    );
    let exact = wallet(&mut e, empty_rent).pubkey();
    assert_ok(
        &e.set_fee(&admin, exact, 1).res,
        "exactly the empty-account minimum",
    );
}

/// Was: cancel then create with the same id in the same slot made a second instance with the
/// same address and `created_slot`. Now the id is spent once used.
#[test]
fn reused_offer_id_in_the_same_slot_is_refused() {
    let (mut e, o, a, b) = setup();
    let maker = e.maker.insecure_clone();
    let first = e.offer(&o).created_slot;
    assert_ok(&e.cancel(&maker, &o).res, "cancel");
    e.add_delegate(a, &maker, &o);
    let exp = e.now() + DAY;
    let res = e.create_offer(&maker, 1, &[a], &[Slot::Asset(b)], exp).res;
    assert_trade_err(&res, TradeError::OfferIdNotNext);
    assert!(!e.exists(&o));
    e.revoke_delegate(a, &maker);
    let second = e.list(&maker, 2, &[a], &[Slot::Asset(b)], exp);
    assert_ne!(second, o);
    assert_eq!(
        e.offer(&second).created_slot,
        first,
        "same slot, different address"
    );
}

// ---------------------------------------------------------------- L-03: recipient aliases

/// Every program-owned or executable account the audit named is refused as a recipient: the
/// offer (above), its Listed PDA, a Core asset, the Config, the program itself, the system
/// program and Core.
#[test]
fn program_owned_and_executable_recipients_are_refused() {
    let (mut e, o, a, _b) = setup();
    let admin = e.admin.insecure_clone();
    let listed = e.listed_pda(&a);
    let program_id = e.program_id;
    let config = e.config;
    for (name, r) in [
        ("offer", o),
        ("listed", listed),
        ("core asset", a),
        ("config", config),
        ("this program", program_id),
        ("system program", system_program::ID),
        ("core", CORE_ID),
    ] {
        let res = e.set_fee(&admin, r, 1).res;
        assert_trade_err(&res, TradeError::BadFeeRecipient);
        println!("recipient {name}: refused");
    }
}

/// The maker's or taker's own wallet is a plain system wallet, so the program allows it: the
/// maker is paid the fee, or the taker pays themselves. Neither changes which cards move.
#[test]
fn maker_or_taker_as_recipient_is_a_plain_payment() {
    let (mut e, o, a, b) = setup();
    let admin = e.admin.insecure_clone();
    let taker = e.taker.insecure_clone();
    let (m, t) = (e.maker.pubkey(), taker.pubkey());
    assert_ok(&e.set_fee(&admin, m, 1_000_000).res, "maker as recipient");
    let rent = e.offer_rent(&o);
    let before = e.lamports(&m);
    let res = e.fill(&taker, &o, &[b]).res;
    assert_ok(&res, "fill paying the maker");
    assert_eq!(e.lamports(&m), before + rent + 1_000_000);
    assert_eq!((e.owner_of(&a), e.owner_of(&b)), (t, m));

    let maker = e.maker.insecure_clone();
    let c = e.mint(&m, 4, 1);
    let d = e.mint(&t, 5, 1);
    let exp = e.now() + DAY;
    let o2 = e.list(&maker, 2, &[c], &[Slot::Asset(d)], exp);
    assert_ok(&e.set_fee(&admin, t, 1_000_000).res, "taker as recipient");
    let before = e.lamports(&t);
    let res = e.fill(&taker, &o2, &[d]).res;
    assert_ok(&res, "fill paying the taker");
    assert_eq!(
        e.lamports(&t),
        before - fee_of(&res),
        "the fee nets to zero"
    );
    assert_eq!((e.owner_of(&c), e.owner_of(&d)), (t, m));
}

/// The recipient wallet's owner reassigns it to a program after it was set. Fills then fail
/// (nothing moves) instead of paying a program account; cancel is unaffected.
#[test]
fn recipient_reassigned_after_setup_blocks_fills_not_cancels() {
    let (mut e, o, a, b) = setup();
    let admin = e.admin.insecure_clone();
    let maker = e.maker.insecure_clone();
    let taker = e.taker.insecure_clone();
    let r = wallet(&mut e, 1_000_000_000);
    assert_ok(&e.set_fee(&admin, r.pubkey(), 1_000_000).res, "fee");
    let mut data = 1u32.to_le_bytes().to_vec();
    data.extend_from_slice(CORE_ID.as_ref());
    let assign = Instruction {
        program_id: system_program::ID,
        accounts: vec![AccountMeta::new(r.pubkey(), true)],
        data,
    };
    assert_ok(&e.send(&[assign], &r, &[]).res, "assign the recipient away");

    let res = e.fill(&taker, &o, &[b]).res;
    assert_trade_err(&res, TradeError::BadFeeRecipient);
    assert_eq!(
        (e.owner_of(&a), e.owner_of(&b)),
        (maker.pubkey(), taker.pubkey())
    );
    assert_ok(&e.cancel(&maker, &o).res, "cancel");
}

// ---------------------------------------------------------------- I-01: unreadable assets

/// Overwrites a give card's account with Core-owned bytes this program cannot decode.
fn corrupt(e: &mut Env, a: &Pubkey, data: Vec<u8>) {
    let mut acc = e.svm.get_account(a).unwrap();
    acc.data = data;
    e.svm.set_account(*a, acc).unwrap();
}

/// A Core-owned give card that does not decode stops cancel and close (`UnreadableAsset`)
/// instead of being skipped with a delegation possibly still on it. Nothing is released.
#[test]
fn unreadable_core_asset_fails_cleanup_closed() {
    type Mangle = fn(&[u8]) -> Vec<u8>;
    let cases: [(&str, Mangle); 3] = [
        ("truncated asset", |d| d[..40].to_vec()),
        ("another Core key", |d| {
            let mut v = d.to_vec();
            v[0] = 2;
            v
        }),
        ("plugin header cut off", |d| d[..d.len() - 20].to_vec()),
    ];
    for (name, mangle) in cases {
        let (mut e, o, a, _b) = setup();
        let maker = e.maker.insecure_clone();
        let stranger = e.stranger.insecure_clone();
        let data = e.svm.get_account(&a).unwrap().data;
        corrupt(&mut e, &a, mangle(&data));
        let res = e.cancel(&maker, &o).res;
        assert_trade_err(&res, TradeError::UnreadableAsset);
        assert!(
            e.exists(&o) && e.exists(&e.listed_pda(&a)),
            "{name}: released"
        );
        e.warp_to(e.now() + 2 * DAY);
        assert_trade_err(&e.close(&stranger, &o).res, TradeError::UnreadableAsset);
        println!("{name}: UnreadableAsset");
    }
}

/// Accounts that can no longer carry a delegation are still skipped: closed (system-owned, no
/// data) and burned (one `Uninitialized` byte). The rest of the offer is released.
#[test]
fn gone_assets_are_skipped() {
    for (name, gone) in [
        (
            "closed",
            solana_account::Account {
                lamports: 0,
                data: vec![],
                owner: system_program::ID,
                executable: false,
                rent_epoch: 0,
            },
        ),
        (
            "burned",
            solana_account::Account {
                lamports: 1_000_000,
                data: vec![0],
                owner: CORE_ID,
                executable: false,
                rent_epoch: 0,
            },
        ),
    ] {
        let mut e = Env::new(RuleSet::None);
        let maker = e.maker.insecure_clone();
        let m = maker.pubkey();
        let a = e.mint(&m, 1, 1);
        let c = e.mint(&m, 4, 1);
        let b = e.mint(&e.taker.pubkey(), 2, 1);
        let exp = e.now() + DAY;
        let o = e.list(&maker, 1, &[a, c], &[Slot::Asset(b)], exp);
        e.svm.set_account(a, gone).unwrap();
        assert_ok(&e.cancel(&maker, &o).res, name);
        assert!(!e.exists(&o) && !e.exists(&e.listed_pda(&c)));
        assert!(
            !e.delegated_to(&c, &o),
            "{name}: the readable card kept its delegation"
        );
    }
}

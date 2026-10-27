//! Property test of `fill_offer` against LiteSVM and the real Core binary.
//!
//! Each case lists random give cards for random slots, then fills with a random account list
//! (orderings, duplicates, a foreign-collection asset, a random-URI asset, the maker's own card)
//! and a random give-account order. An independent model predicts success. Invariant: either
//! nothing moved, or exactly the offer's give cards went to the taker and the passed taker cards
//! went to the maker, and the outcome matches the model.
//!
//! Cases: `MTB_FUZZ_CASES` (default 256).

mod common;

use anchor_lang::prelude::Pubkey;
use common::*;
use mpl_core::types::RuleSet;
use mud_trade_book::state::Slot;
use proptest::{
    prelude::*,
    test_runner::{Config, TestRunner},
};
use solana_signer::Signer;

const PREFIX: &str = "https://assets.mudwater.bayc.com/m/c/";
const POOL: usize = 6;

/// Independent oracle for the URI rule (not the program's parser).
fn oracle_card(uri: &str) -> Option<(u8, bool)> {
    let rest = uri.strip_prefix(PREFIX)?;
    let (i, s) = rest.split_once('-')?;
    let ok = |p: &str| {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && (p == "0" || !p.starts_with('0'))
    };
    if !ok(i) || !ok(s) || i.len() > 3 || s.len() > 10 {
        return None;
    }
    let idx: u64 = i.parse().ok()?;
    let serial: u64 = s.parse().ok()?;
    if idx > 239 || serial > u64::from(u32::MAX) {
        return None;
    }
    Some(((idx % 120 + 1) as u8, idx >= 120))
}

#[derive(Debug, Clone)]
enum SlotSpec {
    Asset(usize),
    Cards {
        include: Vec<bool>,
        extra: Vec<u8>,
        mask: u8,
    },
}

#[derive(Debug, Clone)]
struct Case {
    give_idx: Vec<u32>,
    pool_idx: Vec<u32>,
    bad_uri: String,
    want: Vec<SlotSpec>,
    /// Indexes into [pool 0..6, foreign, random-URI, maker's give[0]].
    pick: Vec<usize>,
    /// 0 exact, 1 reversed, 2 first duplicated, 3 last dropped.
    give_order: u8,
    /// 0 random picks; 1 honest picks; 2 honest then swap the first two; 3 honest then
    /// duplicate the first.
    mode: u8,
}

fn slot_spec() -> impl Strategy<Value = SlotSpec> {
    prop_oneof![
        (0..POOL).prop_map(SlotSpec::Asset),
        (
            prop::collection::vec(any::<bool>(), POOL),
            prop::collection::vec(1u8..=120, 0..3),
            1u8..=3
        )
            .prop_map(|(include, extra, mask)| SlotSpec::Cards {
                include,
                extra,
                mask
            }),
    ]
}

fn case() -> impl Strategy<Value = Case> {
    let bad_uri = prop_oneof![
        "[ -~]{0,60}",
        "[0-9+-]{0,14}".prop_map(|t| format!("{PREFIX}{t}")),
        "[0-9]{1,3}-[0-9]{1,11}".prop_map(|t| format!("{PREFIX}{t}")),
    ];
    (
        prop::collection::vec(0u32..240, 1..=4),
        prop::collection::vec(0u32..240, POOL),
        bad_uri,
        prop::collection::vec(slot_spec(), 1..=4),
        prop::collection::vec(0usize..POOL + 3, 0..=5),
        prop_oneof![3 => Just(0u8), 1 => 1u8..4],
        0u8..4,
    )
        .prop_map(
            |(give_idx, pool_idx, bad_uri, want, pick, give_order, mode)| Case {
                give_idx,
                pool_idx,
                bad_uri,
                want,
                pick,
                give_order,
                mode,
            },
        )
}

#[derive(Default)]
struct Tally {
    cases: usize,
    create_refused: usize,
    fills_ok: usize,
    fills_refused: usize,
}

/// For each slot, the first unused candidate that satisfies it.
fn honest(
    want: &[Slot],
    candidates: &[Pubkey],
    card_of: &dyn Fn(&Pubkey) -> Option<(u8, bool)>,
) -> Vec<Pubkey> {
    let mut out: Vec<Pubkey> = Vec::new();
    for slot in want {
        let hit = candidates.iter().find(|k| {
            !out.contains(k)
                && match slot {
                    Slot::Asset(w) => w == *k,
                    Slot::Cards { cards, finish_mask } => card_of(k).is_some_and(|(n, foil)| {
                        let i = usize::from(n - 1);
                        cards[i / 8] & (1 << (i % 8)) != 0
                            && finish_mask & if foil { 2 } else { 1 } != 0
                    }),
                }
        });
        if let Some(k) = hit {
            out.push(*k);
        }
    }
    out
}

fn run(c: &Case, tally: &std::cell::RefCell<Tally>) -> Result<(), TestCaseError> {
    tally.borrow_mut().cases += 1;
    let mut env = Env::new(RuleSet::None);
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let give: Vec<Pubkey> = c
        .give_idx
        .iter()
        .enumerate()
        .map(|(i, x)| env.mint(&m, *x, i as u32))
        .collect();
    let pool: Vec<Pubkey> = c
        .pool_idx
        .iter()
        .enumerate()
        .map(|(i, x)| env.mint(&t, *x, 100 + i as u32))
        .collect();
    let other = env.create_collection(RuleSet::None);
    let foreign = env.mint_in(other, &t, &uri(c.pool_idx[0], 999));
    let col = env.collection;
    let bad = env.mint_in(col, &t, &c.bad_uri);
    let candidates: Vec<Pubkey> = pool
        .iter()
        .copied()
        .chain([foreign, bad, give[0]])
        .collect();
    let card_of = |k: &Pubkey| -> Option<(u8, bool)> {
        if let Some(i) = pool.iter().position(|p| p == k) {
            return oracle_card(&uri(c.pool_idx[i], 100 + i as u32));
        }
        if *k == bad {
            return oracle_card(&c.bad_uri);
        }
        None
    };

    let want: Vec<Slot> = c
        .want
        .iter()
        .map(|s| match s {
            SlotSpec::Asset(i) => Slot::Asset(pool[*i]),
            SlotSpec::Cards {
                include,
                extra,
                mask,
            } => {
                let mut nums: Vec<u8> = include
                    .iter()
                    .zip(&c.pool_idx)
                    .filter(|(inc, _)| **inc)
                    .map(|(_, idx)| (idx % 120 + 1) as u8)
                    .collect();
                nums.extend(extra);
                cards_slot(&nums, *mask)
            }
        })
        .collect();

    // Model of create_offer: Asset slots distinct, every Cards slot non-empty.
    let asset_keys: Vec<Pubkey> = want
        .iter()
        .filter_map(|s| {
            if let Slot::Asset(k) = s {
                Some(*k)
            } else {
                None
            }
        })
        .collect();
    let distinct_assets = asset_keys
        .iter()
        .enumerate()
        .all(|(i, k)| !asset_keys[..i].contains(k));
    let nonempty = want.iter().all(|s| match s {
        Slot::Cards { cards, .. } => cards.iter().any(|b| *b != 0),
        Slot::Asset(_) => true,
    });
    let mut ixs: Vec<_> = give.iter().map(|a| env.delegate_ix(*a, &m)).collect();
    let exp = env.now() + DAY;
    ixs.push(env.create_offer_ix(&m, 1, &give, &want, exp));
    let maker = env.maker.insecure_clone();
    let created = env.send(&ixs, &maker, &[]).res;
    prop_assert_eq!(
        created.is_ok(),
        distinct_assets && nonempty,
        "create model: {:?}",
        created
    );
    if created.is_err() {
        tally.borrow_mut().create_refused += 1;
        for a in give.iter().chain(&candidates[..POOL + 2]) {
            prop_assert_eq!(env.owner_of(a), if give.contains(a) { m } else { t });
        }
        return Ok(());
    }
    let offer = env.offer_pda(&m, 1);
    let rent = env.offer_rent(&offer);

    let mut picked: Vec<Pubkey> = if c.mode == 0 {
        c.pick.iter().map(|i| candidates[*i]).collect()
    } else {
        honest(&want, &candidates[..POOL + 2], &card_of)
    };
    match c.mode {
        2 if picked.len() > 1 => picked.swap(0, 1),
        3 if !picked.is_empty() => picked.push(picked[0]),
        _ => {}
    }
    let mut give_passed = give.clone();
    match c.give_order {
        1 => give_passed.reverse(),
        2 => give_passed.insert(0, give[0]),
        3 if give.len() > 1 => {
            give_passed.pop();
        }
        _ => {}
    }

    // Model of fill_offer.
    let distinct = picked
        .iter()
        .enumerate()
        .all(|(i, k)| !picked[..i].contains(k));
    let expect_ok = give_passed == give
        && picked.len() == want.len()
        && distinct
        && !picked.iter().any(|k| give.contains(k))
        && picked.iter().all(|k| *k != foreign)
        && picked.iter().zip(&want).all(|(k, slot)| match slot {
            Slot::Asset(w) => w == k,
            Slot::Cards { cards, finish_mask } => card_of(k).is_some_and(|(n, foil)| {
                let i = usize::from(n - 1);
                cards[i / 8] & (1 << (i % 8)) != 0 && finish_mask & if foil { 2 } else { 1 } != 0
            }),
        });

    let mut tracked = give.clone();
    tracked.extend(&candidates[..POOL + 2]);
    env.prepare_cards(&offer, &picked);
    let before = env.snap(&[m, t], &tracked);
    let taker = env.taker.insecure_clone();
    let ix = env.fill_ix_with(&t, &offer, &give_passed, &picked, CORE_ID);
    let res = env.send(&[ix], &taker, &[]).res;
    prop_assert_eq!(
        res.is_ok(),
        expect_ok,
        "fill model mismatch: {:?} for {:?}",
        res,
        c
    );

    if res.is_ok() {
        tally.borrow_mut().fills_ok += 1;
        for a in &tracked {
            let want_owner = if give.contains(a) {
                t
            } else if picked.contains(a) {
                m
            } else {
                before.owners.iter().find(|(k, _)| k == a).unwrap().1
            };
            prop_assert_eq!(env.owner_of(a), want_owner, "asset {} ended wrong", a);
        }
        prop_assert!(!env.exists(&offer));
        prop_assert_eq!(env.lamports(&m), before.lamports[0].1 + rent);
        prop_assert_eq!(env.lamports(&t), before.lamports[1].1 - fee_of(&res));
    } else {
        tally.borrow_mut().fills_refused += 1;
        env.assert_nothing_moved(&before, &t, &res);
        prop_assert!(env.exists(&offer));
    }
    Ok(())
}

#[test]
fn fuzz_fill_offer_invariant() {
    let cases: u32 = std::env::var("MTB_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    let mut runner = TestRunner::new(Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    });
    let tally = std::cell::RefCell::new(Tally::default());
    let started = std::time::Instant::now();
    let result = runner.run(&case(), |c| run(&c, &tally));
    let tally = tally.into_inner();
    println!(
        "FUZZ {} cases in {:.1}s: {} create refused (model agreed), {} fills ok, {} fills refused",
        tally.cases,
        started.elapsed().as_secs_f64(),
        tally.create_refused,
        tally.fills_ok,
        tally.fills_refused
    );
    result.unwrap();
}

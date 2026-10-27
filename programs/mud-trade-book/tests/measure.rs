//! Measurements: compute units per fill shape, largest transaction size, rent.
//! Plus a realism check: the program's reader over real mainnet account bytes.

mod common;

use anchor_lang::{prelude::Pubkey, Space};
use base64::Engine;
use common::*;
use mpl_core::types::RuleSet;
use mud_trade_book::{state::Slot, FINISH_ALL};
use solana_signer::Signer;

/// Lists `n_give` maker cards for `n_want` Cards slots and fills it; returns (CU, tx bytes).
fn measure(n_give: u32, n_want: u32, exact: bool) -> (u64, usize) {
    let mut env = Env::new(RuleSet::None);
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let give: Vec<Pubkey> = (0..n_give)
        .map(|i| env.mint(&m, 100 + i, 4_000_000_000))
        .collect();
    let got: Vec<Pubkey> = (0..n_want)
        .map(|i| env.mint(&t, 120 + i, 4_000_000_000 + i))
        .collect();
    let want: Vec<Slot> = got
        .iter()
        .enumerate()
        .map(|(i, a)| {
            if exact {
                Slot::Asset(*a)
            } else {
                cards_slot(&[i as u8 + 1], FINISH_ALL)
            }
        })
        .collect();
    let maker = env.maker.insecure_clone();
    let exp = env.now() + DAY;
    let offer = env.list(&maker, 1, &give, &want, exp);
    let taker = env.taker.insecure_clone();
    let sent = env.fill(&taker, &offer, &got);
    assert!(sent.res.is_ok(), "{:#?}", sent.res);
    for a in &give {
        assert_eq!(env.owner_of(a), t);
    }
    for a in &got {
        assert_eq!(env.owner_of(a), m);
    }
    (cu_of(&sent.res), sent.size)
}

#[test]
fn measure_compute_units_and_tx_size() {
    let mut largest = 0;
    for (g, w) in [(1, 1), (4, 1), (1, 4), (4, 4)] {
        for exact in [true, false] {
            let (cu, size) = measure(g, w, exact);
            let kind = if exact { "Asset slots" } else { "Cards slots" };
            println!("MEASURE fill {g}->{w} ({kind}): {cu} CU, {size} bytes");
            assert!(cu < 400_000, "over the 400k target");
            assert!(size <= 1232, "over the packet limit");
            largest = largest.max(size);
        }
    }
    println!("MEASURE largest fill tx: {largest} bytes (limit 1232)");
}

/// One listing transaction exactly as a client sends it: one Add or Approve per give card, then
/// `create_offer`. `approve[i]` gives card i a TransferDelegate plugin first (owner-revoked), so
/// the harness picks Approve for it. Wants are `Asset` slots, the larger encoding.
fn measure_create(approve: &[bool], n_want: u32) -> (u64, usize) {
    let mut env = Env::new(RuleSet::None);
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let maker = env.maker.insecure_clone();
    let give: Vec<Pubkey> = (0..approve.len() as u32)
        .map(|i| env.mint(&m, 100 + i, 4_000_000_000 + i))
        .collect();
    for (a, &ap) in give.iter().zip(approve) {
        if ap {
            env.add_delegate(*a, &maker, &env.offer_pda(&m, 1));
            env.revoke_delegate(*a, &maker);
        }
    }
    let want: Vec<Slot> = (0..n_want)
        .map(|i| Slot::Asset(env.mint(&t, 120 + i, 4_000_000_000 + i)))
        .collect();
    let offer = env.offer_pda(&m, 1);
    let mut ixs: Vec<_> = give
        .iter()
        .map(|a| env.delegate_ix(*a, &m, &offer))
        .collect();
    ixs.push(env.create_offer_ix(&m, 1, &give, &want, env.now() + DAY));
    let sent = env.send(&ixs, &maker, &[]);
    assert!(sent.res.is_ok(), "{:#?}", sent.res);
    for a in &give {
        assert!(env.delegated_to(a, &offer));
    }
    (cu_of(&sent.res), sent.size)
}

#[test]
fn measure_create_tx_size() {
    let mut largest = 0;
    for n in 1..=4usize {
        let mixes: Vec<Vec<bool>> = vec![
            vec![false; n],
            vec![true; n],
            (0..n).map(|i| i % 2 == 1).collect(),
        ];
        for mix in mixes {
            for w in [1, 4] {
                let (cu, size) = measure_create(&mix, w);
                let kinds: String = mix.iter().map(|a| if *a { 'P' } else { 'A' }).collect();
                println!(
                    "MEASURE create {n}->{w} [{kinds}] (A=Add, P=Approve): {cu} CU, {size} bytes"
                );
                assert!(
                    size <= 1232,
                    "create {n}->{w} [{kinds}] is {size} bytes, over 1232"
                );
                assert!(cu < 400_000, "over the 400k target");
                largest = largest.max(size);
            }
        }
    }
    println!("MEASURE largest create tx: {largest} bytes (limit 1232)");
}

/// Cancel and close of a 4-give offer, each revoking all four delegations.
#[test]
fn measure_cancel_and_close() {
    let mut env = Env::new(RuleSet::None);
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let maker = env.maker.insecure_clone();
    let give: Vec<Pubkey> = (0..4)
        .map(|i| env.mint(&m, 100 + i, 4_000_000_000 + i))
        .collect();
    let want: Vec<Slot> = (0..4)
        .map(|i| Slot::Asset(env.mint(&t, 120 + i, 4_000_000_000 + i)))
        .collect();
    let exp = env.now() + DAY;
    let offer = env.list(&maker, 1, &give, &want, exp);
    let sent = env.cancel(&maker, &offer);
    assert!(sent.res.is_ok(), "{:#?}", sent.res);
    println!(
        "MEASURE cancel 4 give (4 revokes): {} CU, {} bytes",
        cu_of(&sent.res),
        sent.size
    );

    let offer = env.list(&maker, 2, &give, &want, exp);
    env.warp_to(exp);
    let stranger = env.stranger.insecure_clone();
    let sent = env.close(&stranger, &offer);
    assert!(sent.res.is_ok(), "{:#?}", sent.res);
    println!(
        "MEASURE close 4 give (4 revokes): {} CU, {} bytes",
        cu_of(&sent.res),
        sent.size
    );
    for a in &give {
        assert!(!env.delegated_to(a, &offer));
    }
}

/// `register_card` at a given proof depth. The root is folded from random siblings, so any
/// depth up to the cap is a valid proof without building a 2^depth tree.
fn measure_register(depth: usize) -> (u64, usize) {
    let mut env = Env::new(RuleSet::None);
    let t = env.taker.pubkey();
    let b = env.mint(&t, 9, 4_000_000_000);
    let proof: Vec<[u8; 32]> = (0..depth).map(|i| [i as u8 + 1; 32]).collect();
    let leaf = mud_trade_book::registry::leaf_hash(&b, 10, mud_trade_book::FINISH_BASE);
    let root = proof
        .iter()
        .fold(leaf, |acc, s| mud_trade_book::registry::node_hash(&acc, s));
    let admin = env.admin.insecure_clone();
    assert!(env
        .add_card_root(&admin, root, 1 << depth.min(31))
        .res
        .is_ok());
    let payer = env.stranger.insecure_clone();
    let ix = env.register_card_ix(
        &payer.pubkey(),
        0,
        &b,
        10,
        mud_trade_book::FINISH_BASE,
        proof,
    );
    let sent = env.send(&[ix], &payer, &[]);
    assert!(sent.res.is_ok(), "{:#?}", sent.res);
    (cu_of(&sent.res), sent.size)
}

#[test]
fn measure_register_card() {
    for depth in [0usize, 14, 17, 24] {
        let (cu, size) = measure_register(depth);
        println!("MEASURE register_card depth {depth}: {cu} CU, {size} bytes");
        assert!(size <= 1232, "register_card depth {depth} is {size} bytes");
        assert!(cu < 200_000, "over the default compute budget");
    }
}

#[test]
fn measure_rent() {
    let env = Env::new(RuleSet::None);
    let offer_len = 8 + mud_trade_book::state::Offer::INIT_SPACE;
    let listed_len = 8 + mud_trade_book::state::Listed::INIT_SPACE;
    let config_len = 8 + mud_trade_book::state::Config::INIT_SPACE;
    let permit_len = 8 + mud_trade_book::state::MakerPermit::INIT_SPACE;
    let r = |len| env.svm.minimum_balance_for_rent_exemption(len);
    println!(
        "MEASURE MakerPermit: {permit_len} bytes, {} lamports",
        r(permit_len)
    );
    println!(
        "MEASURE Offer (4+4, fixed size): {offer_len} bytes, {} lamports",
        r(offer_len)
    );
    println!(
        "MEASURE Listed: {listed_len} bytes, {} lamports",
        r(listed_len)
    );
    println!(
        "MEASURE Config: {config_len} bytes, {} lamports",
        r(config_len)
    );
    let root_len = 8 + mud_trade_book::state::CardRoot::INIT_SPACE;
    let record_len = 8 + mud_trade_book::state::CardRecord::INIT_SPACE;
    println!(
        "MEASURE CardRoot: {root_len} bytes, {} lamports",
        r(root_len)
    );
    println!(
        "MEASURE CardRecord: {record_len} bytes, {} lamports",
        r(record_len)
    );
    println!(
        "MEASURE 4-give offer total: {} lamports",
        r(offer_len) + 4 * r(listed_len)
    );
}

#[test]
fn measure_real_offer_rent_matches() {
    let mut env = Env::new(RuleSet::None);
    let (m, t) = (env.maker.pubkey(), env.taker.pubkey());
    let give: Vec<Pubkey> = (1..=4).map(|n| env.mint(&m, n, 1)).collect();
    let want: Vec<Slot> = (1..=4u8).map(|n| cards_slot(&[n], FINISH_ALL)).collect();
    let _ = t;
    let maker = env.maker.insecure_clone();
    let exp = env.now() + DAY;
    let offer = env.list(&maker, 1, &give, &want, exp);
    let acc = env.svm.get_account(&offer).unwrap();
    assert_eq!(acc.data.len(), 8 + mud_trade_book::state::Offer::INIT_SPACE);
    assert_eq!(
        acc.lamports,
        env.svm.minimum_balance_for_rent_exemption(acc.data.len())
    );
    let listed = env.svm.get_account(&env.listed_pda(&give[0])).unwrap();
    assert_eq!(
        listed.lamports,
        env.svm
            .minimum_balance_for_rent_exemption(listed.data.len())
    );
    println!(
        "MEASURE live 4+4 Offer account: {} bytes, {} lamports",
        acc.data.len(),
        acc.lamports
    );
}

fn fixture(name: &str) -> (Pubkey, u64, Vec<u8>, Pubkey) {
    let path = format!("{}/../../tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let key: Pubkey = v["pubkey"].as_str().unwrap().parse().unwrap();
    let acc = &v["account"];
    let data = base64::engine::general_purpose::STANDARD
        .decode(acc["data"][0].as_str().unwrap())
        .unwrap();
    let owner: Pubkey = acc["owner"].as_str().unwrap().parse().unwrap();
    (key, acc["lamports"].as_u64().unwrap(), data, owner)
}

#[test]
fn fixture_real_mainnet_card_decodes() {
    let (key, lamports, data, owner) = fixture("card_DHqi.json");
    assert_eq!(owner, CORE_ID);
    let base = mpl_core::accounts::BaseAssetV1::from_bytes(&data).unwrap();
    let asset = decode(&key, lamports, data, &owner).expect("program reader decodes mainnet card");
    let collection: Pubkey = "2Qa62ZQoytj3TL4BtRpj8Eujta3zpG93gqXtBg7C5g3p"
        .parse()
        .unwrap();
    assert_eq!(asset.collection, Some(collection));
    assert_eq!(asset.uri, "https://assets.mudwater.bayc.com/m/c/31-303");
    assert_eq!(base.name, "Mudwater Card 3490-0");
    let card = mud_trade_book::card::parse_card_uri(&asset.uri).expect("in scheme");
    assert_eq!(card.card_no, 32);
    println!("FIXTURE card {key}");
    println!("FIXTURE   owner          {}", asset.owner);
    println!("FIXTURE   update auth    Collection({collection})");
    println!("FIXTURE   name           {}", base.name);
    println!("FIXTURE   uri            {}", asset.uri);
    println!(
        "FIXTURE   parsed         card #{:03} {:?}",
        card.card_no, card.finish
    );
    println!("FIXTURE   transfer deleg {:?}", asset.transfer_delegate);
}

#[test]
fn fixture_real_mainnet_collection_decodes() {
    let (key, lamports, mut data, owner) = fixture("collection_2Qa6.json");
    assert_eq!(owner, CORE_ID);
    let col = mpl_core::accounts::BaseCollectionV1::from_bytes(&data).unwrap();
    let mut lamports = lamports;
    let info = anchor_lang::prelude::AccountInfo::new(
        &key,
        false,
        false,
        &mut lamports,
        &mut data,
        &owner,
        false,
    );
    mud_trade_book::core_io::check_collection(&info).expect("program accepts the collection");
    let (_, royalties, _) = mpl_core::fetch_collection_plugin::<mpl_core::types::Royalties>(
        &info,
        mpl_core::types::PluginType::Royalties,
    )
    .expect("royalties plugin");
    assert_eq!(royalties.basis_points, 500);
    assert_eq!(royalties.rule_set, RuleSet::None);
    println!(
        "FIXTURE   royalties      {} bps, rule_set {:?}, creators {:?}",
        royalties.basis_points, royalties.rule_set, royalties.creators
    );
    println!("FIXTURE collection {key}");
    println!("FIXTURE   update auth    {}", col.update_authority);
    println!("FIXTURE   name           {}", col.name);
    println!("FIXTURE   uri            {}", col.uri);
    println!(
        "FIXTURE   minted/current {}/{}",
        col.num_minted, col.current_size
    );
}

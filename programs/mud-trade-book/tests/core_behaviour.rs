//! What the real Core binary does to a live TransferDelegate when the collection authority
//! moves a card. These pin the facts the release path is built on (external audit M-02).

mod common;

use common::*;
use mpl_core::{instructions::UpdateV2Builder, types::RuleSet, types::UpdateAuthority};
use solana_signer::Signer;

fn listed_card(env: &mut Env) -> anchor_lang::prelude::Pubkey {
    let maker = env.maker.insecure_clone();
    let a = env.mint(&maker.pubkey(), 1, 1);
    let offer = env.offer_pda(&maker.pubkey(), 1);
    env.add_delegate(a, &maker, &offer);
    assert!(env.delegated_to(&a, &offer));
    a
}

#[test]
fn moving_a_card_to_another_collection_keeps_its_transfer_delegate() {
    let mut env = Env::new(RuleSet::None);
    let a = listed_card(&mut env);
    let other = env.create_collection(RuleSet::None);
    let admin = env.admin.insecure_clone();
    let ix = UpdateV2Builder::new()
        .asset(a)
        .collection(Some(env.collection))
        .payer(admin.pubkey())
        .authority(Some(admin.pubkey()))
        .new_collection(Some(other))
        .new_update_authority(UpdateAuthority::Collection(other))
        .instruction();
    let res = env.send(&[ix], &admin, &[]).res;
    assert!(res.is_ok(), "move collection: {res:#?}");
    let after = env.read(&a);
    assert_eq!(after.collection, Some(other));
    assert_eq!(after.owner, env.maker.pubkey());
    assert!(after.delegate_is(&env.delegate_of(&env.offer_pda(&env.maker.pubkey(), 1))));
}

#[test]
fn removing_a_card_from_its_collection_keeps_its_transfer_delegate() {
    let mut env = Env::new(RuleSet::None);
    let a = listed_card(&mut env);
    let admin = env.admin.insecure_clone();
    let ix = UpdateV2Builder::new()
        .asset(a)
        .collection(Some(env.collection))
        .payer(admin.pubkey())
        .authority(Some(admin.pubkey()))
        .new_update_authority(UpdateAuthority::Address(admin.pubkey()))
        .instruction();
    let res = env.send(&[ix], &admin, &[]).res;
    assert!(res.is_ok(), "remove from collection: {res:#?}");
    let after = env.read(&a);
    assert_eq!(after.collection, None);
    assert!(after.delegate_is(&env.delegate_of(&env.offer_pda(&env.maker.pubkey(), 1))));
}

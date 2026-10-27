//! Validation shared by the instructions.

use anchor_lang::prelude::*;

use crate::{
    constants::*,
    core_io::{read_asset, CoreAsset},
    error::TradeError,
    state::Slot,
};

/// A give card that can still be swapped: the expected key, a Core asset in the collection,
/// owned by the maker, with the Trade Book as its transfer delegate.
pub fn check_give_asset(
    info: &AccountInfo,
    expected: &Pubkey,
    maker: &Pubkey,
    collection: &Pubkey,
    delegate: &Pubkey,
) -> Result<CoreAsset> {
    require_keys_eq!(*info.key, *expected, TradeError::WrongAccount);
    let asset = read_asset(info)?;
    require!(
        asset.collection == Some(*collection),
        TradeError::WrongCollection
    );
    require_keys_eq!(asset.owner, *maker, TradeError::NotOwner);
    require!(asset.delegate_is(delegate), TradeError::DelegateMissing);
    Ok(asset)
}

/// Shape rules for `give` and `want` at creation.
pub fn check_offer_shape(give: &[Pubkey], want: &[Slot]) -> Result<()> {
    require!(!give.is_empty(), TradeError::EmptyGive);
    require!(give.len() <= MAX_GIVE, TradeError::TooMany);
    require!(want.len() <= MAX_WANT, TradeError::TooMany);
    require!(!want.is_empty(), TradeError::BadWant);
    for (i, key) in give.iter().enumerate() {
        require!(!give[..i].contains(key), TradeError::DuplicateAsset);
    }
    for (i, slot) in want.iter().enumerate() {
        require!(slot.is_well_formed(), TradeError::BadWant);
        if let Slot::Asset(key) = slot {
            require!(!give.contains(key), TradeError::BadWant);
            require!(!want[..i].contains(&Slot::Asset(*key)), TradeError::BadWant);
        }
    }
    Ok(())
}

pub fn check_expiry_at_creation(now: i64, expires_at: i64) -> Result<()> {
    let latest = now
        .checked_add(MAX_EXPIRY_SECS)
        .ok_or(TradeError::Overflow)?;
    require!(
        now < expires_at && expires_at <= latest,
        TradeError::BadExpiry
    );
    Ok(())
}

/// `now >= expires_at` is expired.
pub fn is_expired(now: i64, expires_at: i64) -> bool {
    now >= expires_at
}

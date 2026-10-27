//! Ending an offer without a fill: revoke this offer's delegation on every give card the maker
//! still holds, so no TransferDelegate pointing at the Trade Book outlives its offer.

use anchor_lang::prelude::*;

use crate::{
    constants::{DELEGATE_SEED, MAX_GIVE},
    core_io::{asset_gone, core_revoke_transfer_delegate, read_asset, RevokeAccounts},
    error::TradeError,
};

pub struct ReleaseAccounts<'a, 'b> {
    pub offer: &'b Pubkey,
    pub maker: &'b Pubkey,
    pub collection_key: &'b Pubkey,
    pub collection: &'b AccountInfo<'a>,
    /// Current collections of give cards that moved out of the configured one.
    pub other_collections: &'b [AccountInfo<'a>],
    pub core_program: &'b AccountInfo<'a>,
    pub payer: &'b AccountInfo<'a>,
    pub delegate: &'b AccountInfo<'a>,
    pub delegate_bump: u8,
    pub system_program: &'b AccountInfo<'a>,
}

/// Each give account must be the offer's own key. A card is revoked when it is still owned by
/// the maker and still delegated to this offer's delegate, whatever collection it is in now; a
/// card that moved owner, was burned or was already revoked is skipped. A Core-owned account
/// that does not decode fails the call (`UnreadableAsset`) rather than risk skipping a live
/// delegation; the owner can still revoke it directly through Core. A card that changed
/// collection needs that collection among `other_collections`, or the call fails, so the
/// delegation can never be left behind. Only this offer's delegate is touched, so a delegation
/// a new owner granted for their own listing is never revoked.
pub fn revoke_listed_delegations<'info>(
    give: &[Pubkey],
    give_infos: &[AccountInfo<'info>],
    accounts: ReleaseAccounts<'info, '_>,
) -> Result<Vec<Pubkey>> {
    for (key, info) in give.iter().zip(give_infos) {
        require_keys_eq!(*info.key, *key, TradeError::WrongAccount);
    }
    require!(
        accounts.other_collections.len() <= MAX_GIVE,
        TradeError::AccountCount
    );
    let delegate_key = *accounts.delegate.key;
    let bump = [accounts.delegate_bump];
    let seeds: &[&[u8]] = &[DELEGATE_SEED, accounts.offer.as_ref(), &bump];
    let mut revoked = Vec::with_capacity(give.len());
    for (key, info) in give.iter().zip(give_infos) {
        if asset_gone(info) {
            continue;
        }
        let asset = read_asset(info).map_err(|_| error!(TradeError::UnreadableAsset))?;
        if asset.owner != *accounts.maker || !asset.delegate_is(&delegate_key) {
            continue;
        }
        let collection = match asset.collection {
            None => None,
            Some(c) if c == *accounts.collection_key => Some(accounts.collection),
            Some(c) => Some(
                accounts
                    .other_collections
                    .iter()
                    .find(|other| *other.key == c)
                    .ok_or(TradeError::MissingCollection)?,
            ),
        };
        core_revoke_transfer_delegate(
            RevokeAccounts {
                core_program: accounts.core_program,
                asset: info,
                collection,
                payer: accounts.payer,
                delegate: accounts.delegate,
                system_program: accounts.system_program,
            },
            &[seeds],
        )?;
        revoked.push(*key);
    }
    Ok(revoked)
}

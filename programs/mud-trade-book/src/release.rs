//! Ending an offer without a fill: revoke our delegation on every give card the maker still
//! lists, so no TransferDelegate pointing at the Trade Book outlives its offer.

use anchor_lang::prelude::*;

use crate::{
    checks::check_give_asset,
    constants::DELEGATE_SEED,
    core_io::{core_revoke_transfer_delegate, RevokeAccounts},
    error::TradeError,
};

pub struct ReleaseAccounts<'a, 'b> {
    pub maker: &'b Pubkey,
    pub collection_key: &'b Pubkey,
    pub collection: &'b AccountInfo<'a>,
    pub core_program: &'b AccountInfo<'a>,
    pub payer: &'b AccountInfo<'a>,
    pub delegate: &'b AccountInfo<'a>,
    pub delegate_bump: u8,
    pub system_program: &'b AccountInfo<'a>,
}

/// Each give account must be the offer's own key. A card is revoked only if it still passes
/// the give check (in the collection, owned by the maker, delegated to us); a card that moved,
/// was burned or was already revoked is skipped, so cancel and close never depend on a card's
/// state. A moved card's delegation was already reset by Core, and skipping it leaves any
/// delegation its new owner granted for their own listing untouched.
pub fn revoke_listed_delegations<'info>(
    give: &[Pubkey],
    give_infos: &[AccountInfo<'info>],
    accounts: ReleaseAccounts<'info, '_>,
) -> Result<Vec<Pubkey>> {
    for (key, info) in give.iter().zip(give_infos) {
        require_keys_eq!(*info.key, *key, TradeError::WrongAccount);
    }
    let delegate_key = *accounts.delegate.key;
    let bump = [accounts.delegate_bump];
    let seeds: &[&[u8]] = &[DELEGATE_SEED, &bump];
    let mut revoked = Vec::with_capacity(give.len());
    for (key, info) in give.iter().zip(give_infos) {
        if check_give_asset(
            info,
            key,
            accounts.maker,
            accounts.collection_key,
            &delegate_key,
        )
        .is_err()
        {
            continue;
        }
        core_revoke_transfer_delegate(
            RevokeAccounts {
                core_program: accounts.core_program,
                asset: info,
                collection: accounts.collection,
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

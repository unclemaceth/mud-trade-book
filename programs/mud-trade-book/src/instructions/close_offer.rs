use anchor_lang::prelude::*;

use crate::{
    checks::{check_give_asset, is_expired},
    constants::*,
    error::TradeError,
    listing::close_listed,
    release::{revoke_listed_delegations, ReleaseAccounts},
    state::{Config, Offer, OfferClosed},
};

#[derive(Accounts)]
pub struct CloseOffer<'info> {
    /// Writable only because Core marks the revoke payer writable; it gains and pays nothing
    /// beyond the fee.
    #[account(mut)]
    pub closer: Signer<'info>,
    #[account(
        mut,
        seeds = [OFFER_SEED, offer.maker.as_ref(), &offer.offer_id.to_le_bytes()],
        bump = offer.bump,
        has_one = maker @ TradeError::WrongAccount,
        close = maker
    )]
    pub offer: Account<'info, Offer>,
    /// CHECK: equal to `offer.maker` (has_one); receives all rent.
    #[account(mut)]
    pub maker: UncheckedAccount<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: this offer's delegate PDA, no data; signs the Core revokes.
    #[account(seeds = [DELEGATE_SEED, offer.key().as_ref()], bump)]
    pub delegate: UncheckedAccount<'info>,
    /// CHECK: must be the configured collection (Core-owned, checked at init). Writable only
    /// because Core's revoke marks it writable; this program never writes it.
    #[account(mut, address = config.collection @ TradeError::WrongCollection)]
    pub collection: UncheckedAccount<'info>,
    /// CHECK: must be Metaplex Core.
    #[account(address = CORE_PROGRAM_ID @ TradeError::WrongProgram)]
    pub core_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// Anyone may close an offer that is expired or stale (a give card left the maker, left the
/// collection, or lost this offer's delegation). Works while paused. Remaining accounts: the
/// give assets (offer order), their `Listed` PDAs (offer order), then the current collection of
/// any give card that left the configured one (writable, any order, at most four). Each give
/// asset key must match the offer, so staleness cannot be faked with a substitute account. Give
/// cards still delegated for this offer are revoked, so a stranger's cleanup leaves no
/// delegation behind.
pub fn handle_close_offer<'info>(ctx: Context<'info, CloseOffer<'info>>) -> Result<()> {
    let offer = &ctx.accounts.offer;
    let n = offer.give.len();
    let fixed = n.checked_mul(2).ok_or(TradeError::Overflow)?;
    require!(
        ctx.remaining_accounts.len() >= fixed,
        TradeError::AccountCount
    );
    let (give_infos, rest) = ctx.remaining_accounts.split_at(n);
    let (listed_infos, other_collections) = rest.split_at(n);

    for (key, info) in offer.give.iter().zip(give_infos) {
        require_keys_eq!(*info.key, *key, TradeError::WrongAccount);
    }

    let now = Clock::get()?.unix_timestamp;
    let collection = ctx.accounts.config.collection;
    let delegate_key = ctx.accounts.delegate.key();
    let stale = offer.give.iter().zip(give_infos).any(|(key, info)| {
        check_give_asset(info, key, &offer.maker, &collection, &delegate_key).is_err()
    });
    require!(
        is_expired(now, offer.expires_at) || stale,
        TradeError::NotExpiredOrStale
    );

    let offer_key = offer.key();
    let revoked = revoke_listed_delegations(
        &offer.give,
        give_infos,
        ReleaseAccounts {
            offer: &offer_key,
            maker: &offer.maker,
            collection_key: &collection,
            collection: &ctx.accounts.collection.to_account_info(),
            other_collections,
            core_program: &ctx.accounts.core_program.to_account_info(),
            payer: &ctx.accounts.closer.to_account_info(),
            delegate: &ctx.accounts.delegate.to_account_info(),
            delegate_bump: ctx.bumps.delegate,
            system_program: &ctx.accounts.system_program.to_account_info(),
        },
    )?;

    let maker_info = ctx.accounts.maker.to_account_info();
    for (key, info) in offer.give.iter().zip(listed_infos) {
        close_listed(info, key, &offer_key, &maker_info, ctx.program_id)?;
    }
    emit!(OfferClosed {
        offer: offer_key,
        maker: offer.maker,
        closer: ctx.accounts.closer.key(),
        revoked,
    });
    Ok(())
}

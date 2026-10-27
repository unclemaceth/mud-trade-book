use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::TradeError,
    listing::close_listed,
    release::{revoke_listed_delegations, ReleaseAccounts},
    state::{Config, Offer, OfferCancelled},
};

#[derive(Accounts)]
pub struct CancelOffer<'info> {
    #[account(mut)]
    pub maker: Signer<'info>,
    #[account(
        mut,
        seeds = [OFFER_SEED, offer.maker.as_ref(), &offer.offer_id.to_le_bytes()],
        bump = offer.bump,
        has_one = maker @ TradeError::NotMaker,
        close = maker
    )]
    pub offer: Account<'info, Offer>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: PDA with no data; signs the Core revokes.
    #[account(seeds = [DELEGATE_SEED], bump)]
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

/// Works while paused. Remaining accounts: the give assets (offer order), then their `Listed`
/// PDAs (offer order).
pub fn handle_cancel_offer<'info>(ctx: Context<'info, CancelOffer<'info>>) -> Result<()> {
    let offer = &ctx.accounts.offer;
    let n = offer.give.len();
    let expected_len = n.checked_mul(2).ok_or(TradeError::Overflow)?;
    require!(
        ctx.remaining_accounts.len() == expected_len,
        TradeError::AccountCount
    );
    let (give_infos, listed_infos) = ctx.remaining_accounts.split_at(n);

    let maker_info = ctx.accounts.maker.to_account_info();
    let revoked = revoke_listed_delegations(
        &offer.give,
        give_infos,
        ReleaseAccounts {
            maker: &offer.maker,
            collection_key: &ctx.accounts.config.collection,
            collection: &ctx.accounts.collection.to_account_info(),
            core_program: &ctx.accounts.core_program.to_account_info(),
            payer: &maker_info,
            delegate: &ctx.accounts.delegate.to_account_info(),
            delegate_bump: ctx.bumps.delegate,
            system_program: &ctx.accounts.system_program.to_account_info(),
        },
    )?;

    let offer_key = offer.key();
    for (key, info) in offer.give.iter().zip(listed_infos) {
        close_listed(info, key, &offer_key, &maker_info, ctx.program_id)?;
    }
    emit!(OfferCancelled {
        offer: offer_key,
        maker: offer.maker,
        revoked,
    });
    Ok(())
}

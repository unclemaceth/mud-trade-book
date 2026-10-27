use anchor_lang::prelude::*;

use crate::{
    checks::{check_expiry_at_creation, check_give_asset, check_offer_shape},
    constants::*,
    error::TradeError,
    listing::create_listed,
    state::{Config, MakerState, Offer, OfferCreated, Slot},
};

#[derive(Accounts)]
#[instruction(offer_id: u64)]
pub struct CreateOffer<'info> {
    #[account(mut)]
    pub maker: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: the maker's permit PDA. It counts only while it exists (program-owned); it need
    /// not exist once the Config is public.
    #[account(seeds = [PERMIT_SEED, maker.key().as_ref()], bump)]
    pub maker_permit: UncheckedAccount<'info>,
    #[account(
        init,
        payer = maker,
        space = 8 + Offer::INIT_SPACE,
        seeds = [OFFER_SEED, maker.key().as_ref(), &offer_id.to_le_bytes()],
        bump
    )]
    pub offer: Account<'info, Offer>,
    /// CHECK: this offer's delegate PDA, no data; only its address is used here. The client
    /// derives the offer address first and approves this delegate in the same transaction.
    #[account(seeds = [DELEGATE_SEED, offer.key().as_ref()], bump)]
    pub delegate: UncheckedAccount<'info>,
    /// Created by the maker's first offer. Never closed, so an offer id is never used twice.
    #[account(
        init_if_needed,
        payer = maker,
        space = 8 + MakerState::INIT_SPACE,
        seeds = [MAKER_SEED, maker.key().as_ref()],
        bump
    )]
    pub maker_state: Account<'info, MakerState>,
    pub system_program: Program<'info, System>,
}

/// Remaining accounts: the give assets (in `give` order), then their `Listed` PDAs (same order).
/// `offer_id` must be the maker's last offer id plus one (the first is 1).
pub fn handle_create_offer<'info>(
    ctx: Context<'info, CreateOffer<'info>>,
    offer_id: u64,
    give: Vec<Pubkey>,
    want: Vec<Slot>,
    expires_at: i64,
) -> Result<()> {
    let config = &ctx.accounts.config;
    require!(!config.paused, TradeError::Paused);
    require!(
        config.public || ctx.accounts.maker_permit.owner == ctx.program_id,
        TradeError::NotPermitted
    );
    check_offer_shape(&give, &want)?;
    require!(
        config.cards_enabled || !want.iter().any(Slot::is_cards),
        TradeError::CardsDisabled
    );
    let clock = Clock::get()?;
    check_expiry_at_creation(clock.unix_timestamp, expires_at)?;

    let maker_state = &mut ctx.accounts.maker_state;
    let next = maker_state
        .last_offer_id
        .checked_add(1)
        .ok_or(TradeError::Overflow)?;
    require!(offer_id == next, TradeError::OfferIdNotNext);
    maker_state.last_offer_id = offer_id;
    maker_state.bump = ctx.bumps.maker_state;

    let n = give.len();
    let expected_len = n.checked_mul(2).ok_or(TradeError::Overflow)?;
    require!(
        ctx.remaining_accounts.len() == expected_len,
        TradeError::AccountCount
    );
    let (assets, listeds) = ctx.remaining_accounts.split_at(n);

    let maker_key = ctx.accounts.maker.key();
    let offer_key = ctx.accounts.offer.key();
    let collection = ctx.accounts.config.collection;
    let delegate = ctx.accounts.delegate.key();
    let maker_info = ctx.accounts.maker.to_account_info();

    for ((key, asset_info), listed_info) in give.iter().zip(assets).zip(listeds) {
        check_give_asset(asset_info, key, &maker_key, &collection, &delegate)?;
        create_listed(listed_info, key, offer_key, &maker_info, ctx.program_id)?;
    }

    let offer = &mut ctx.accounts.offer;
    offer.maker = maker_key;
    offer.offer_id = offer_id;
    offer.give = give.clone();
    offer.want = want.clone();
    offer.expires_at = expires_at;
    offer.created_slot = clock.slot;
    offer.bump = ctx.bumps.offer;

    emit!(OfferCreated {
        offer: offer_key,
        maker: maker_key,
        offer_id,
        give,
        want,
        expires_at,
    });
    Ok(())
}

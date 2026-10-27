//! The trade fee: one `["fee"]` PDA, created and changed only by the admin, capped by
//! `MAX_FEE_LAMPORTS`. Fills read it; nothing else does.
//!
//! The recipient is passed as an account and must be a funded system wallet: system-owned, not
//! executable, holding at least the rent-exempt minimum for an empty account. A program account
//! (an offer, a Listed PDA, a CardRecord) or an unfunded address would make every fill that
//! pays it fail. The admin must keep the recipient a plain wallet afterwards; a fill re-checks
//! that it is still system-owned.

use anchor_lang::{prelude::*, system_program};

use crate::{
    constants::*,
    error::TradeError,
    state::{Config, FeeChanged, FeeConfig},
};

#[derive(Accounts)]
pub struct InitFeeConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ TradeError::NotAdmin)]
    pub config: Account<'info, Config>,
    #[account(
        init,
        payer = admin,
        space = 8 + FeeConfig::INIT_SPACE,
        seeds = [FEE_SEED],
        bump
    )]
    pub fee_config: Account<'info, FeeConfig>,
    /// CHECK: checked by `check_recipient`.
    pub recipient: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetFee<'info> {
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ TradeError::NotAdmin)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [FEE_SEED], bump = fee_config.bump)]
    pub fee_config: Account<'info, FeeConfig>,
    /// CHECK: checked by `check_recipient`.
    pub recipient: UncheckedAccount<'info>,
}

fn check_recipient(recipient: &AccountInfo) -> Result<()> {
    require!(
        *recipient.owner == system_program::ID
            && !recipient.executable
            && recipient.lamports() >= Rent::get()?.minimum_balance(0),
        TradeError::BadFeeRecipient
    );
    Ok(())
}

fn check_fee(lamports: u64) -> Result<()> {
    require!(lamports <= MAX_FEE_LAMPORTS, TradeError::BadFee);
    Ok(())
}

pub fn handle_init_fee_config(ctx: Context<InitFeeConfig>, lamports: u64) -> Result<()> {
    check_fee(lamports)?;
    check_recipient(&ctx.accounts.recipient)?;
    let recipient = ctx.accounts.recipient.key();
    let fee = &mut ctx.accounts.fee_config;
    fee.recipient = recipient;
    fee.lamports = lamports;
    fee.bump = ctx.bumps.fee_config;
    emit!(FeeChanged {
        recipient,
        lamports
    });
    Ok(())
}

pub fn handle_set_fee(ctx: Context<SetFee>, lamports: u64) -> Result<()> {
    check_fee(lamports)?;
    check_recipient(&ctx.accounts.recipient)?;
    let recipient = ctx.accounts.recipient.key();
    let fee = &mut ctx.accounts.fee_config;
    fee.recipient = recipient;
    fee.lamports = lamports;
    emit!(FeeChanged {
        recipient,
        lamports
    });
    Ok(())
}

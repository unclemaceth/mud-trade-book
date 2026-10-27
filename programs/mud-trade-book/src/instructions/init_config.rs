use anchor_lang::prelude::*;

use crate::{constants::*, core_io::check_collection, error::TradeError, state::Config};

#[derive(Accounts)]
pub struct InitConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + Config::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Account<'info, Config>,
    /// CHECK: owner-checked against Core and decoded as a collection in the handler.
    pub collection: UncheckedAccount<'info>,
    #[account(
        constraint = program.programdata_address()? == Some(program_data.key())
            @ TradeError::NotUpgradeAuthority
    )]
    pub program: Program<'info, crate::program::MudTradeBook>,
    #[account(
        constraint = program_data.upgrade_authority_address == Some(admin.key())
            @ TradeError::NotUpgradeAuthority
    )]
    pub program_data: Account<'info, ProgramData>,
    pub system_program: Program<'info, System>,
}

/// Born paused, closed to makers without a permit, exact-asset wants only.
pub fn handle_init_config(ctx: Context<InitConfig>) -> Result<()> {
    check_collection(&ctx.accounts.collection)?;
    let config = &mut ctx.accounts.config;
    config.admin = ctx.accounts.admin.key();
    config.paused = true;
    config.collection = ctx.accounts.collection.key();
    config.bump = ctx.bumps.config;
    config.pending_admin = None;
    config.public = false;
    config.cards_enabled = false;
    config.root_count = 0;
    Ok(())
}

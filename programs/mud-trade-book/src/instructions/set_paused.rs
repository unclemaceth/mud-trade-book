use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::TradeError,
    state::{Config, PausedChanged},
};

#[derive(Accounts)]
pub struct SetPaused<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ TradeError::NotAdmin)]
    pub config: Account<'info, Config>,
}

pub fn handle_set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
    ctx.accounts.config.paused = paused;
    emit!(PausedChanged { paused });
    Ok(())
}

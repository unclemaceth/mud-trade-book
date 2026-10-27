use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::TradeError,
    state::{Config, MakerPermit, MakerPermitChanged},
};

#[derive(Accounts)]
#[instruction(maker: Pubkey)]
pub struct GrantMakerPermit<'info> {
    /// Pays the permit's rent and gets it back on revoke.
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ TradeError::NotAdmin)]
    pub config: Account<'info, Config>,
    #[account(
        init,
        payer = admin,
        space = 8 + MakerPermit::INIT_SPACE,
        seeds = [PERMIT_SEED, maker.as_ref()],
        bump
    )]
    pub permit: Account<'info, MakerPermit>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(maker: Pubkey)]
pub struct RevokeMakerPermit<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ TradeError::NotAdmin)]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        seeds = [PERMIT_SEED, maker.as_ref()],
        bump = permit.bump,
        close = admin
    )]
    pub permit: Account<'info, MakerPermit>,
}

pub fn handle_grant_maker_permit(ctx: Context<GrantMakerPermit>, maker: Pubkey) -> Result<()> {
    let permit = &mut ctx.accounts.permit;
    permit.maker = maker;
    permit.bump = ctx.bumps.permit;
    emit!(MakerPermitChanged {
        maker,
        permitted: true,
    });
    Ok(())
}

/// Stops new offers from this maker. Their open offers stay fillable and cancellable.
pub fn handle_revoke_maker_permit(_ctx: Context<RevokeMakerPermit>, maker: Pubkey) -> Result<()> {
    emit!(MakerPermitChanged {
        maker,
        permitted: false,
    });
    Ok(())
}

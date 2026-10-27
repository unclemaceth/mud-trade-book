//! Admin-only Config switches: the two-step admin transfer, opening to every maker, and the
//! card-set want gate.

use anchor_lang::prelude::*;

use crate::{
    constants::*,
    error::TradeError,
    state::{AdminAccepted, CardsEnabledChanged, Config, OpenedPublic, PendingAdminChanged},
};

#[derive(Accounts)]
pub struct AdminConfig<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ TradeError::NotAdmin)]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    pub new_admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
}

/// Replaces any earlier proposal. Nothing changes until `new_admin` accepts.
pub fn handle_propose_admin(ctx: Context<AdminConfig>, new_admin: Pubkey) -> Result<()> {
    let config = &mut ctx.accounts.config;
    require!(
        new_admin != Pubkey::default() && new_admin != config.admin,
        TradeError::BadNewAdmin
    );
    config.pending_admin = Some(new_admin);
    emit!(PendingAdminChanged {
        admin: config.admin,
        pending_admin: config.pending_admin,
    });
    Ok(())
}

pub fn handle_cancel_admin_transfer(ctx: Context<AdminConfig>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    require!(config.pending_admin.is_some(), TradeError::NoPendingAdmin);
    config.pending_admin = None;
    emit!(PendingAdminChanged {
        admin: config.admin,
        pending_admin: None,
    });
    Ok(())
}

pub fn handle_accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    let signer = ctx.accounts.new_admin.key();
    let pending = config.pending_admin.ok_or(TradeError::NoPendingAdmin)?;
    require_keys_eq!(signer, pending, TradeError::NotPendingAdmin);
    let old_admin = config.admin;
    config.admin = signer;
    config.pending_admin = None;
    emit!(AdminAccepted {
        old_admin,
        admin: signer,
    });
    Ok(())
}

/// One way: there is no instruction that sets `public` back to false.
pub fn handle_open_public(ctx: Context<AdminConfig>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    require!(!config.public, TradeError::AlreadyPublic);
    config.public = true;
    emit!(OpenedPublic {});
    Ok(())
}

pub fn handle_set_cards_enabled(ctx: Context<AdminConfig>, enabled: bool) -> Result<()> {
    ctx.accounts.config.cards_enabled = enabled;
    emit!(CardsEnabledChanged { enabled });
    Ok(())
}

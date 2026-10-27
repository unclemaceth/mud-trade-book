//! Mudwater Trade Book: escrowless card-for-card swaps of Metaplex Core assets.

#![forbid(unsafe_code)]

pub mod card;
pub mod checks;
pub mod constants;
pub mod core_io;
pub mod error;
pub mod instructions;
pub mod listing;
pub mod registry;
pub mod release;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("Mudav3fe4qFihgcfCdZAXxNFupTuJR2pUa6Qo7M3CBN");

#[program]
pub mod mud_trade_book {
    use super::*;

    /// One time. The signer must be the program's upgrade authority and becomes the admin.
    /// The Config starts paused, permit-only and exact-asset only.
    pub fn init_config(ctx: Context<InitConfig>) -> Result<()> {
        instructions::init_config::handle_init_config(ctx)
    }

    /// Admin only. Stops new offers and fills; never moves or freezes a card. Cancel and close
    /// keep working.
    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        instructions::set_paused::handle_set_paused(ctx, paused)
    }

    /// Admin only. Step one of an admin transfer.
    pub fn propose_admin(ctx: Context<AdminConfig>, new_admin: Pubkey) -> Result<()> {
        instructions::admin_config::handle_propose_admin(ctx, new_admin)
    }

    /// Admin only. Drops a pending admin transfer.
    pub fn cancel_admin_transfer(ctx: Context<AdminConfig>) -> Result<()> {
        instructions::admin_config::handle_cancel_admin_transfer(ctx)
    }

    /// Signed by the pending admin. Step two of an admin transfer.
    pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
        instructions::admin_config::handle_accept_admin(ctx)
    }

    /// Admin only. Lets `maker` create offers while the Config is not public.
    pub fn grant_maker_permit(ctx: Context<GrantMakerPermit>, maker: Pubkey) -> Result<()> {
        instructions::maker_permit::handle_grant_maker_permit(ctx, maker)
    }

    /// Admin only. Removes `maker`'s permit; their open offers are untouched.
    pub fn revoke_maker_permit(ctx: Context<RevokeMakerPermit>, maker: Pubkey) -> Result<()> {
        instructions::maker_permit::handle_revoke_maker_permit(ctx, maker)
    }

    /// Admin only, one way. Every wallet can create offers from now on.
    pub fn open_public(ctx: Context<AdminConfig>) -> Result<()> {
        instructions::admin_config::handle_open_public(ctx)
    }

    /// Admin only. Allows or refuses `Slot::Cards` wants at create and fill.
    pub fn set_cards_enabled(ctx: Context<AdminConfig>, enabled: bool) -> Result<()> {
        instructions::admin_config::handle_set_cards_enabled(ctx, enabled)
    }

    /// Admin only, once. Creates the trade fee account. Fills fail until it exists. The
    /// recipient account must be a funded system wallet.
    pub fn init_fee_config(ctx: Context<InitFeeConfig>, lamports: u64) -> Result<()> {
        instructions::fee_config::handle_init_fee_config(ctx, lamports)
    }

    /// Admin only. Changes the fee recipient and amount, at most `MAX_FEE_LAMPORTS`. The
    /// recipient account must be a funded system wallet.
    pub fn set_fee(ctx: Context<SetFee>, lamports: u64) -> Result<()> {
        instructions::fee_config::handle_set_fee(ctx, lamports)
    }

    /// Admin only, append only. Publishes a registry snapshot root at the next index.
    pub fn add_card_root(ctx: Context<AddCardRoot>, root: [u8; 32], leaves: u32) -> Result<()> {
        instructions::card_registry::handle_add_card_root(ctx, root, leaves)
    }

    /// Anyone. Fixes a card's identity once, when its URI and a published root agree.
    pub fn register_card(
        ctx: Context<RegisterCard>,
        root_index: u16,
        card_no: u8,
        finish: u8,
        proof: Vec<[u8; 32]>,
    ) -> Result<()> {
        instructions::card_registry::handle_register_card(ctx, root_index, card_no, finish, proof)
    }

    pub fn create_offer<'info>(
        ctx: Context<'info, CreateOffer<'info>>,
        offer_id: u64,
        give: Vec<Pubkey>,
        want: Vec<Slot>,
        expires_at: i64,
    ) -> Result<()> {
        instructions::create_offer::handle_create_offer(ctx, offer_id, give, want, expires_at)
    }

    /// The only instruction that moves a card. The taker pays the trade fee, at most `max_fee`.
    pub fn fill_offer<'info>(ctx: Context<'info, FillOffer<'info>>, max_fee: u64) -> Result<()> {
        instructions::fill_offer::handle_fill_offer(ctx, max_fee)
    }

    /// Maker only. Revokes this offer's delegation on every give card the maker still holds.
    pub fn cancel_offer<'info>(ctx: Context<'info, CancelOffer<'info>>) -> Result<()> {
        instructions::cancel_offer::handle_cancel_offer(ctx)
    }

    /// Anyone, once expired or stale. Revokes like cancel.
    pub fn close_offer<'info>(ctx: Context<'info, CloseOffer<'info>>) -> Result<()> {
        instructions::close_offer::handle_close_offer(ctx)
    }
}

//! Fixed card identity for `Cards` wants. The admin publishes snapshot roots; anyone registers
//! a card once, and only when the snapshot and the card's own URI agree.

use anchor_lang::prelude::*;

use crate::{
    card::parse_card_uri,
    constants::*,
    core_io::read_asset,
    error::TradeError,
    registry::{leaf_hash, verify_proof},
    state::{CardRecord, CardRegistered, CardRoot, CardRootAdded, Config},
};

#[derive(Accounts)]
pub struct AddCardRoot<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ TradeError::NotAdmin)]
    pub config: Account<'info, Config>,
    #[account(
        init,
        payer = admin,
        space = 8 + CardRoot::INIT_SPACE,
        seeds = [ROOT_SEED, &config.root_count.to_le_bytes()],
        bump
    )]
    pub card_root: Account<'info, CardRoot>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(root_index: u16)]
pub struct RegisterCard<'info> {
    /// Pays the record's rent. The record is permanent, so the rent is not returned.
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(seeds = [ROOT_SEED, &root_index.to_le_bytes()], bump = card_root.bump)]
    pub card_root: Account<'info, CardRoot>,
    /// CHECK: owner-checked against Core and decoded in the handler.
    pub asset: UncheckedAccount<'info>,
    #[account(
        init,
        payer = payer,
        space = 8 + CardRecord::INIT_SPACE,
        seeds = [CARD_SEED, asset.key().as_ref()],
        bump
    )]
    pub record: Account<'info, CardRecord>,
    pub system_program: Program<'info, System>,
}

/// Append only: roots are never edited or removed, and a new root only ever adds cards that
/// were not registered before.
pub fn handle_add_card_root(ctx: Context<AddCardRoot>, root: [u8; 32], leaves: u32) -> Result<()> {
    require!(root != [0u8; 32] && leaves > 0, TradeError::BadRoot);
    let config = &mut ctx.accounts.config;
    let index = config.root_count;
    config.root_count = index.checked_add(1).ok_or(TradeError::Overflow)?;
    let card_root = &mut ctx.accounts.card_root;
    card_root.root = root;
    card_root.index = index;
    card_root.leaves = leaves;
    card_root.bump = ctx.bumps.card_root;
    emit!(CardRootAdded {
        index,
        root,
        leaves,
    });
    Ok(())
}

pub fn handle_register_card(
    ctx: Context<RegisterCard>,
    root_index: u16,
    card_no: u8,
    finish: u8,
    proof: Vec<[u8; 32]>,
) -> Result<()> {
    require!(proof.len() <= MAX_PROOF_LEN, TradeError::ProofTooLong);
    require!(
        finish == FINISH_BASE || finish == FINISH_FOIL,
        TradeError::CardMismatch
    );
    let asset_info = ctx.accounts.asset.to_account_info();
    let asset = read_asset(&asset_info)?;
    require!(
        asset.collection == Some(ctx.accounts.config.collection),
        TradeError::WrongCollection
    );
    let from_uri = parse_card_uri(&asset.uri).ok_or(TradeError::UnrecognisedCard)?;
    require!(
        from_uri.card_no == card_no && from_uri.finish.bit() == finish,
        TradeError::CardMismatch
    );
    let asset_key = asset_info.key();
    require!(
        verify_proof(
            leaf_hash(&asset_key, card_no, finish),
            &proof,
            &ctx.accounts.card_root.root
        ),
        TradeError::BadProof
    );
    let record = &mut ctx.accounts.record;
    record.card_no = card_no;
    record.finish = finish;
    record.root_index = root_index;
    record.bump = ctx.bumps.record;
    emit!(CardRegistered {
        asset: asset_key,
        card_no,
        finish,
        root_index,
        payer: ctx.accounts.payer.key(),
    });
    Ok(())
}

use anchor_lang::{prelude::*, system_program};

use crate::{
    checks::{check_give_asset, is_expired},
    constants::*,
    core_io::{core_transfer, read_asset, TransferAccounts},
    error::TradeError,
    listing::close_listed,
    state::{CardRecord, Config, FeeConfig, Offer, OfferFilled, Slot},
};

#[derive(Accounts)]
pub struct FillOffer<'info> {
    #[account(mut)]
    pub taker: Signer<'info>,
    /// CHECK: the taker's permit PDA. It counts only while it exists (program-owned); it need
    /// not exist once the Config is public.
    #[account(seeds = [PERMIT_SEED, taker.key().as_ref()], bump)]
    pub taker_permit: UncheckedAccount<'info>,
    #[account(
        mut,
        seeds = [OFFER_SEED, offer.maker.as_ref(), &offer.offer_id.to_le_bytes()],
        bump = offer.bump,
        has_one = maker @ TradeError::WrongAccount,
        close = maker
    )]
    pub offer: Account<'info, Offer>,
    /// CHECK: equal to `offer.maker` (has_one); receives the taker's cards and all rent.
    #[account(mut)]
    pub maker: UncheckedAccount<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: this offer's delegate PDA, no data; signs the Core transfers of the give cards.
    #[account(seeds = [DELEGATE_SEED, offer.key().as_ref()], bump)]
    pub delegate: UncheckedAccount<'info>,
    #[account(seeds = [FEE_SEED], bump = fee_config.bump)]
    pub fee_config: Account<'info, FeeConfig>,
    /// CHECK: pinned to the configured recipient, which must still be a system wallet (a program
    /// account could not take the transfer); only receives lamports.
    #[account(
        mut,
        address = fee_config.recipient @ TradeError::WrongFeeRecipient,
        owner = system_program::ID @ TradeError::BadFeeRecipient
    )]
    pub fee_recipient: UncheckedAccount<'info>,
    /// CHECK: must be the configured collection, owned by Core.
    #[account(
        address = config.collection @ TradeError::WrongCollection,
        owner = CORE_PROGRAM_ID @ TradeError::WrongCollection
    )]
    pub collection: UncheckedAccount<'info>,
    /// CHECK: must be Metaplex Core.
    #[account(address = CORE_PROGRAM_ID @ TradeError::WrongProgram)]
    pub core_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// Remaining accounts: the give assets (offer order), the taker's assets (slot order), the give
/// cards' `Listed` PDAs (offer order), then one `CardRecord` per `Cards` slot (slot order).
/// The taker pays the configured fee and names the most they accept in `max_fee`, so a fee
/// raised after they built the transaction makes it fail instead of charging more.
pub fn handle_fill_offer<'info>(ctx: Context<'info, FillOffer<'info>>, max_fee: u64) -> Result<()> {
    let config = &ctx.accounts.config;
    require!(!config.paused, TradeError::Paused);
    let fee = ctx.accounts.fee_config.lamports;
    require!(fee <= max_fee, TradeError::FeeAboveMax);
    require!(
        config.public || ctx.accounts.taker_permit.owner == ctx.program_id,
        TradeError::NotPermitted
    );
    let offer = &ctx.accounts.offer;
    require!(
        config.cards_enabled || !offer.want.iter().any(Slot::is_cards),
        TradeError::CardsDisabled
    );
    let now = Clock::get()?.unix_timestamp;
    require!(!is_expired(now, offer.expires_at), TradeError::Expired);
    let taker_key = ctx.accounts.taker.key();
    require_keys_neq!(taker_key, offer.maker, TradeError::SelfFill);

    let n_give = offer.give.len();
    let n_want = offer.want.len();
    let n_cards = offer.want.iter().filter(|s| s.is_cards()).count();
    let expected_len = n_give
        .checked_mul(2)
        .and_then(|v| v.checked_add(n_want))
        .and_then(|v| v.checked_add(n_cards))
        .ok_or(TradeError::Overflow)?;
    require!(
        ctx.remaining_accounts.len() == expected_len,
        TradeError::AccountCount
    );
    let (give_infos, rest) = ctx.remaining_accounts.split_at(n_give);
    let (taker_infos, rest) = rest.split_at(n_want);
    let (listed_infos, record_infos) = rest.split_at(n_give);
    let mut records = record_infos.iter();

    let collection = config.collection;
    let delegate_key = ctx.accounts.delegate.key();

    for (key, info) in offer.give.iter().zip(give_infos) {
        check_give_asset(info, key, &offer.maker, &collection, &delegate_key)?;
    }

    for (j, (slot, info)) in offer.want.iter().zip(taker_infos).enumerate() {
        let key = info.key;
        require!(!offer.give.contains(key), TradeError::DuplicateAsset);
        require!(
            !taker_infos[..j].iter().any(|other| other.key == key),
            TradeError::DuplicateAsset
        );
        let asset = read_asset(info)?;
        require!(
            asset.collection == Some(collection),
            TradeError::WrongCollection
        );
        require_keys_eq!(asset.owner, taker_key, TradeError::NotOwner);
        match slot {
            Slot::Asset(wanted) => require_keys_eq!(*key, *wanted, TradeError::SlotMismatch),
            Slot::Cards { .. } => {
                let record_info = records.next().ok_or(TradeError::AccountCount)?;
                let card = read_card_record(record_info, key, ctx.program_id)?
                    .card_id()
                    .ok_or(TradeError::BadCardRecord)?;
                require!(slot.matches_card(card), TradeError::SlotMismatch);
            }
        }
    }

    let core_program = ctx.accounts.core_program.to_account_info();
    let collection_info = ctx.accounts.collection.to_account_info();
    let taker_info = ctx.accounts.taker.to_account_info();
    let maker_info = ctx.accounts.maker.to_account_info();
    let delegate_info = ctx.accounts.delegate.to_account_info();
    let system_info = ctx.accounts.system_program.to_account_info();
    let offer_key = ctx.accounts.offer.key();
    let delegate_bump = [ctx.bumps.delegate];
    let delegate_seeds: &[&[u8]] = &[DELEGATE_SEED, offer_key.as_ref(), &delegate_bump];

    if fee > 0 {
        system_program::transfer(
            CpiContext::new(
                system_program::ID,
                system_program::Transfer {
                    from: taker_info.clone(),
                    to: ctx.accounts.fee_recipient.to_account_info(),
                },
            ),
            fee,
        )?;
    }

    for info in give_infos {
        core_transfer(
            TransferAccounts {
                core_program: &core_program,
                asset: info,
                collection: &collection_info,
                payer: &taker_info,
                authority: &delegate_info,
                new_owner: &taker_info,
                system_program: &system_info,
            },
            &[delegate_seeds],
        )?;
    }
    for info in taker_infos {
        core_transfer(
            TransferAccounts {
                core_program: &core_program,
                asset: info,
                collection: &collection_info,
                payer: &taker_info,
                authority: &taker_info,
                new_owner: &maker_info,
                system_program: &system_info,
            },
            &[],
        )?;
    }

    for (key, info) in offer.give.iter().zip(listed_infos) {
        close_listed(info, key, &offer_key, &maker_info, ctx.program_id)?;
    }

    emit!(OfferFilled {
        offer: offer_key,
        maker: offer.maker,
        taker: taker_key,
        give: offer.give.clone(),
        got: taker_infos.iter().map(|i| *i.key).collect(),
        fee,
    });
    Ok(())
}

/// The asset's own `CardRecord`: program-owned, the right type, and at `["card", asset]`.
fn read_card_record(info: &AccountInfo, asset: &Pubkey, program_id: &Pubkey) -> Result<CardRecord> {
    require_keys_eq!(*info.owner, *program_id, TradeError::BadCardRecord);
    let record = {
        let data = info.try_borrow_data()?;
        CardRecord::try_deserialize(&mut &data[..])
            .map_err(|_| error!(TradeError::BadCardRecord))?
    };
    let expected =
        Pubkey::create_program_address(&[CARD_SEED, asset.as_ref(), &[record.bump]], program_id)
            .map_err(|_| error!(TradeError::BadCardRecord))?;
    require_keys_eq!(*info.key, expected, TradeError::BadCardRecord);
    Ok(record)
}

//! `Listed` PDAs live in remaining accounts (1..=4 per offer), so they are created and closed
//! by hand. Every key is re-derived from its seeds before use.

use anchor_lang::{
    prelude::*,
    system_program::{self, Allocate, Assign, CreateAccount, Transfer},
};

use crate::{constants::LISTED_SEED, error::TradeError, state::Listed};

pub fn listed_address(asset: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[LISTED_SEED, asset.as_ref()], program_id)
}

/// Creates the `Listed` PDA for `asset`, rent paid by `maker`. Fails with `AlreadyListed` if
/// the card is in a live offer. A PDA that someone pre-funded with lamports is still claimed.
pub fn create_listed<'info>(
    listed: &AccountInfo<'info>,
    asset: &Pubkey,
    offer: Pubkey,
    maker: &AccountInfo<'info>,
    program_id: &Pubkey,
) -> Result<()> {
    let (expected, bump) = listed_address(asset, program_id);
    require_keys_eq!(*listed.key, expected, TradeError::WrongAccount);
    require!(listed.owner != program_id, TradeError::AlreadyListed);
    require_keys_eq!(*listed.owner, system_program::ID, TradeError::WrongAccount);
    require!(listed.data_is_empty(), TradeError::WrongAccount);

    let space = 8 + Listed::INIT_SPACE;
    let rent = Rent::get()?.minimum_balance(space);
    let bump_seed = [bump];
    let seeds: &[&[u8]] = &[LISTED_SEED, asset.as_ref(), &bump_seed];
    let signer: &[&[&[u8]]] = &[seeds];

    let current = listed.lamports();
    if current == 0 {
        system_program::create_account(
            CpiContext::new_with_signer(
                system_program::ID,
                CreateAccount {
                    from: maker.clone(),
                    to: listed.clone(),
                },
                signer,
            ),
            rent,
            space as u64,
            program_id,
        )?;
    } else {
        let top_up = rent.saturating_sub(current);
        if top_up > 0 {
            system_program::transfer(
                CpiContext::new(
                    system_program::ID,
                    Transfer {
                        from: maker.clone(),
                        to: listed.clone(),
                    },
                ),
                top_up,
            )?;
        }
        system_program::allocate(
            CpiContext::new_with_signer(
                system_program::ID,
                Allocate {
                    account_to_allocate: listed.clone(),
                },
                signer,
            ),
            space as u64,
        )?;
        system_program::assign(
            CpiContext::new_with_signer(
                system_program::ID,
                Assign {
                    account_to_assign: listed.clone(),
                },
                signer,
            ),
            program_id,
        )?;
    }

    let mut data = listed.try_borrow_mut_data()?;
    Listed { offer, bump }.try_serialize(&mut &mut data[..])?;
    Ok(())
}

/// Checks the `Listed` PDA for `asset` belongs to `offer`, then closes it, rent to `maker`.
pub fn close_listed<'info>(
    listed: &AccountInfo<'info>,
    asset: &Pubkey,
    offer: &Pubkey,
    maker: &AccountInfo<'info>,
    program_id: &Pubkey,
) -> Result<()> {
    let (expected, _) = listed_address(asset, program_id);
    require_keys_eq!(*listed.key, expected, TradeError::BadListed);
    require_keys_eq!(*listed.owner, *program_id, TradeError::BadListed);
    {
        let data = listed.try_borrow_data()?;
        let state =
            Listed::try_deserialize(&mut &data[..]).map_err(|_| error!(TradeError::BadListed))?;
        require_keys_eq!(state.offer, *offer, TradeError::BadListed);
    }
    close_account(listed, maker)
}

/// Same steps as Anchor's `close` constraint: lamports out, owner back to System, data cleared.
pub fn close_account<'info>(
    info: &AccountInfo<'info>,
    destination: &AccountInfo<'info>,
) -> Result<()> {
    destination.add_lamports(info.lamports())?;
    **info.try_borrow_mut_lamports()? = 0;
    info.assign(&system_program::ID);
    info.resize(0).map_err(Into::into)
}

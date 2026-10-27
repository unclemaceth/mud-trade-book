//! Reading Metaplex Core assets, moving them and revoking our delegation. The only Core CPI
//! builders in the program.

use anchor_lang::prelude::*;
use mpl_core::{
    accounts::{BaseAssetV1, BaseCollectionV1, PluginHeaderV1},
    instructions::{
        RevokePluginAuthorityV1Cpi, RevokePluginAuthorityV1CpiAccounts,
        RevokePluginAuthorityV1InstructionArgs, TransferV1Cpi, TransferV1CpiAccounts,
        TransferV1InstructionArgs,
    },
    types::{Key, PluginAuthority, PluginType, UpdateAuthority},
    PluginRegistryV1Safe,
};

use crate::{constants::CORE_PROGRAM_ID, error::TradeError};

/// What the Trade Book needs from a Core asset account.
pub struct CoreAsset {
    pub owner: Pubkey,
    /// `Some` only when the update authority is `Collection(..)`.
    pub collection: Option<Pubkey>,
    pub uri: String,
    /// The TransferDelegate plugin's authority, if the plugin is present.
    pub transfer_delegate: Option<PluginAuthority>,
}

impl CoreAsset {
    pub fn delegate_is(&self, delegate: &Pubkey) -> bool {
        matches!(
            &self.transfer_delegate,
            Some(PluginAuthority::Address { address }) if address == delegate
        )
    }
}

/// Owner-checks the account against Core, then decodes it.
pub fn read_asset(info: &AccountInfo) -> Result<CoreAsset> {
    require_keys_eq!(*info.owner, CORE_PROGRAM_ID, TradeError::NotCoreAsset);
    let data = info.try_borrow_data()?;
    // Checked before decoding: another Core account type read as an asset can carry a string
    // length that exhausts the 32 KiB SBF heap, which panics instead of returning an error.
    require!(
        data.first() == Some(&(Key::AssetV1 as u8)),
        TradeError::NotCoreAsset
    );
    let mut rest: &[u8] = &data;
    let base = BaseAssetV1::deserialize(&mut rest).map_err(|_| error!(TradeError::NotCoreAsset))?;
    require!(base.key == Key::AssetV1, TradeError::NotCoreAsset);
    let collection = match base.update_authority {
        UpdateAuthority::Collection(c) => Some(c),
        _ => None,
    };
    let transfer_delegate = if rest.is_empty() {
        None
    } else {
        find_plugin_authority(&data, rest, PluginType::TransferDelegate)?
    };
    Ok(CoreAsset {
        owner: base.owner,
        collection,
        uri: base.uri,
        transfer_delegate,
    })
}

fn find_plugin_authority(
    data: &[u8],
    after_base: &[u8],
    plugin: PluginType,
) -> Result<Option<PluginAuthority>> {
    let header =
        PluginHeaderV1::from_bytes(after_base).map_err(|_| error!(TradeError::NotCoreAsset))?;
    require!(header.key == Key::PluginHeaderV1, TradeError::NotCoreAsset);
    let offset = usize::try_from(header.plugin_registry_offset)
        .map_err(|_| error!(TradeError::NotCoreAsset))?;
    let registry_bytes = data.get(offset..).ok_or(error!(TradeError::NotCoreAsset))?;
    let registry = PluginRegistryV1Safe::from_bytes(registry_bytes)
        .map_err(|_| error!(TradeError::NotCoreAsset))?;
    let wanted = plugin as u8;
    Ok(registry
        .registry
        .into_iter()
        .find(|r| r.plugin_type == wanted)
        .map(|r| r.authority))
}

/// Owner-checks a collection account against Core and confirms it decodes as a collection.
pub fn check_collection(info: &AccountInfo) -> Result<()> {
    require_keys_eq!(*info.owner, CORE_PROGRAM_ID, TradeError::WrongCollection);
    let data = info.try_borrow_data()?;
    // Same heap hazard as `read_asset`: an asset read as a collection panics on its URI length.
    require!(
        data.first() == Some(&(Key::CollectionV1 as u8)),
        TradeError::WrongCollection
    );
    let mut rest: &[u8] = &data;
    let base = BaseCollectionV1::deserialize(&mut rest)
        .map_err(|_| error!(TradeError::WrongCollection))?;
    require!(base.key == Key::CollectionV1, TradeError::WrongCollection);
    Ok(())
}

pub struct TransferAccounts<'a, 'b> {
    pub core_program: &'b AccountInfo<'a>,
    pub asset: &'b AccountInfo<'a>,
    pub collection: &'b AccountInfo<'a>,
    pub payer: &'b AccountInfo<'a>,
    pub authority: &'b AccountInfo<'a>,
    pub new_owner: &'b AccountInfo<'a>,
    pub system_program: &'b AccountInfo<'a>,
}

/// Core `TransferV1`. Called only from `fill_offer`.
pub fn core_transfer(accounts: TransferAccounts<'_, '_>, signer_seeds: &[&[&[u8]]]) -> Result<()> {
    require_keys_eq!(
        *accounts.core_program.key,
        CORE_PROGRAM_ID,
        TradeError::WrongProgram
    );
    TransferV1Cpi::new(
        accounts.core_program,
        TransferV1CpiAccounts {
            asset: accounts.asset,
            collection: Some(accounts.collection),
            payer: accounts.payer,
            authority: Some(accounts.authority),
            new_owner: accounts.new_owner,
            system_program: Some(accounts.system_program),
            log_wrapper: None,
        },
        TransferV1InstructionArgs {
            compression_proof: None,
        },
    )
    .invoke_signed(signer_seeds)?;
    Ok(())
}

pub struct RevokeAccounts<'a, 'b> {
    pub core_program: &'b AccountInfo<'a>,
    pub asset: &'b AccountInfo<'a>,
    pub collection: &'b AccountInfo<'a>,
    pub payer: &'b AccountInfo<'a>,
    pub delegate: &'b AccountInfo<'a>,
    pub system_program: &'b AccountInfo<'a>,
}

/// Core `RevokePluginAuthorityV1` on the TransferDelegate, signed by the delegate PDA itself.
/// The plugin's authority returns to the owner. Core moves no lamports for this (the account
/// shrinks and keeps its balance), so the payer only signs. Called from cancel and close.
pub fn core_revoke_transfer_delegate(
    accounts: RevokeAccounts<'_, '_>,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    require_keys_eq!(
        *accounts.core_program.key,
        CORE_PROGRAM_ID,
        TradeError::WrongProgram
    );
    RevokePluginAuthorityV1Cpi::new(
        accounts.core_program,
        RevokePluginAuthorityV1CpiAccounts {
            asset: accounts.asset,
            collection: Some(accounts.collection),
            payer: accounts.payer,
            authority: Some(accounts.delegate),
            system_program: accounts.system_program,
            log_wrapper: None,
        },
        RevokePluginAuthorityV1InstructionArgs {
            plugin_type: PluginType::TransferDelegate,
        },
    )
    .invoke_signed(signer_seeds)?;
    Ok(())
}

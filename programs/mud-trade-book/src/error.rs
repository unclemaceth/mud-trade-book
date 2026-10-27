use anchor_lang::prelude::*;

#[error_code]
pub enum TradeError {
    #[msg("Trading is paused")]
    Paused,
    #[msg("The offer has expired")]
    Expired,
    #[msg("The offer is live and valid; only the maker can cancel it")]
    NotExpiredOrStale,
    #[msg("The asset is not in the configured collection")]
    WrongCollection,
    #[msg("The asset is not owned by the expected wallet")]
    NotOwner,
    #[msg("The asset's transfer delegate is not the Trade Book")]
    DelegateMissing,
    #[msg("The same asset appears twice")]
    DuplicateAsset,
    #[msg("This card is already in an offer")]
    AlreadyListed,
    #[msg("A wanted slot is malformed")]
    BadWant,
    #[msg("A taker card does not match its wanted slot")]
    SlotMismatch,
    #[msg("The card URI is not in the Mudwater scheme")]
    UnrecognisedCard,
    #[msg("The maker cannot fill their own offer")]
    SelfFill,
    #[msg("Too many cards or slots (at most 4 each)")]
    TooMany,
    #[msg("Expiry must be in the future and at most 30 days out")]
    BadExpiry,
    #[msg("An offer must give at least one card")]
    EmptyGive,
    #[msg("The account is not a Metaplex Core asset")]
    NotCoreAsset,
    #[msg("An account is not the one the offer expects")]
    WrongAccount,
    #[msg("Wrong number of remaining accounts")]
    AccountCount,
    #[msg("The Core program account is wrong")]
    WrongProgram,
    #[msg("Only the admin can do this")]
    NotAdmin,
    #[msg("Only the maker can do this")]
    NotMaker,
    #[msg("A listing account does not belong to this offer")]
    BadListed,
    #[msg("Only the program's upgrade authority can initialise the config")]
    NotUpgradeAuthority,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("This wallet has no maker permit while the beta is closed")]
    NotPermitted,
    #[msg("Card-set wants are disabled; ask for exact cards")]
    CardsDisabled,
    #[msg("The Trade Book is already open to every maker")]
    AlreadyPublic,
    #[msg("The new admin must be a real key other than the current admin")]
    BadNewAdmin,
    #[msg("Only the pending admin can accept")]
    NotPendingAdmin,
    #[msg("No admin transfer is pending")]
    NoPendingAdmin,
    #[msg("A registry root must be non-zero and cover at least one card")]
    BadRoot,
    #[msg("The registry proof does not match the root")]
    BadProof,
    #[msg("The registry proof is too long")]
    ProofTooLong,
    #[msg("The card's URI does not match the registry entry")]
    CardMismatch,
    #[msg("A card record is missing or is not this card's")]
    BadCardRecord,
}

use anchor_lang::prelude::*;

#[constant]
pub const CONFIG_SEED: &[u8] = b"config";

#[constant]
pub const OFFER_SEED: &[u8] = b"offer";

#[constant]
pub const LISTED_SEED: &[u8] = b"listed";

/// `["delegate", offer]`: each offer has its own transfer delegate, so an approval granted for
/// one offer can never fill another.
#[constant]
pub const DELEGATE_SEED: &[u8] = b"delegate";

/// `["maker", maker]`: the maker's offer counter. Offer ids run 1, 2, 3, ... per maker and are
/// never reused, so an offer address (and its delegate) names one offer for ever.
#[constant]
pub const MAKER_SEED: &[u8] = b"maker";

#[constant]
pub const FEE_SEED: &[u8] = b"fee";

/// Highest trade fee the admin can set: 0.01 SOL.
#[constant]
pub const MAX_FEE_LAMPORTS: u64 = 10_000_000;

#[constant]
pub const PERMIT_SEED: &[u8] = b"permit";

#[constant]
pub const ROOT_SEED: &[u8] = b"root";

#[constant]
pub const CARD_SEED: &[u8] = b"card";

/// Deepest registry proof accepted: 2^24 leaves, far above the collection's size.
pub const MAX_PROOF_LEN: usize = 24;

/// Most cards a maker can give in one offer.
pub const MAX_GIVE: usize = 4;

/// Most slots a maker can ask for in one offer.
pub const MAX_WANT: usize = 4;

/// Latest expiry allowed at creation: 30 days out.
#[constant]
pub const MAX_EXPIRY_SECS: i64 = 30 * 24 * 60 * 60;

/// Metaplex Core. Every asset account and every transfer CPI is checked against it.
pub const CORE_PROGRAM_ID: Pubkey = mpl_core::ID;

/// The exact prefix every Mudwater card URI carries on chain.
pub const CARD_URI_PREFIX: &[u8] = b"https://assets.mudwater.bayc.com/m/c/";

/// Card numbers run 1..=120. The URI index runs 0..=359 in three blocks of 120: Base, Foil,
/// Serialized.
pub const CARD_COUNT: u64 = 120;
pub const FINISH_COUNT: u64 = 3;
pub const MAX_CARD_INDEX: u64 = FINISH_COUNT * CARD_COUNT - 1;

/// `Slot::Cards.finish_mask` bits.
pub const FINISH_BASE: u8 = 0b001;
pub const FINISH_FOIL: u8 = 0b010;
pub const FINISH_SERIALIZED: u8 = 0b100;
pub const FINISH_ALL: u8 = FINISH_BASE | FINISH_FOIL | FINISH_SERIALIZED;

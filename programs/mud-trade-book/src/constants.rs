use anchor_lang::prelude::*;

#[constant]
pub const CONFIG_SEED: &[u8] = b"config";

#[constant]
pub const OFFER_SEED: &[u8] = b"offer";

#[constant]
pub const LISTED_SEED: &[u8] = b"listed";

#[constant]
pub const DELEGATE_SEED: &[u8] = b"delegate";

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

/// Card numbers run 1..=120; the URI index runs 0..=239 (120..=239 are foils).
pub const CARD_COUNT: u64 = 120;
pub const MAX_CARD_INDEX: u64 = 2 * CARD_COUNT - 1;

/// `Slot::Cards.finish_mask` bits.
pub const FINISH_BASE: u8 = 0b01;
pub const FINISH_FOIL: u8 = 0b10;
pub const FINISH_ALL: u8 = FINISH_BASE | FINISH_FOIL;

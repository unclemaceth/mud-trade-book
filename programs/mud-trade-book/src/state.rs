use anchor_lang::prelude::*;

use crate::{
    card::{card_in_set, CardId, Finish},
    constants::*,
};

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    pub paused: bool,
    pub collection: Pubkey,
    pub bump: u8,
    /// Set by `propose_admin`; becomes `admin` when it signs `accept_admin`.
    pub pending_admin: Option<Pubkey>,
    /// One way. While false, only wallets with a `MakerPermit` can create offers.
    pub public: bool,
    /// While false, `Slot::Cards` is refused at create and fill. Card identity comes from a URI
    /// the collection update authority can rewrite, so set wants stay off until that is solved.
    pub cards_enabled: bool,
    /// Number of `CardRoot`s added; the next root goes at this index.
    pub root_count: u16,
}

/// One registry snapshot: a Merkle root over `(asset, card_no, finish)` leaves. Never changed
/// once added.
#[account]
#[derive(InitSpace)]
pub struct CardRoot {
    pub root: [u8; 32],
    pub index: u16,
    pub leaves: u32,
    pub bump: u8,
}

/// A card's identity, fixed the first time it is registered. `Cards` fills read this, never
/// the URI, so a later URI change cannot alter what the card counts as.
#[account]
#[derive(InitSpace)]
pub struct CardRecord {
    pub card_no: u8,
    /// `FINISH_BASE`, `FINISH_FOIL` or `FINISH_SERIALIZED`.
    pub finish: u8,
    pub root_index: u16,
    pub bump: u8,
}

impl CardRecord {
    /// `None` if `finish` is not exactly one finish bit.
    pub fn card_id(&self) -> Option<CardId> {
        Some(CardId {
            card_no: self.card_no,
            finish: Finish::from_bit(self.finish)?,
        })
    }
}

/// The trade fee the taker pays on every fill. Admin-set, at most `MAX_FEE_LAMPORTS`.
#[account]
#[derive(InitSpace)]
pub struct FeeConfig {
    pub recipient: Pubkey,
    pub lamports: u64,
    pub bump: u8,
}

/// A maker's offer counter, created by their first offer and never closed. The next offer must
/// use `last_offer_id + 1`.
#[account]
#[derive(InitSpace)]
pub struct MakerState {
    pub last_offer_id: u64,
    pub bump: u8,
}

/// Admin-granted beta admission for one maker. Its existence is the permit.
#[account]
#[derive(InitSpace)]
pub struct MakerPermit {
    pub maker: Pubkey,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Offer {
    pub maker: Pubkey,
    pub offer_id: u64,
    #[max_len(4)]
    pub give: Vec<Pubkey>,
    #[max_len(4)]
    pub want: Vec<Slot>,
    pub expires_at: i64,
    pub created_slot: u64,
    pub bump: u8,
}

/// One per give card: a card is promised to at most one offer at a time.
#[account]
#[derive(InitSpace)]
pub struct Listed {
    pub offer: Pubkey,
    pub bump: u8,
}

/// One wanted card. Each slot takes exactly one distinct taker asset.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum Slot {
    /// Exactly this asset.
    Asset(Pubkey),
    /// Any asset whose card number is in `cards` (bit n-1 = card n) and whose finish is in
    /// `finish_mask` (bit0 Base, bit1 Foil, bit2 Serialized).
    Cards { cards: [u8; 15], finish_mask: u8 },
}

impl Slot {
    pub fn is_well_formed(&self) -> bool {
        match self {
            Slot::Asset(_) => true,
            Slot::Cards { cards, finish_mask } => {
                cards.iter().any(|b| *b != 0) && *finish_mask != 0 && finish_mask & !FINISH_ALL == 0
            }
        }
    }

    pub fn is_cards(&self) -> bool {
        matches!(self, Slot::Cards { .. })
    }

    /// Cards slots match on the parsed card; Asset slots never match through this path.
    pub fn matches_card(&self, card: CardId) -> bool {
        match self {
            Slot::Asset(_) => false,
            Slot::Cards { cards, finish_mask } => {
                card_in_set(cards, card.card_no) && finish_mask & card.finish.bit() != 0
            }
        }
    }
}

#[event]
pub struct OfferCreated {
    pub offer: Pubkey,
    pub maker: Pubkey,
    pub offer_id: u64,
    pub give: Vec<Pubkey>,
    pub want: Vec<Slot>,
    pub expires_at: i64,
}

#[event]
pub struct OfferFilled {
    pub offer: Pubkey,
    pub maker: Pubkey,
    pub taker: Pubkey,
    pub give: Vec<Pubkey>,
    pub got: Vec<Pubkey>,
    /// Lamports the taker paid to the fee recipient.
    pub fee: u64,
}

#[event]
pub struct OfferCancelled {
    pub offer: Pubkey,
    pub maker: Pubkey,
    /// Give cards whose TransferDelegate was revoked here.
    pub revoked: Vec<Pubkey>,
}

#[event]
pub struct OfferClosed {
    pub offer: Pubkey,
    pub maker: Pubkey,
    pub closer: Pubkey,
    /// Give cards whose TransferDelegate was revoked here.
    pub revoked: Vec<Pubkey>,
}

#[event]
pub struct PausedChanged {
    pub paused: bool,
}

#[event]
pub struct PendingAdminChanged {
    pub admin: Pubkey,
    pub pending_admin: Option<Pubkey>,
}

#[event]
pub struct AdminAccepted {
    pub old_admin: Pubkey,
    pub admin: Pubkey,
}

#[event]
pub struct MakerPermitChanged {
    pub maker: Pubkey,
    pub permitted: bool,
}

#[event]
pub struct OpenedPublic {}

#[event]
pub struct CardsEnabledChanged {
    pub enabled: bool,
}

#[event]
pub struct FeeChanged {
    pub recipient: Pubkey,
    pub lamports: u64,
}

#[event]
pub struct CardRootAdded {
    pub index: u16,
    pub root: [u8; 32],
    pub leaves: u32,
}

#[event]
pub struct CardRegistered {
    pub asset: Pubkey,
    pub card_no: u8,
    pub finish: u8,
    pub root_index: u16,
    pub payer: Pubkey,
}

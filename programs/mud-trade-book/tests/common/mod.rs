//! LiteSVM harness: the real Metaplex Core binary (dumped from mainnet) plus the built
//! `mud_trade_book.so`, a Core collection with the Mudwater royalty shape, and helpers.
#![allow(dead_code)]

use anchor_lang::{
    prelude::Pubkey,
    solana_program::{
        instruction::{AccountMeta, Instruction},
        system_program,
    },
    AccountDeserialize, InstructionData, ToAccountMetas,
};
use litesvm::{types::TransactionResult, LiteSVM};
use mpl_core::{
    instructions::{
        AddPluginV1Builder, ApprovePluginAuthorityV1Builder, CreateCollectionV1Builder,
        CreateV1Builder, RevokePluginAuthorityV1Builder, TransferV1Builder,
    },
    types::{
        Creator, DataState, FreezeDelegate, Plugin, PluginAuthority, PluginAuthorityPair,
        PluginType, Royalties, RuleSet, TransferDelegate,
    },
};
use mud_trade_book::{error::TradeError, state::Slot};
use solana_clock::Clock;
use solana_instruction::error::InstructionError;
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_error::TransactionError;

pub const CORE_ID: Pubkey = mpl_core::ID;
pub const START_TS: i64 = 1_800_000_000;
pub const DAY: i64 = 24 * 60 * 60;
pub const SOL: u64 = 1_000_000_000;

pub fn uri(idx: u32, serial: u32) -> String {
    format!("https://assets.mudwater.bayc.com/m/c/{idx}-{serial}")
}

/// Bitset with the given card numbers (1..=120).
pub fn cards(nums: &[u8]) -> [u8; 15] {
    let mut set = [0u8; 15];
    for &n in nums {
        let i = usize::from(n - 1);
        set[i / 8] |= 1 << (i % 8);
    }
    set
}

pub fn cards_slot(nums: &[u8], finish_mask: u8) -> Slot {
    Slot::Cards {
        cards: cards(nums),
        finish_mask,
    }
}

pub fn err_code(e: TradeError) -> u32 {
    anchor_lang::error::ERROR_CODE_OFFSET + e as u32
}

/// The custom error code of a failed transaction, with the failing instruction index.
pub fn custom_error(res: &TransactionResult) -> Option<(u8, u32)> {
    match res {
        Err(f) => match &f.err {
            TransactionError::InstructionError(ix, InstructionError::Custom(code)) => {
                Some((*ix, *code))
            }
            _ => None,
        },
        Ok(_) => None,
    }
}

#[track_caller]
pub fn assert_trade_err(res: &TransactionResult, expected: TradeError) {
    let got = custom_error(res);
    let want = err_code(expected);
    assert!(
        matches!(got, Some((_, c)) if c == want),
        "expected {expected:?} ({want}), got {got:?}; logs: {:#?}",
        res.as_ref().err().map(|f| &f.meta.logs)
    );
}

pub struct Env {
    pub svm: LiteSVM,
    pub program_id: Pubkey,
    pub admin: Keypair,
    pub maker: Keypair,
    pub taker: Keypair,
    pub stranger: Keypair,
    pub collection: Pubkey,
    /// The v1 shared delegate `["delegate"]`. v2 never signs as it; kept to prove that.
    pub legacy_delegate: Pubkey,
    pub config: Pubkey,
    pub fee: Pubkey,
    pub fee_recipient: Pubkey,
}

pub struct Sent {
    pub res: TransactionResult,
    pub size: usize,
}

impl Env {
    /// `raw`, then unpaused, open to every maker and with card-set wants on, so the swap tests
    /// exercise the full feature set.
    pub fn new(rule_set: RuleSet) -> Self {
        let mut env = Self::raw(rule_set);
        let admin = env.admin.insecure_clone();
        for res in [
            env.set_paused(&admin, false).res,
            env.open_public(&admin).res,
            env.set_cards_enabled(&admin, true).res,
        ] {
            assert!(res.is_ok(), "open config: {res:?}");
        }
        env
    }

    /// Collection with Royalties 500 bps and the given rule set; config initialised on it and
    /// left exactly as `init_config` writes it (paused, permit-only, exact wants only).
    pub fn raw(rule_set: RuleSet) -> Self {
        let mut env = Self::bare(rule_set);
        let admin = env.admin.insecure_clone();
        let res = env.init_config(&admin, env.collection).res;
        assert!(res.is_ok(), "init_config: {res:?}");
        let recipient = env.fee_recipient;
        let res = env.init_fee_config(&admin, recipient, 0).res;
        assert!(res.is_ok(), "init_fee_config: {res:?}");
        env
    }

    /// Programs loaded, wallets funded, collection created; no config yet.
    pub fn bare(rule_set: RuleSet) -> Self {
        let program_id = mud_trade_book::id();
        let mut svm = LiteSVM::new();
        svm.add_program(
            CORE_ID,
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/mpl_core.so"
            )),
        )
        .expect("load core");
        svm.add_program(
            program_id,
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/deploy/mud_trade_book.so"
            )),
        )
        .expect("load trade book");

        let mut clock: Clock = svm.get_sysvar();
        clock.unix_timestamp = START_TS;
        svm.set_sysvar(&clock);

        let admin = Keypair::new();
        let maker = Keypair::new();
        let taker = Keypair::new();
        let stranger = Keypair::new();
        for k in [&admin, &maker, &taker, &stranger] {
            svm.airdrop(&k.pubkey(), 100 * SOL).expect("airdrop");
        }
        // A funded system wallet, as `init_fee_config` requires.
        let fee_recipient = Keypair::new().pubkey();
        svm.airdrop(&fee_recipient, SOL).expect("airdrop");
        set_upgrade_authority(&mut svm, &program_id, &admin.pubkey());

        let legacy_delegate = Pubkey::find_program_address(&[b"delegate"], &program_id).0;
        let config = Pubkey::find_program_address(&[b"config"], &program_id).0;
        let fee = Pubkey::find_program_address(&[b"fee"], &program_id).0;
        let mut env = Env {
            svm,
            program_id,
            admin,
            maker,
            taker,
            stranger,
            collection: Pubkey::default(),
            legacy_delegate,
            config,
            fee,
            fee_recipient,
        };
        env.collection = env.create_collection(rule_set);
        env
    }

    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    pub fn warp_to(&mut self, ts: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp = ts;
        clock.slot += 10;
        self.svm.set_sysvar(&clock);
    }

    pub fn send(&mut self, ixs: &[Instruction], payer: &Keypair, signers: &[&Keypair]) -> Sent {
        let blockhash = self.svm.latest_blockhash();
        let msg = Message::new_with_blockhash(ixs, Some(&payer.pubkey()), &blockhash);
        let mut all: Vec<&Keypair> = vec![payer];
        for s in signers {
            if s.pubkey() != payer.pubkey() {
                all.push(s);
            }
        }
        let vmsg = VersionedMessage::Legacy(msg);
        let tx = VersionedTransaction::try_new(vmsg.clone(), &all).expect("sign");
        let size = 1 + 64 * tx.signatures.len() + vmsg.serialize().len();
        let res = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        Sent { res, size }
    }

    pub fn lamports(&self, key: &Pubkey) -> u64 {
        self.svm.get_account(key).map(|a| a.lamports).unwrap_or(0)
    }

    pub fn exists(&self, key: &Pubkey) -> bool {
        self.svm.get_account(key).is_some_and(|a| a.lamports > 0)
    }

    // ---------- Core helpers ----------

    pub fn create_collection(&mut self, rule_set: RuleSet) -> Pubkey {
        let col = Keypair::new();
        let admin = self.admin.insecure_clone();
        let ix = CreateCollectionV1Builder::new()
            .collection(col.pubkey())
            .update_authority(Some(admin.pubkey()))
            .payer(admin.pubkey())
            .name("Mudwater Cards".into())
            .uri("https://assets.mudwater.bayc.com/m/cards/collection/v1.json".into())
            .plugins(vec![PluginAuthorityPair {
                plugin: Plugin::Royalties(Royalties {
                    basis_points: 500,
                    creators: vec![Creator {
                        address: admin.pubkey(),
                        percentage: 100,
                    }],
                    rule_set,
                }),
                authority: None,
            }])
            .instruction();
        let res = self.send(&[ix], &admin, &[&col]).res;
        assert!(res.is_ok(), "create collection: {res:?}");
        col.pubkey()
    }

    pub fn mint_in(&mut self, collection: Pubkey, owner: &Pubkey, uri_str: &str) -> Pubkey {
        let asset = Keypair::new();
        let admin = self.admin.insecure_clone();
        let ix = CreateV1Builder::new()
            .asset(asset.pubkey())
            .collection(Some(collection))
            .authority(Some(admin.pubkey()))
            .payer(admin.pubkey())
            .owner(Some(*owner))
            .data_state(DataState::AccountState)
            .name("Mudwater Card".into())
            .uri(uri_str.into())
            .instruction();
        let res = self.send(&[ix], &admin, &[&asset]).res;
        assert!(res.is_ok(), "mint: {res:?}");
        asset.pubkey()
    }

    pub fn mint(&mut self, owner: &Pubkey, idx: u32, serial: u32) -> Pubkey {
        let col = self.collection;
        self.mint_in(col, owner, &uri(idx, serial))
    }

    /// The transfer delegate of one offer: `["delegate", offer]`.
    pub fn delegate_of(&self, offer: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[b"delegate", offer.as_ref()], &self.program_id).0
    }

    /// Points the card's TransferDelegate at `delegate`. A card that was ever transferred keeps
    /// the plugin with its authority reset to the owner, so it needs Approve, not Add (Core 0xF).
    pub fn approve_ix(&self, asset: Pubkey, owner: &Pubkey, delegate: Pubkey) -> Instruction {
        let collection = self.read(&asset).collection;
        if self.read(&asset).transfer_delegate.is_some() {
            return ApprovePluginAuthorityV1Builder::new()
                .asset(asset)
                .collection(collection)
                .payer(*owner)
                .authority(Some(*owner))
                .plugin_type(PluginType::TransferDelegate)
                .new_authority(PluginAuthority::Address { address: delegate })
                .instruction();
        }
        AddPluginV1Builder::new()
            .asset(asset)
            .collection(collection)
            .payer(*owner)
            .authority(Some(*owner))
            .plugin(Plugin::TransferDelegate(TransferDelegate {}))
            .init_authority(PluginAuthority::Address { address: delegate })
            .instruction()
    }

    /// Approves the delegate of `offer`, the PDA at `[maker, offer_id]`.
    pub fn delegate_ix(&self, asset: Pubkey, owner: &Pubkey, offer: &Pubkey) -> Instruction {
        self.approve_ix(asset, owner, self.delegate_of(offer))
    }

    pub fn add_delegate(&mut self, asset: Pubkey, owner: &Keypair, offer: &Pubkey) {
        let ix = self.delegate_ix(asset, &owner.pubkey(), offer);
        let res = self.send(&[ix], owner, &[]).res;
        assert!(res.is_ok(), "add delegate: {res:?}");
    }

    /// True if the card is delegated to `offer`'s delegate.
    pub fn delegated_to(&self, asset: &Pubkey, offer: &Pubkey) -> bool {
        self.read(asset).delegate_is(&self.delegate_of(offer))
    }

    pub fn revoke_ix(&self, asset: Pubkey, owner: &Pubkey) -> Instruction {
        RevokePluginAuthorityV1Builder::new()
            .asset(asset)
            .collection(self.read(&asset).collection)
            .payer(*owner)
            .authority(Some(*owner))
            .plugin_type(PluginType::TransferDelegate)
            .instruction()
    }

    /// Core refuses to re-approve a delegated plugin (CannotRedelegate, 0x1B), so moving a
    /// delegation from one address to another is a revoke then an approve.
    pub fn redelegate_ixs(
        &self,
        asset: Pubkey,
        owner: &Pubkey,
        delegate: Pubkey,
    ) -> Vec<Instruction> {
        vec![
            self.revoke_ix(asset, owner),
            self.approve_ix(asset, owner, delegate),
        ]
    }

    pub fn revoke_delegate(&mut self, asset: Pubkey, owner: &Keypair) {
        let ix = self.revoke_ix(asset, &owner.pubkey());
        let res = self.send(&[ix], owner, &[]).res;
        assert!(res.is_ok(), "revoke delegate: {res:?}");
    }

    pub fn core_transfer(&mut self, asset: Pubkey, owner: &Keypair, to: &Pubkey) {
        let ix = TransferV1Builder::new()
            .asset(asset)
            .collection(Some(self.collection))
            .payer(owner.pubkey())
            .authority(Some(owner.pubkey()))
            .new_owner(*to)
            .instruction();
        let res = self.send(&[ix], owner, &[]).res;
        assert!(res.is_ok(), "core transfer: {res:?}");
    }

    /// Owner-managed FreezeDelegate, frozen. Core then refuses to transfer the card.
    pub fn freeze(&mut self, asset: Pubkey, owner: &Keypair) {
        let ix = AddPluginV1Builder::new()
            .asset(asset)
            .collection(Some(self.collection))
            .payer(owner.pubkey())
            .authority(Some(owner.pubkey()))
            .plugin(Plugin::FreezeDelegate(FreezeDelegate { frozen: true }))
            .instruction();
        let res = self.send(&[ix], owner, &[]).res;
        assert!(res.is_ok(), "freeze: {res:?}");
    }

    pub fn owner_of(&self, asset: &Pubkey) -> Pubkey {
        let acc = self.svm.get_account(asset).expect("asset exists");
        let base = mpl_core::accounts::BaseAssetV1::from_bytes(&acc.data).expect("asset decodes");
        base.owner
    }

    // ---------- Trade Book helpers ----------

    pub fn offer_pda(&self, maker: &Pubkey, offer_id: u64) -> Pubkey {
        Pubkey::find_program_address(
            &[b"offer", maker.as_ref(), &offer_id.to_le_bytes()],
            &self.program_id,
        )
        .0
    }

    pub fn maker_state_pda(&self, maker: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[b"maker", maker.as_ref()], &self.program_id).0
    }

    /// Rent of a `MakerState`, which a maker's first offer pays once.
    pub fn maker_state_rent(&self) -> u64 {
        self.svm.minimum_balance_for_rent_exemption(
            8 + <mud_trade_book::state::MakerState as anchor_lang::Space>::INIT_SPACE,
        )
    }

    /// The maker's last offer id, 0 before their first offer.
    pub fn last_offer_id(&self, maker: &Pubkey) -> u64 {
        self.svm
            .get_account(&self.maker_state_pda(maker))
            .and_then(|a| mud_trade_book::state::MakerState::try_deserialize(&mut &a.data[..]).ok())
            .map_or(0, |m| m.last_offer_id)
    }

    pub fn listed_pda(&self, asset: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[b"listed", asset.as_ref()], &self.program_id).0
    }

    pub fn offer(&self, key: &Pubkey) -> mud_trade_book::state::Offer {
        let acc = self.svm.get_account(key).expect("offer exists");
        mud_trade_book::state::Offer::try_deserialize(&mut &acc.data[..]).expect("offer decodes")
    }

    pub fn permit_pda(&self, maker: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[b"permit", maker.as_ref()], &self.program_id).0
    }

    pub fn card_pda(&self, asset: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[b"card", asset.as_ref()], &self.program_id).0
    }

    pub fn root_pda(&self, index: u16) -> Pubkey {
        Pubkey::find_program_address(&[b"root", &index.to_le_bytes()], &self.program_id).0
    }

    pub fn card_record(&self, asset: &Pubkey) -> Option<mud_trade_book::state::CardRecord> {
        let acc = self.svm.get_account(&self.card_pda(asset))?;
        mud_trade_book::state::CardRecord::try_deserialize(&mut &acc.data[..]).ok()
    }

    /// `(card_no, finish bit)` from the asset's URI, if it is a card of this collection.
    pub fn uri_identity(&self, asset: &Pubkey) -> Option<(u8, u8)> {
        let acc = self.svm.get_account(asset)?;
        if acc.owner != CORE_ID {
            return None;
        }
        let a = decode(asset, acc.lamports, acc.data, &acc.owner).ok()?;
        if a.collection != Some(self.collection) {
            return None;
        }
        let id = mud_trade_book::card::parse_card_uri(&a.uri)?;
        Some((id.card_no, id.finish.bit()))
    }

    pub fn add_card_root_ix(
        &self,
        signer: &Pubkey,
        index: u16,
        root: [u8; 32],
        leaves: u32,
    ) -> Instruction {
        Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::AddCardRoot { root, leaves }.data(),
            mud_trade_book::accounts::AddCardRoot {
                admin: *signer,
                config: self.config,
                card_root: self.root_pda(index),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        )
    }

    pub fn add_card_root(&mut self, signer: &Keypair, root: [u8; 32], leaves: u32) -> Sent {
        let index = self.config().root_count;
        let ix = self.add_card_root_ix(&signer.pubkey(), index, root, leaves);
        self.send(&[ix], signer, &[])
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register_card_ix(
        &self,
        payer: &Pubkey,
        root_index: u16,
        asset: &Pubkey,
        card_no: u8,
        finish: u8,
        proof: Vec<[u8; 32]>,
    ) -> Instruction {
        Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::RegisterCard {
                root_index,
                card_no,
                finish,
                proof,
            }
            .data(),
            mud_trade_book::accounts::RegisterCard {
                payer: *payer,
                config: self.config,
                card_root: self.root_pda(root_index),
                asset: *asset,
                record: self.card_pda(asset),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        )
    }

    /// Publishes one root over `entries` and returns its index and each entry's proof.
    pub fn publish_root(&mut self, entries: &[(Pubkey, u8, u8)]) -> (u16, Vec<Vec<[u8; 32]>>) {
        let tree = Tree::new(entries);
        let index = self.config().root_count;
        let admin = self.admin.insecure_clone();
        let res = self
            .add_card_root(&admin, tree.root(), entries.len() as u32)
            .res;
        assert!(res.is_ok(), "add_card_root: {res:#?}");
        let proofs = (0..entries.len()).map(|i| tree.proof(i)).collect();
        (index, proofs)
    }

    /// Registers each asset at its URI identity under one fresh root. Skips anything
    /// `prepare_cards` would skip.
    pub fn register_cards(&mut self, assets: &[Pubkey]) {
        let mut entries: Vec<(Pubkey, u8, u8)> = Vec::new();
        for a in assets {
            if entries.iter().any(|(k, _, _)| k == a) || self.exists(&self.card_pda(a)) {
                continue;
            }
            if let Some((n, f)) = self.uri_identity(a) {
                entries.push((*a, n, f));
            }
        }
        if entries.is_empty() {
            return;
        }
        let (index, proofs) = self.publish_root(&entries);
        let admin = self.admin.insecure_clone();
        for ((a, n, f), proof) in entries.iter().zip(proofs) {
            let ix = self.register_card_ix(&admin.pubkey(), index, a, *n, *f, proof);
            let res = self.send(&[ix], &admin, &[]).res;
            assert!(res.is_ok(), "register_card: {res:#?}");
        }
    }

    pub fn config(&self) -> mud_trade_book::state::Config {
        let acc = self.svm.get_account(&self.config).expect("config exists");
        mud_trade_book::state::Config::try_deserialize(&mut &acc.data[..]).expect("config decodes")
    }

    pub fn fee_config(&self) -> Option<mud_trade_book::state::FeeConfig> {
        let acc = self.svm.get_account(&self.fee)?;
        mud_trade_book::state::FeeConfig::try_deserialize(&mut &acc.data[..]).ok()
    }

    pub fn init_fee_config(&mut self, signer: &Keypair, recipient: Pubkey, lamports: u64) -> Sent {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::InitFeeConfig { lamports }.data(),
            mud_trade_book::accounts::InitFeeConfig {
                admin: signer.pubkey(),
                config: self.config,
                fee_config: self.fee,
                recipient,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        );
        self.send(&[ix], signer, &[])
    }

    pub fn set_fee(&mut self, signer: &Keypair, recipient: Pubkey, lamports: u64) -> Sent {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::SetFee { lamports }.data(),
            mud_trade_book::accounts::SetFee {
                admin: signer.pubkey(),
                config: self.config,
                fee_config: self.fee,
                recipient,
            }
            .to_account_metas(None),
        );
        self.send(&[ix], signer, &[])
    }

    pub fn program_data(&self) -> Pubkey {
        Pubkey::find_program_address(
            &[self.program_id.as_ref()],
            &anchor_lang::solana_program::bpf_loader_upgradeable::ID,
        )
        .0
    }

    pub fn init_config(&mut self, signer: &Keypair, collection: Pubkey) -> Sent {
        let program_data = self.program_data();
        self.init_config_with(signer, collection, program_data)
    }

    pub fn init_config_with(
        &mut self,
        signer: &Keypair,
        collection: Pubkey,
        program_data: Pubkey,
    ) -> Sent {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::InitConfig {}.data(),
            mud_trade_book::accounts::InitConfig {
                admin: signer.pubkey(),
                config: self.config,
                collection,
                program: self.program_id,
                program_data,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        );
        self.send(&[ix], signer, &[])
    }

    pub fn set_paused(&mut self, signer: &Keypair, paused: bool) -> Sent {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::SetPaused { paused }.data(),
            mud_trade_book::accounts::SetPaused {
                admin: signer.pubkey(),
                config: self.config,
            }
            .to_account_metas(None),
        );
        self.send(&[ix], signer, &[])
    }

    fn admin_config_ix(&self, signer: &Pubkey, data: Vec<u8>) -> Instruction {
        Instruction::new_with_bytes(
            self.program_id,
            &data,
            mud_trade_book::accounts::AdminConfig {
                admin: *signer,
                config: self.config,
            }
            .to_account_metas(None),
        )
    }

    pub fn propose_admin(&mut self, signer: &Keypair, new_admin: Pubkey) -> Sent {
        let data = mud_trade_book::instruction::ProposeAdmin { new_admin }.data();
        let ix = self.admin_config_ix(&signer.pubkey(), data);
        self.send(&[ix], signer, &[])
    }

    pub fn cancel_admin_transfer(&mut self, signer: &Keypair) -> Sent {
        let data = mud_trade_book::instruction::CancelAdminTransfer {}.data();
        let ix = self.admin_config_ix(&signer.pubkey(), data);
        self.send(&[ix], signer, &[])
    }

    pub fn accept_admin(&mut self, signer: &Keypair) -> Sent {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::AcceptAdmin {}.data(),
            mud_trade_book::accounts::AcceptAdmin {
                new_admin: signer.pubkey(),
                config: self.config,
            }
            .to_account_metas(None),
        );
        self.send(&[ix], signer, &[])
    }

    pub fn open_public(&mut self, signer: &Keypair) -> Sent {
        let data = mud_trade_book::instruction::OpenPublic {}.data();
        let ix = self.admin_config_ix(&signer.pubkey(), data);
        self.send(&[ix], signer, &[])
    }

    pub fn set_cards_enabled(&mut self, signer: &Keypair, enabled: bool) -> Sent {
        let data = mud_trade_book::instruction::SetCardsEnabled { enabled }.data();
        let ix = self.admin_config_ix(&signer.pubkey(), data);
        self.send(&[ix], signer, &[])
    }

    pub fn grant_permit(&mut self, signer: &Keypair, maker: Pubkey) -> Sent {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::GrantMakerPermit { maker }.data(),
            mud_trade_book::accounts::GrantMakerPermit {
                admin: signer.pubkey(),
                config: self.config,
                permit: self.permit_pda(&maker),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
        );
        self.send(&[ix], signer, &[])
    }

    pub fn revoke_permit(&mut self, signer: &Keypair, maker: Pubkey) -> Sent {
        let ix = Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::RevokeMakerPermit { maker }.data(),
            mud_trade_book::accounts::RevokeMakerPermit {
                admin: signer.pubkey(),
                config: self.config,
                permit: self.permit_pda(&maker),
            }
            .to_account_metas(None),
        );
        self.send(&[ix], signer, &[])
    }

    pub fn create_offer_ix(
        &self,
        maker: &Pubkey,
        offer_id: u64,
        give: &[Pubkey],
        want: &[Slot],
        expires_at: i64,
    ) -> Instruction {
        let mut metas = mud_trade_book::accounts::CreateOffer {
            maker: *maker,
            config: self.config,
            maker_permit: self.permit_pda(maker),
            offer: self.offer_pda(maker, offer_id),
            delegate: self.delegate_of(&self.offer_pda(maker, offer_id)),
            maker_state: self.maker_state_pda(maker),
            system_program: system_program::ID,
        }
        .to_account_metas(None);
        metas.extend(give.iter().map(|a| AccountMeta::new_readonly(*a, false)));
        metas.extend(
            give.iter()
                .map(|a| AccountMeta::new(self.listed_pda(a), false)),
        );
        Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::CreateOffer {
                offer_id,
                give: give.to_vec(),
                want: want.to_vec(),
                expires_at,
            }
            .data(),
            metas,
        )
    }

    pub fn create_offer(
        &mut self,
        maker: &Keypair,
        offer_id: u64,
        give: &[Pubkey],
        want: &[Slot],
        expires_at: i64,
    ) -> Sent {
        let ix = self.create_offer_ix(&maker.pubkey(), offer_id, give, want, expires_at);
        self.send(&[ix], maker, &[])
    }

    /// Adds the delegate to each give card, then creates the offer, all in one transaction.
    pub fn list(
        &mut self,
        maker: &Keypair,
        offer_id: u64,
        give: &[Pubkey],
        want: &[Slot],
        expires_at: i64,
    ) -> Pubkey {
        let offer = self.offer_pda(&maker.pubkey(), offer_id);
        let mut ixs: Vec<Instruction> = give
            .iter()
            .map(|a| self.delegate_ix(*a, &maker.pubkey(), &offer))
            .collect();
        ixs.push(self.create_offer_ix(&maker.pubkey(), offer_id, give, want, expires_at));
        let res = self.send(&ixs, maker, &[]).res;
        assert!(res.is_ok(), "list: {res:?}");
        self.offer_pda(&maker.pubkey(), offer_id)
    }

    pub fn fill_ix_with(
        &self,
        taker: &Pubkey,
        offer: &Pubkey,
        give_accounts: &[Pubkey],
        taker_assets: &[Pubkey],
        core_program: Pubkey,
    ) -> Instruction {
        let max_fee = self.fee_config().map_or(0, |f| f.lamports);
        self.fill_ix_fee(
            taker,
            offer,
            give_accounts,
            taker_assets,
            core_program,
            max_fee,
        )
    }

    /// `fill_ix_with` with an explicit `max_fee`, paid to the recipient the fee account names.
    pub fn fill_ix_fee(
        &self,
        taker: &Pubkey,
        offer: &Pubkey,
        give_accounts: &[Pubkey],
        taker_assets: &[Pubkey],
        core_program: Pubkey,
        max_fee: u64,
    ) -> Instruction {
        let recipient = self
            .fee_config()
            .map_or(self.fee_recipient, |f| f.recipient);
        self.fill_ix_full(
            taker,
            offer,
            give_accounts,
            taker_assets,
            core_program,
            max_fee,
            recipient,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fill_ix_full(
        &self,
        taker: &Pubkey,
        offer: &Pubkey,
        give_accounts: &[Pubkey],
        taker_assets: &[Pubkey],
        core_program: Pubkey,
        max_fee: u64,
        fee_recipient: Pubkey,
    ) -> Instruction {
        let o = self.offer(offer);
        let mut metas = mud_trade_book::accounts::FillOffer {
            taker: *taker,
            taker_permit: self.permit_pda(taker),
            offer: *offer,
            maker: o.maker,
            config: self.config,
            delegate: self.delegate_of(offer),
            fee_config: self.fee,
            fee_recipient,
            collection: self.collection,
            core_program,
            system_program: system_program::ID,
        }
        .to_account_metas(None);
        metas.extend(give_accounts.iter().map(|a| AccountMeta::new(*a, false)));
        metas.extend(taker_assets.iter().map(|a| AccountMeta::new(*a, false)));
        metas.extend(
            o.give
                .iter()
                .map(|a| AccountMeta::new(self.listed_pda(a), false)),
        );
        metas.extend(
            o.want
                .iter()
                .zip(taker_assets)
                .filter(|(slot, _)| slot.is_cards())
                .map(|(_, a)| AccountMeta::new_readonly(self.card_pda(a), false)),
        );
        Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::FillOffer { max_fee }.data(),
            metas,
        )
    }

    /// Registers the taker's cards for the offer's `Cards` slots, as a client does before
    /// filling. Admin-paid so the taker's balance checks are untouched. Skips assets that are
    /// not this collection's, whose URI is outside the scheme, or that are already registered.
    pub fn prepare_cards(&mut self, offer: &Pubkey, taker_assets: &[Pubkey]) {
        let o = self.offer(offer);
        let assets: Vec<Pubkey> = o
            .want
            .iter()
            .zip(taker_assets)
            .filter(|(slot, _)| slot.is_cards())
            .map(|(_, a)| *a)
            .collect();
        self.register_cards(&assets);
    }

    pub fn fill(&mut self, taker: &Keypair, offer: &Pubkey, taker_assets: &[Pubkey]) -> Sent {
        self.prepare_cards(offer, taker_assets);
        let give = self.offer(offer).give;
        let ix = self.fill_ix_with(&taker.pubkey(), offer, &give, taker_assets, CORE_ID);
        self.send(&[ix], taker, &[])
    }

    /// Cancel with the give assets and Listed PDAs passed explicitly, for substitution tests.
    pub fn cancel_ix_with(
        &self,
        signer: &Pubkey,
        offer: &Pubkey,
        give_accounts: &[Pubkey],
        listed_accounts: &[Pubkey],
    ) -> Instruction {
        let mut metas = mud_trade_book::accounts::CancelOffer {
            maker: *signer,
            offer: *offer,
            config: self.config,
            delegate: self.delegate_of(offer),
            collection: self.collection,
            core_program: CORE_ID,
            system_program: system_program::ID,
        }
        .to_account_metas(None);
        metas.extend(give_accounts.iter().map(|a| AccountMeta::new(*a, false)));
        metas.extend(listed_accounts.iter().map(|a| AccountMeta::new(*a, false)));
        Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::CancelOffer {}.data(),
            metas,
        )
    }

    pub fn cancel_ix(&self, signer: &Pubkey, offer: &Pubkey) -> Instruction {
        let o = self.offer(offer);
        let listed: Vec<Pubkey> = o.give.iter().map(|a| self.listed_pda(a)).collect();
        self.cancel_ix_with(signer, offer, &o.give, &listed)
    }

    pub fn cancel(&mut self, signer: &Keypair, offer: &Pubkey) -> Sent {
        let ix = self.cancel_ix(&signer.pubkey(), offer);
        self.send(&[ix], signer, &[])
    }

    /// Close with the maker, give assets and Listed PDAs passed explicitly.
    pub fn close_ix_with(
        &self,
        closer: &Pubkey,
        offer: &Pubkey,
        maker: Pubkey,
        give_accounts: &[Pubkey],
        listed_accounts: &[Pubkey],
    ) -> Instruction {
        let mut metas = mud_trade_book::accounts::CloseOffer {
            closer: *closer,
            offer: *offer,
            maker,
            config: self.config,
            delegate: self.delegate_of(offer),
            collection: self.collection,
            core_program: CORE_ID,
            system_program: system_program::ID,
        }
        .to_account_metas(None);
        metas.extend(give_accounts.iter().map(|a| AccountMeta::new(*a, false)));
        metas.extend(listed_accounts.iter().map(|a| AccountMeta::new(*a, false)));
        Instruction::new_with_bytes(
            self.program_id,
            &mud_trade_book::instruction::CloseOffer {}.data(),
            metas,
        )
    }

    pub fn close_ix(&self, closer: &Pubkey, offer: &Pubkey) -> Instruction {
        let o = self.offer(offer);
        let listed: Vec<Pubkey> = o.give.iter().map(|a| self.listed_pda(a)).collect();
        self.close_ix_with(closer, offer, o.maker, &o.give, &listed)
    }

    pub fn close(&mut self, closer: &Keypair, offer: &Pubkey) -> Sent {
        let ix = self.close_ix(&closer.pubkey(), offer);
        self.send(&[ix], closer, &[])
    }

    /// The asset decoded by the program's own reader.
    pub fn read(&self, asset: &Pubkey) -> mud_trade_book::core_io::CoreAsset {
        let acc = self.svm.get_account(asset).expect("asset exists");
        decode(asset, acc.lamports, acc.data, &acc.owner).expect("program reader decodes")
    }

    /// Snapshot of wallet lamports and asset owners, to prove what did or did not move.
    pub fn snap(&self, wallets: &[Pubkey], assets: &[Pubkey]) -> Snap {
        Snap {
            lamports: wallets.iter().map(|w| (*w, self.lamports(w))).collect(),
            owners: assets.iter().map(|a| (*a, self.owner_of(a))).collect(),
        }
    }

    /// Every asset kept its owner; every wallet kept its lamports except `payer`, which paid
    /// exactly the transaction fee.
    #[track_caller]
    pub fn assert_nothing_moved(&self, before: &Snap, payer: &Pubkey, res: &TransactionResult) {
        let fee = fee_of(res);
        for (a, owner) in &before.owners {
            assert_eq!(self.owner_of(a), *owner, "asset {a} moved");
        }
        for (w, lamports) in &before.lamports {
            let expect = if w == payer {
                lamports - fee
            } else {
                *lamports
            };
            assert_eq!(self.lamports(w), expect, "lamports of {w} changed");
        }
    }

    /// Rent held by the offer and its `Listed` PDAs right now.
    pub fn offer_rent(&self, offer: &Pubkey) -> u64 {
        let o = self.offer(offer);
        self.lamports(offer)
            + o.give
                .iter()
                .map(|a| self.lamports(&self.listed_pda(a)))
                .sum::<u64>()
    }
}

pub struct Snap {
    pub lamports: Vec<(Pubkey, u64)>,
    pub owners: Vec<(Pubkey, Pubkey)>,
}

pub fn fee_of(res: &TransactionResult) -> u64 {
    match res {
        Ok(m) => m.fee,
        Err(f) => f.meta.fee,
    }
}

pub fn cu_of(res: &TransactionResult) -> u64 {
    match res {
        Ok(m) => m.compute_units_consumed,
        Err(f) => f.meta.compute_units_consumed,
    }
}

/// Run the program's Core reader over raw account bytes.
pub fn decode(
    key: &Pubkey,
    lamports: u64,
    mut data: Vec<u8>,
    owner: &Pubkey,
) -> anchor_lang::Result<mud_trade_book::core_io::CoreAsset> {
    let mut lamports = lamports;
    let info = anchor_lang::prelude::AccountInfo::new(
        key,
        false,
        false,
        &mut lamports,
        &mut data,
        owner,
        false,
    );
    mud_trade_book::core_io::read_asset(&info)
}

/// LiteSVM's ProgramData has no upgrade authority; give it one so `init_config` can be gated.
pub fn set_upgrade_authority(svm: &mut LiteSVM, program_id: &Pubkey, authority: &Pubkey) {
    let pd = Pubkey::find_program_address(
        &[program_id.as_ref()],
        &anchor_lang::solana_program::bpf_loader_upgradeable::ID,
    )
    .0;
    let mut acc = svm.get_account(&pd).expect("programdata");
    // UpgradeableLoaderState::ProgramData { slot: u64, upgrade_authority_address: Option<Pubkey> }
    // = [u32 tag = 3][u64 slot][u8 option][32 bytes]
    assert_eq!(&acc.data[0..4], &3u32.to_le_bytes());
    acc.data[12] = 1;
    acc.data[13..45].copy_from_slice(authority.as_ref());
    svm.set_account(pd, acc).expect("set programdata");
}

/// Off-chain registry tree, built the same way as the program's reference builder.
pub struct Tree {
    levels: Vec<Vec<[u8; 32]>>,
}

impl Tree {
    pub fn new(entries: &[(Pubkey, u8, u8)]) -> Self {
        use mud_trade_book::registry::{leaf_hash, node_hash};
        let leaves: Vec<[u8; 32]> = entries
            .iter()
            .map(|(a, n, f)| leaf_hash(a, *n, *f))
            .collect();
        let mut levels = vec![leaves];
        while levels.last().unwrap().len() > 1 {
            let next = levels
                .last()
                .unwrap()
                .chunks(2)
                .map(|p| {
                    if p.len() == 2 {
                        node_hash(&p[0], &p[1])
                    } else {
                        p[0]
                    }
                })
                .collect();
            levels.push(next);
        }
        Tree { levels }
    }

    pub fn root(&self) -> [u8; 32] {
        self.levels.last().unwrap()[0]
    }

    pub fn proof(&self, mut i: usize) -> Vec<[u8; 32]> {
        let mut out = Vec::new();
        for level in &self.levels[..self.levels.len() - 1] {
            if (i ^ 1) < level.len() {
                out.push(level[i ^ 1]);
            }
            i /= 2;
        }
        out
    }
}

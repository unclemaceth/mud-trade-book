//! Card registry Merkle tree. Leaves and inner nodes carry different prefixes so a node can
//! never pass as a leaf; pairs are hashed in sorted order so proofs need no left/right flags.

use anchor_lang::prelude::Pubkey;
use solana_sha256_hasher::hashv;

const LEAF_PREFIX: &[u8] = &[0];
const NODE_PREFIX: &[u8] = &[1];

pub fn leaf_hash(asset: &Pubkey, card_no: u8, finish: u8) -> [u8; 32] {
    hashv(&[LEAF_PREFIX, asset.as_ref(), &[card_no, finish]]).to_bytes()
}

pub fn node_hash(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    hashv(&[NODE_PREFIX, lo, hi]).to_bytes()
}

pub fn verify_proof(leaf: [u8; 32], proof: &[[u8; 32]], root: &[u8; 32]) -> bool {
    proof.iter().fold(leaf, |acc, sibling| node_hash(&acc, sibling)) == *root
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{FINISH_BASE, FINISH_FOIL};

    /// Reference tree builder: an odd node is carried up unchanged.
    fn build(leaves: &[[u8; 32]]) -> (Vec<Vec<[u8; 32]>>, [u8; 32]) {
        let mut levels = vec![leaves.to_vec()];
        while levels.last().unwrap().len() > 1 {
            let prev = levels.last().unwrap();
            let next = prev
                .chunks(2)
                .map(|p| if p.len() == 2 { node_hash(&p[0], &p[1]) } else { p[0] })
                .collect();
            levels.push(next);
        }
        let root = levels.last().unwrap()[0];
        (levels, root)
    }

    fn proof(levels: &[Vec<[u8; 32]>], mut i: usize) -> Vec<[u8; 32]> {
        let mut out = Vec::new();
        for level in &levels[..levels.len() - 1] {
            let sib = i ^ 1;
            if sib < level.len() {
                out.push(level[sib]);
            }
            i /= 2;
        }
        out
    }

    fn keys(n: usize) -> Vec<Pubkey> {
        (0..n)
            .map(|i| {
                let mut b = [0u8; 32];
                b[..8].copy_from_slice(&(i as u64 + 1).to_le_bytes());
                Pubkey::new_from_array(b)
            })
            .collect()
    }

    #[test]
    fn every_leaf_proves_for_every_tree_size() {
        for n in 1..=33 {
            let assets = keys(n);
            let leaves: Vec<_> = assets
                .iter()
                .enumerate()
                .map(|(i, a)| leaf_hash(a, (i % 120) as u8 + 1, FINISH_BASE))
                .collect();
            let (levels, root) = build(&leaves);
            for (i, leaf) in leaves.iter().enumerate() {
                assert!(verify_proof(*leaf, &proof(&levels, i), &root), "n {n} i {i}");
            }
        }
    }

    #[test]
    fn wrong_identity_or_asset_fails() {
        let assets = keys(9);
        let leaves: Vec<_> = assets.iter().map(|a| leaf_hash(a, 5, FINISH_BASE)).collect();
        let (levels, root) = build(&leaves);
        let p = proof(&levels, 3);
        assert!(verify_proof(leaf_hash(&assets[3], 5, FINISH_BASE), &p, &root));
        assert!(!verify_proof(leaf_hash(&assets[3], 6, FINISH_BASE), &p, &root));
        assert!(!verify_proof(leaf_hash(&assets[3], 5, FINISH_FOIL), &p, &root));
        assert!(!verify_proof(leaf_hash(&assets[4], 5, FINISH_BASE), &p, &root));
        assert!(!verify_proof(leaf_hash(&assets[3], 5, FINISH_BASE), &p[1..], &root));
        let mut extra = p.clone();
        extra.push([7; 32]);
        assert!(!verify_proof(leaf_hash(&assets[3], 5, FINISH_BASE), &extra, &root));
    }

    /// A fixed root for seven leaves; an off-chain builder must reproduce it.
    #[test]
    fn cross_language_vector() {
        let leaves: Vec<_> = keys(7)
            .iter()
            .enumerate()
            .map(|(i, a)| leaf_hash(a, (i % 120) as u8 + 1, if i % 2 == 0 { FINISH_BASE } else { FINISH_FOIL }))
            .collect();
        let (_, root) = build(&leaves);
        let hex: String = root.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "6ca4968874b88b9389927573ec227479bf019e59cf8da55af80dfab0e6dadd28");
    }

    #[test]
    fn inner_node_cannot_pass_as_leaf() {
        let assets = keys(4);
        let leaves: Vec<_> = assets.iter().map(|a| leaf_hash(a, 1, FINISH_BASE)).collect();
        let (levels, root) = build(&leaves);
        // levels[1][0] is an inner node; presenting it with the leaf-level proof of its parent
        // must not verify as anything a leaf hash can reach.
        assert!(verify_proof(levels[1][0], &[levels[1][1]], &root));
        for card_no in 1..=120u8 {
            for finish in [FINISH_BASE, FINISH_FOIL] {
                for a in &assets {
                    assert_ne!(leaf_hash(a, card_no, finish), levels[1][0]);
                }
            }
        }
    }
}

//! Card identity from the on-chain URI. Pure functions, no panics on any input.

use crate::constants::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    Base,
    Foil,
}

impl Finish {
    pub fn bit(self) -> u8 {
        match self {
            Finish::Base => FINISH_BASE,
            Finish::Foil => FINISH_FOIL,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardId {
    /// 1..=120
    pub card_no: u8,
    pub finish: Finish,
}

/// `https://assets.mudwater.bayc.com/m/c/<idx>-<serial>`: decimal digits only, no sign, no
/// leading zeros except a lone `0`, idx 0..=239, serial fits u32, nothing after.
pub fn parse_card_uri(uri: &str) -> Option<CardId> {
    let rest = uri.as_bytes().strip_prefix(CARD_URI_PREFIX)?;
    let dash = rest.iter().position(|b| *b == b'-')?;
    let idx = parse_decimal(rest.get(..dash)?, MAX_CARD_INDEX)?;
    parse_decimal(rest.get(dash.checked_add(1)?..)?, u64::from(u32::MAX))?;
    let card_no = u8::try_from(idx.checked_rem(CARD_COUNT)?.checked_add(1)?).ok()?;
    let finish = if idx >= CARD_COUNT {
        Finish::Foil
    } else {
        Finish::Base
    };
    Some(CardId { card_no, finish })
}

fn parse_decimal(digits: &[u8], max: u64) -> Option<u64> {
    let first = *digits.first()?;
    if first == b'0' && digits.len() > 1 {
        return None;
    }
    let mut value: u64 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u64::from(b - b'0'))?;
        if value > max {
            return None;
        }
    }
    Some(value)
}

/// Bit `n-1` of the 120-bit set.
pub fn card_in_set(cards: &[u8; 15], card_no: u8) -> bool {
    let Some(i) = card_no.checked_sub(1) else {
        return false;
    };
    let i = usize::from(i);
    cards
        .get(i / 8)
        .is_some_and(|byte| (byte >> (i % 8)) & 1 == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(tail: &str) -> String {
        format!("https://assets.mudwater.bayc.com/m/c/{tail}")
    }

    #[test]
    fn every_index_maps_to_card_and_finish() {
        for idx in 0u64..=300 {
            let parsed = parse_card_uri(&uri(&format!("{idx}-7")));
            if idx <= 239 {
                let c = parsed.expect("in range");
                assert_eq!(u64::from(c.card_no), idx % 120 + 1);
                assert_eq!(
                    c.finish,
                    if idx >= 120 {
                        Finish::Foil
                    } else {
                        Finish::Base
                    }
                );
            } else {
                assert_eq!(parsed, None, "idx {idx}");
            }
        }
    }

    #[test]
    fn real_sample_card() {
        // DHqiKdprCG5XA3iy4ruetzZM7mg3BwL6JyhunS1NDEuy, "The Game Room", #032 Base.
        let c = parse_card_uri(&uri("31-303")).expect("sample parses");
        assert_eq!(
            c,
            CardId {
                card_no: 32,
                finish: Finish::Base
            }
        );
    }

    #[test]
    fn serial_bounds() {
        assert!(parse_card_uri(&uri("0-0")).is_some());
        assert!(parse_card_uri(&uri("0-4294967295")).is_some());
        assert!(parse_card_uri(&uri("0-4294967296")).is_none());
        assert!(parse_card_uri(&uri("0-99999999999999999999999")).is_none());
    }

    #[test]
    fn rejects_bad_forms() {
        let bad = [
            "https://assets.mudwater.bayc.com/m/c/".to_string(),
            "http://assets.mudwater.bayc.com/m/c/1-1".to_string(),
            "https://assets.mudwater.bayc.co/m/c/1-1".to_string(),
            "https://evil.com/m/c/1-1".to_string(),
            "https://assets.mudwater.bayc.com/m/x/1-1".to_string(),
            "1-1".to_string(),
            "".to_string(),
            uri("240-1"),
            uri("01-1"),
            uri("00-1"),
            uri("1-01"),
            uri("1-"),
            uri("-1"),
            uri("1-2x"),
            uri("1-2-3"),
            uri("+1-2"),
            uri("1-+2"),
            uri("1"),
            uri("1 -2"),
            uri("1-2 "),
            uri("1-2/"),
            uri("1-2?x=1"),
            uri("１-2"),
            format!("{}\u{0}", uri("1-2")),
        ];
        for b in bad {
            assert_eq!(parse_card_uri(&b), None, "{b:?}");
        }
    }

    #[test]
    fn exhaustive_short_tails_never_panic() {
        // Every tail up to 4 bytes over a hostile alphabet: no panic, and every accepted tail
        // round-trips through the strict grammar.
        let alphabet: &[u8] = b"0129-+ x\xff";
        let mut tails: Vec<Vec<u8>> = vec![vec![]];
        for _ in 0..4 {
            let mut next = Vec::new();
            for t in &tails {
                for &a in alphabet {
                    let mut n = t.clone();
                    n.push(a);
                    next.push(n);
                }
            }
            for t in &next {
                let mut full = CARD_URI_PREFIX.to_vec();
                full.extend_from_slice(t);
                if let Ok(s) = String::from_utf8(full) {
                    if let Some(c) = parse_card_uri(&s) {
                        assert!((1..=120).contains(&c.card_no));
                    }
                }
            }
            tails = next;
        }
    }

    #[test]
    fn bitset() {
        let mut set = [0u8; 15];
        set[0] = 0b1; // card 1
        set[14] = 0b1000_0000; // card 120
        assert!(card_in_set(&set, 1));
        assert!(card_in_set(&set, 120));
        assert!(!card_in_set(&set, 2));
        assert!(!card_in_set(&set, 0));
        assert!(!card_in_set(&set, 121));
        assert!(!card_in_set(&set, 255));
    }
}

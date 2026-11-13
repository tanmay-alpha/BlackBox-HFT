//! Read the engine's private queue estimate through its public, round-tripping Debug output.

use std::fmt::{self, Debug, Write};

/// Debug emits the front quantity before the cumulative trade quantity. Stop at its delimiter:
/// neither a heap allocation nor formatting the second float is needed to observe the first.
/// Overflow or a changed format remains unreadable, never an inferred queue estimate.
pub(crate) fn front_qty(pos: &impl Debug) -> Option<f64> {
    struct Prefix {
        bytes: [u8; 128],
        len: usize,
        complete: bool,
    }
    impl Write for Prefix {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let end = s.find(',');
            let part = &s[..end.unwrap_or(s.len())];
            if self.len + part.len() > self.bytes.len() {
                return Err(fmt::Error);
            }
            self.bytes[self.len..self.len + part.len()].copy_from_slice(part.as_bytes());
            self.len += part.len();
            if end.is_some() {
                self.complete = true;
                return Err(fmt::Error);
            }
            Ok(())
        }
    }
    let mut prefix = Prefix {
        bytes: [0; 128],
        len: 0,
        complete: false,
    };
    let _ = write!(&mut prefix, "{pos:?}");
    if !prefix.complete {
        return None;
    }
    parse_front_qty(std::str::from_utf8(&prefix.bytes[..prefix.len]).ok()?)
}

pub(crate) fn parse_front_qty(debug: &str) -> Option<f64> {
    let rest = debug.split_once("front_q_qty: ")?.1;
    let end = rest.find([',', ' ', '}']).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Position(f64, f64);

    impl Debug for Position {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("QueuePos")
                .field("front_q_qty", &self.0)
                .field("cum_trade_qty", &self.1)
                .finish()
        }
    }

    #[test]
    fn prefix_matches_full_debug_bit_for_bit() {
        let check = |value| {
            let pos = Position(value, 123.456);
            assert_eq!(
                front_qty(&pos).map(f64::to_bits),
                parse_front_qty(&format!("{pos:?}")).map(f64::to_bits),
            );
        };
        for value in [
            0.0,
            -0.0,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            f64::MAX,
            -f64::MAX,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ] {
            check(value);
        }
        let mut bits = 7_u64;
        for _ in 0..20_000 {
            bits = bits.wrapping_mul(6364136223846793005).wrapping_add(1);
            check(f64::from_bits(bits));
        }
    }

    #[test]
    fn unknown_or_oversized_debug_is_unreadable() {
        assert_eq!(front_qty(&"unrecognized engine state"), None);
        assert_eq!(front_qty(&"x".repeat(256)), None);
    }
}

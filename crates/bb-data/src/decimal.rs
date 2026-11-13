//! Exact decimal values for tick-grid and lot-grid checks.
//!
//! Whether `9350.1` is a multiple of `0.1` cannot be decided in binary floating point
//! (`9350.1 / 0.1` is not an integer in `f64`), so source text is parsed into an integer mantissa
//! and a power-of-ten scale and compared exactly.

/// `mantissa * 10^-scale`, normalized so that the mantissa has no trailing decimal zeros unless
/// the scale is zero.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decimal {
    mantissa: i128,
    scale: u32,
}

impl Decimal {
    /// Parses `[+-]digits[.digits][e[+-]digits]` (at least one mantissa digit). Anything else,
    /// including `inf`, `nan`, whitespace and values too large for 128 bits, is rejected.
    pub(crate) fn parse(text: &str) -> Option<Decimal> {
        let bytes = text.as_bytes();
        let mut i = 0usize;
        let negative = match bytes.first() {
            Some(b'-') => {
                i = 1;
                true
            }
            Some(b'+') => {
                i = 1;
                false
            }
            _ => false,
        };

        let mut mantissa: i128 = 0;
        let mut digits = 0usize;
        let mut frac_digits: i64 = 0;
        let mut seen_point = false;
        while i < bytes.len() {
            match bytes[i] {
                b @ b'0'..=b'9' => {
                    mantissa = mantissa.checked_mul(10)?.checked_add((b - b'0') as i128)?;
                    digits += 1;
                    if seen_point {
                        frac_digits += 1;
                    }
                }
                b'.' if !seen_point => seen_point = true,
                b'e' | b'E' => break,
                _ => return None,
            }
            i += 1;
        }
        if digits == 0 {
            return None;
        }

        let mut exponent: i64 = 0;
        if i < bytes.len() {
            i += 1;
            let exponent_negative = match bytes.get(i) {
                Some(b'-') => {
                    i += 1;
                    true
                }
                Some(b'+') => {
                    i += 1;
                    false
                }
                _ => false,
            };
            let first_digit = i;
            while i < bytes.len() {
                let b = bytes[i];
                if !b.is_ascii_digit() {
                    return None;
                }
                exponent = exponent * 10 + (b - b'0') as i64;
                // Far beyond anything a price or quantity can be; also bounds the loops below.
                if exponent > 1_000 {
                    return None;
                }
                i += 1;
            }
            if i == first_digit {
                return None;
            }
            if exponent_negative {
                exponent = -exponent;
            }
        }

        if mantissa == 0 {
            return Some(Decimal {
                mantissa: 0,
                scale: 0,
            });
        }
        let mut scale = frac_digits - exponent;
        while scale > 0 && mantissa % 10 == 0 {
            mantissa /= 10;
            scale -= 1;
        }
        while scale < 0 {
            mantissa = mantissa.checked_mul(10)?;
            scale += 1;
        }
        if negative {
            mantissa = -mantissa;
        }
        Some(Decimal {
            mantissa,
            scale: u32::try_from(scale).ok()?,
        })
    }

    /// The decimal a configured `f64` size denotes: its shortest round-trip representation
    /// (`0.1_f64` means one tenth, not the binary value slightly above it). `None` unless the
    /// value is finite and positive.
    pub(crate) fn from_positive_f64(value: f64) -> Option<Decimal> {
        if !value.is_finite() || value <= 0.0 {
            return None;
        }
        // `Display` for `f64` prints the shortest digits that round-trip and never uses an
        // exponent.
        Decimal::parse(&format!("{value}"))
    }

    /// Whether `self` is an integer multiple of `unit` (which must be positive).
    pub(crate) fn is_multiple_of(&self, unit: &Decimal) -> bool {
        if unit.mantissa <= 0 {
            return false;
        }
        if self.mantissa == 0 {
            return true;
        }
        // A multiple of `unit` has at most `unit.scale` decimals; a normalized value with more
        // cannot be one.
        if self.scale > unit.scale {
            return false;
        }
        10i128
            .checked_pow(unit.scale - self.scale)
            .and_then(|factor| self.mantissa.checked_mul(factor))
            .is_some_and(|scaled| scaled % unit.mantissa == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(text: &str) -> Decimal {
        Decimal::parse(text).unwrap_or_else(|| panic!("`{text}` should parse"))
    }

    #[test]
    fn parses_plain_and_scientific_notation() {
        assert_eq!(
            d("9350.5"),
            Decimal {
                mantissa: 93505,
                scale: 1
            }
        );
        assert_eq!(
            d("100"),
            Decimal {
                mantissa: 100,
                scale: 0
            }
        );
        assert_eq!(d("1.50"), d("1.5"));
        assert_eq!(
            d("0.0010"),
            Decimal {
                mantissa: 1,
                scale: 3
            }
        );
        assert_eq!(
            d("1e-7"),
            Decimal {
                mantissa: 1,
                scale: 7
            }
        );
        assert_eq!(
            d("2.5E+2"),
            Decimal {
                mantissa: 250,
                scale: 0
            }
        );
        assert_eq!(
            d("-3.25"),
            Decimal {
                mantissa: -325,
                scale: 2
            }
        );
        assert_eq!(
            d("0.000"),
            Decimal {
                mantissa: 0,
                scale: 0
            }
        );
        assert_eq!(d("5."), d("5"));
        assert_eq!(d(".5"), d("0.5"));
    }

    #[test]
    fn rejects_everything_that_is_not_a_decimal_number() {
        for bad in [
            "",
            ".",
            "-",
            "abc",
            "1.2.3",
            "1,5",
            " 1",
            "1 ",
            "inf",
            "NaN",
            "1e",
            "1e+",
            "1e5.0",
            "0x10",
            "1_000",
            "1e99999",
            "1234567890123456789012345678901234567890",
        ] {
            assert_eq!(Decimal::parse(bad), None, "`{bad}` must be rejected");
        }
    }

    #[test]
    fn grid_membership_is_exact() {
        let cases = [
            ("9350.5", "0.5", true),
            ("9350.25", "0.5", false),
            ("9350.1", "0.1", true),
            ("0.30000000000000004", "0.1", false),
            ("0.003", "0.001", true),
            ("0.0035", "0.001", false),
            ("100", "0.01", true),
            ("1e-3", "0.001", true),
            ("7", "1", true),
            ("7.5", "1", false),
            ("15", "5", true),
            ("12", "5", false),
            ("0", "0.5", true),
            ("-1.5", "0.5", true),
            ("0.01", "0.02", false),
        ];
        for (value, unit, expected) in cases {
            assert_eq!(
                d(value).is_multiple_of(&d(unit)),
                expected,
                "{value} multiple of {unit}"
            );
        }
        assert!(!d("1").is_multiple_of(&d("0")));
    }

    #[test]
    fn sizes_given_as_f64_mean_their_shortest_decimal() {
        for (value, text) in [
            (0.1, "0.1"),
            (0.01, "0.01"),
            (0.5, "0.5"),
            (1.0, "1"),
            (0.001, "0.001"),
            (1e-7, "0.0000001"),
        ] {
            assert_eq!(Decimal::from_positive_f64(value), Some(d(text)));
        }
        assert_eq!(Decimal::from_positive_f64(0.0), None);
        assert_eq!(Decimal::from_positive_f64(-0.5), None);
        assert_eq!(Decimal::from_positive_f64(f64::NAN), None);
        assert_eq!(Decimal::from_positive_f64(f64::INFINITY), None);
    }
}

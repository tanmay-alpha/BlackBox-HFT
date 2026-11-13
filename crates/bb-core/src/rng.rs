//! Deterministic pseudo-random numbers.
//!
//! The generators are implemented here rather than taken from a crate so that a given seed
//! produces the same stream for the lifetime of the project, independent of dependency upgrades.
//! Only integer operations and IEEE-754 `+ - * /` are used, so streams are bit-identical across
//! platforms (no `ln`/`cos`, whose last bit varies between math libraries).
//!
//! Algorithms: SplitMix64 (Steele, Lea, Flood 2014) for seeding and xoshiro256** (Blackman, Vigna
//! 2018, public domain reference implementation) for the stream.

/// SplitMix64. Used to expand one `u64` seed into generator state and to derive sub-seeds.
#[derive(Clone, Debug)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Derives an independent sub-seed for a named stream, so that adding a consumer of randomness
/// never shifts the numbers another consumer receives.
pub fn derive_seed(seed: u64, stream: &str) -> u64 {
    let mut h = seed ^ 0xCBF2_9CE4_8422_2325;
    for b in stream.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    SplitMix64::new(h).next_u64()
}

/// xoshiro256**.
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut sm = SplitMix64::new(seed);
        Self {
            s: [sm.next_u64(), sm.next_u64(), sm.next_u64(), sm.next_u64()],
        }
    }

    /// Generator for a named sub-stream of `seed`.
    pub fn for_stream(seed: u64, stream: &str) -> Self {
        Self::new(derive_seed(seed, stream))
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in `[0, 1)` with 53 bits of precision.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform integer in `[0, n)`. `n` must be non-zero. Uses rejection, so it is unbiased.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0)");
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }

    /// Uniform integer in `[lo, hi]` inclusive.
    pub fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        assert!(lo <= hi, "range_i64: lo > hi");
        let span = (hi as i128 - lo as i128 + 1) as u64;
        lo.wrapping_add(self.below(span) as i64)
    }

    /// `true` with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }

    /// Approximately standard normal: the sum of twelve uniforms minus six (Irwin-Hall). Exact
    /// arithmetic only, so it is bit-reproducible everywhere. Support is `[-6, 6]`.
    pub fn approx_normal(&mut self) -> f64 {
        let mut s = 0.0;
        for _ in 0..12 {
            s += self.next_f64();
        }
        s - 6.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_matches_reference_vector() {
        // Reference output for seed 1234567 from the public-domain splitmix64.c.
        let mut sm = SplitMix64::new(1234567);
        assert_eq!(sm.next_u64(), 6457827717110365317);
        assert_eq!(sm.next_u64(), 3203168211198807973);
        assert_eq!(sm.next_u64(), 9817491932198370423);
    }

    #[test]
    fn same_seed_same_stream_and_streams_are_independent() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let xs: Vec<u64> = (0..64).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..64).map(|_| b.next_u64()).collect();
        assert_eq!(xs, ys);

        let mut c = Rng::for_stream(42, "latency.entry");
        let mut d = Rng::for_stream(42, "latency.response");
        assert_ne!(c.next_u64(), d.next_u64());
    }

    #[test]
    fn ranges_hold() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let f = r.next_f64();
            assert!((0.0..1.0).contains(&f));
            let i = r.range_i64(-3, 3);
            assert!((-3..=3).contains(&i));
            let n = r.approx_normal();
            assert!((-6.0..=6.0).contains(&n));
        }
    }
}

//! Deterministic random number generation.
//!
//! The simulation must produce identical results for one mission seed on
//! every platform (x86_64 native and wasm32), across compiler versions, and
//! across releases of third-party crates. To guarantee that, Murmur carries
//! its own small PCG32 implementation instead of depending on the `rand`
//! ecosystem, and all randomness flows from a mission seed through named
//! streams (see [`Stream`]).

use serde::{Deserialize, Serialize};

/// Named RNG streams forked from the mission seed.
///
/// Generation and turn resolution consume randomness at different rates, so
/// they draw from independent streams: rejecting a player command must not
/// consume tie-breaker randomness, and replaying a mission must not depend on
/// how many random numbers generation happened to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// Layout, population, schedules, and item placement.
    Generation,
    /// In-turn tie-breakers during simultaneous action resolution.
    Resolution,
}

impl Stream {
    fn id(self) -> u64 {
        match self {
            Stream::Generation => 1,
            Stream::Resolution => 2,
        }
    }
}

/// A PCG32 (XSH-RR) generator: 64-bit state, 63-bit stream selector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

const PCG_MULT: u64 = 6364136223846793005;

impl Pcg32 {
    /// Creates the generator for one named stream of a mission seed.
    pub fn for_stream(mission_seed: u64, stream: Stream) -> Self {
        Self::new(mission_seed, stream.id())
    }

    /// Creates a generator from a seed and an arbitrary stream selector.
    pub fn new(seed: u64, stream: u64) -> Self {
        let inc = (stream << 1) | 1;
        let mut rng = Self { state: 0, inc };
        rng.step();
        rng.state = rng.state.wrapping_add(seed);
        rng.step();
        rng
    }

    fn step(&mut self) {
        self.state = self.state.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
    }

    /// Returns the next 32 uniformly distributed bits.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.step();
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Returns a uniform value in `0..bound` (`bound` must be non-zero).
    ///
    /// Uses Lemire-style rejection to avoid modulo bias.
    pub fn below(&mut self, bound: u32) -> u32 {
        debug_assert!(bound > 0, "Pcg32::below requires a non-zero bound");
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let value = self.next_u32();
            let mul = u64::from(value) * u64::from(bound);
            if (mul as u32) >= threshold {
                return (mul >> 32) as u32;
            }
        }
    }

    /// Returns a uniform value in the inclusive range `lo..=hi`.
    pub fn range_inclusive(&mut self, lo: u32, hi: u32) -> u32 {
        debug_assert!(lo <= hi);
        lo + self.below(hi - lo + 1)
    }

    /// Returns true with probability `numerator / denominator`.
    pub fn chance(&mut self, numerator: u32, denominator: u32) -> bool {
        self.below(denominator) < numerator
    }

    /// Picks one element of a non-empty slice.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u32) as usize]
    }

    /// Removes and returns one element of a non-empty vector.
    pub fn take<T>(&mut self, items: &mut Vec<T>) -> T {
        items.remove(self.below(items.len() as u32) as usize)
    }

    /// Fisher-Yates shuffle with deterministic order.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u32 + 1) as usize;
            items.swap(i, j);
        }
    }
}

/// SplitMix64, used to derive successor mission seeds from a previous seed so
/// "play again" stays reproducible from the first seed of a session.
pub fn split_mix_64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_seeds_produce_identical_sequences() {
        let mut a = Pcg32::for_stream(42, Stream::Generation);
        let mut b = Pcg32::for_stream(42, Stream::Generation);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn streams_are_independent() {
        let mut generation = Pcg32::for_stream(42, Stream::Generation);
        let mut resolution = Pcg32::for_stream(42, Stream::Resolution);
        let generation_values: Vec<u32> = (0..8).map(|_| generation.next_u32()).collect();
        let resolution_values: Vec<u32> = (0..8).map(|_| resolution.next_u32()).collect();
        assert_ne!(generation_values, resolution_values);
    }

    #[test]
    fn pcg32_matches_reference_vector() {
        // Reference values for PCG32 XSH-RR with seed=42, stream selector=54,
        // from the canonical pcg32-demo output.
        let mut rng = Pcg32::new(42, 54);
        let expected = [
            0xa15c02b7u32,
            0x7b47f409,
            0xba1d3330,
            0x83d2f293,
            0xbfa4784b,
            0xcbed606e,
        ];
        for value in expected {
            assert_eq!(rng.next_u32(), value);
        }
    }

    #[test]
    fn below_stays_in_bounds_and_covers_range() {
        let mut rng = Pcg32::new(7, 1);
        let mut seen = [false; 5];
        for _ in 0..500 {
            let v = rng.below(5);
            assert!(v < 5);
            seen[v as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn shuffle_is_deterministic_for_a_seed() {
        let mut a_rng = Pcg32::new(9, 1);
        let mut b_rng = Pcg32::new(9, 1);
        let mut a: Vec<u32> = (0..20).collect();
        let mut b: Vec<u32> = (0..20).collect();
        a_rng.shuffle(&mut a);
        b_rng.shuffle(&mut b);
        assert_eq!(a, b);
    }
}

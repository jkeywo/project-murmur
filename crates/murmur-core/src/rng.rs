//! Deterministic random number generation.
//!
//! The simulation must produce identical results for one mission seed on
//! every platform (x86_64 native and wasm32), across compiler versions, and
//! across releases of third-party crates. To guarantee that, Murmur uses a
//! small PCG32 rather than the `rand` ecosystem, and all randomness flows from
//! a mission seed through named streams (see [`Stream`]).
//!
//! The arithmetic lives in `vellum-rng`, shared with the other game that wrote
//! the same generator for the same reason. The *layout* stays here: this type
//! is a field of [`World`](crate::world::World), and `World`'s RON text is the
//! mission fingerprint, so its shape is part of the save format. The shared
//! crate is borrowed a draw at a time rather than stored.
//!
//! Note that the bounded draw is Lemire's multiply-and-shift, and the other
//! game's is a remainder. They compute the same rejection threshold, which
//! makes them look interchangeable in a diff; they are not, and swapping them
//! would change every value every mission draws.

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

impl Pcg32 {
    /// Creates the generator for one named stream of a mission seed.
    pub fn for_stream(mission_seed: u64, stream: Stream) -> Self {
        Self::new(mission_seed, stream.id())
    }

    /// Creates a generator from a seed and an arbitrary stream selector.
    pub fn new(seed: u64, stream: u64) -> Self {
        let (state, inc) = vellum_rng::Pcg32::canonical(seed, stream).into_parts();
        Self { state, inc }
    }

    /// Returns the next 32 uniformly distributed bits.
    pub fn next_u32(&mut self) -> u32 {
        self.borrow(vellum_rng::Pcg32::next_u32)
    }

    /// Returns a uniform value in `0..bound` (`bound` must be non-zero).
    ///
    /// Uses Lemire-style multiply-and-shift. Not interchangeable with the
    /// remainder-based draw the other game uses: for the same state the two
    /// return different values, which is why the shared crate keeps both under
    /// their own names rather than offering one `below`.
    pub fn below(&mut self, bound: u32) -> u32 {
        debug_assert!(bound > 0, "Pcg32::below requires a non-zero bound");
        self.borrow(|rng| rng.below_lemire(bound))
    }

    /// Run one draw on the shared generator and take the advanced state back.
    ///
    /// The fields stay here rather than being replaced by
    /// `vellum_rng::Pcg32`, because this struct is a field of `World` and
    /// `World`'s RON text *is* the mission fingerprint. The layout is part of
    /// the save format; only the arithmetic is shared.
    fn borrow<T>(&mut self, draw: impl FnOnce(&mut vellum_rng::Pcg32) -> T) -> T {
        let mut rng = vellum_rng::Pcg32::from_parts(self.state, self.inc);
        let result = draw(&mut rng);
        (self.state, self.inc) = rng.into_parts();
        result
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
    vellum_rng::split_mix_64(seed)
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

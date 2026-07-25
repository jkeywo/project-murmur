//! Deterministic random number generation.
//!
//! The simulation must produce identical results for one mission seed on
//! every platform (x86_64 native and wasm32), across compiler versions, and
//! across releases of third-party crates. To guarantee that, Murmur uses a
//! small PCG32 rather than the `rand` ecosystem, and all randomness flows from
//! a mission seed through named streams (see [`Stream`]).
//!
//! The generator is the fleet's unified construction from `vellum-rng` —
//! `Pcg32::seeded` (canonical PCG warm-up over a SplitMix64-mixed seed, with
//! this game's named streams as the stream selectors) and the Lemire bounded
//! draw — and the shared `Pcg32` type is stored directly, so the serialised
//! generator inside `World` (whose RON text is the mission fingerprint) is
//! the same `{ state, inc }` shape across the fleet.
//!
//! This replaced the pre-unification construction (canonical seeding without
//! the SplitMix pass) under the fleet decision `rng-unification-breaks-saves`:
//! every fixture in this repository was re-blessed, and the share-code prefix
//! bumped to `MUR2-` so format-1 codes are refused rather than misread.

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

/// The simulation's PCG32, stored as the fleet's shared generator type.
///
/// A thin vocabulary wrapper: the type, seeding, and draws are `vellum-rng`'s;
/// the helper names (`pick`, `take`, `chance`) are this game's.
/// `serde(transparent)` keeps the serialised shape exactly the inner
/// `{ state, inc }` — which is part of the mission fingerprint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pcg32 {
    inner: vellum_rng::Pcg32,
}

impl Pcg32 {
    /// Creates the generator for one named stream of a mission seed.
    pub fn for_stream(mission_seed: u64, stream: Stream) -> Self {
        Self::new(mission_seed, stream.id())
    }

    /// Creates a generator from a seed and an arbitrary stream selector.
    pub fn new(seed: u64, stream: u64) -> Self {
        Self {
            inner: vellum_rng::Pcg32::seeded(seed, stream),
        }
    }

    /// Returns the next 32 uniformly distributed bits.
    pub fn next_u32(&mut self) -> u32 {
        self.inner.next_u32()
    }

    /// Returns a uniform value in `0..bound` (`bound` must be non-zero).
    pub fn below(&mut self, bound: u32) -> u32 {
        debug_assert!(bound > 0, "Pcg32::below requires a non-zero bound");
        self.inner.below(bound)
    }

    /// Returns a uniform value in the inclusive range `lo..=hi`.
    pub fn range_inclusive(&mut self, lo: u32, hi: u32) -> u32 {
        self.inner.range_inclusive(lo, hi)
    }

    /// Returns true with probability `numerator / denominator`.
    pub fn chance(&mut self, numerator: u32, denominator: u32) -> bool {
        self.inner.chance(numerator, denominator)
    }

    /// Picks one element of a non-empty slice.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.inner.pick_index(items.len())]
    }

    /// Removes and returns one element of a non-empty vector.
    pub fn take<T>(&mut self, items: &mut Vec<T>) -> T {
        items.remove(self.inner.pick_index(items.len()))
    }

    /// Fisher-Yates shuffle with deterministic order.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        self.inner.shuffle(items);
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

    /// Pinned to the fleet construction (`seeded(42, 1)`), the same constants
    /// vellum-rng pins, so a drift fails in both places. The published PCG
    /// reference vector lives with `vellum_rng::Pcg32::canonical`; this game
    /// seeds through the fleet's SplitMix pass and deliberately does not
    /// reproduce it.
    #[test]
    fn sequence_is_pinned_to_the_fleet_construction() {
        let mut rng = Pcg32::new(42, 1);
        let first: Vec<u32> = (0..4).map(|_| rng.next_u32()).collect();
        assert_eq!(first, [4176028549, 3950285441, 2197104919, 1103863609]);
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

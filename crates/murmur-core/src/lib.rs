//! Project Murmur simulation core.
//!
//! An authoritative, deterministic, discrete-turn social-stealth simulation.
//! This crate has no engine, terminal, or platform dependencies: the same
//! code drives the native terminal build, the WebAssembly build, tests, and
//! replays. Controllers (human, AI, replay, test) submit the same primitive
//! action intents; once a turn's batch is frozen, resolution never branches
//! on the source of an action.

pub mod access;
pub mod actions;
pub mod ai;
pub mod data;
pub mod generator;
pub mod geom;
pub mod map;
pub mod path;
pub mod perception;
pub mod rng;
pub mod turn;
pub mod world;

//! Deterministic mission replay.
//!
//! The mission seed determines layout, population, schedules, item
//! placement, and tie-breaker randomness; replaying the accepted player
//! commands against that seed reproduces the same turn-by-turn simulation
//! result. The turn driver records every accepted command, so a
//! [`MissionRecord`] plus this module is a complete, portable replay.

use serde::{Deserialize, Serialize};

use crate::actions::Command;
use crate::data::GameData;
use crate::generator::{GenError, generate};
use crate::turn::TurnDriver;
use crate::world::World;

/// Everything needed to reproduce a mission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MissionRecord {
    pub seed: u64,
    pub commands: Vec<Command>,
}

#[derive(Clone, Debug)]
pub enum ReplayError {
    Generation(GenError),
    /// A recorded command was rejected on replay: the record and the
    /// simulation have diverged, which the determinism guarantee forbids.
    Diverged {
        at_command: usize,
        reason: String,
    },
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::Generation(err) => write!(f, "replay generation failed: {err}"),
            ReplayError::Diverged { at_command, reason } => {
                write!(f, "replay diverged at command #{at_command}: {reason}")
            }
        }
    }
}

impl std::error::Error for ReplayError {}

/// Replays a record and returns the final world.
pub fn replay(data: &GameData, record: &MissionRecord) -> Result<World, ReplayError> {
    let world = generate(data, record.seed).map_err(ReplayError::Generation)?;
    let mut driver = TurnDriver::new(world, data);
    for (index, command) in record.commands.iter().enumerate() {
        while driver.player_busy() && !driver.mission_over() {
            driver.continue_busy(data);
        }
        if driver.mission_over() {
            break;
        }
        if let Err(reason) = driver.submit(data, command) {
            return Err(ReplayError::Diverged {
                at_command: index,
                reason: reason.message().to_string(),
            });
        }
    }
    while driver.player_busy() && !driver.mission_over() {
        driver.continue_busy(data);
    }
    Ok(driver.into_world())
}

/// A stable fingerprint of the full world state (FNV-1a over the RON
/// serialisation). Two worlds with equal fingerprints went through
/// identical simulations.
pub fn world_fingerprint(world: &World) -> u64 {
    let text = ron::to_string(world).expect("world serialises");
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

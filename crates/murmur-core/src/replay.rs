//! Deterministic mission replay.
//!
//! The mission seed determines layout, population, schedules, item
//! placement, and tie-breaker randomness; replaying the accepted player
//! commands against that seed reproduces the same turn-by-turn simulation
//! result. The turn driver records every accepted command, so a
//! [`MissionRecord`] plus this module is a complete, portable replay.

use serde::{Deserialize, Serialize};

use crate::actions::Command;
use crate::contract::MissionConfig;
use crate::data::GameData;
use crate::generator::{GenError, generate};
use crate::turn::TurnDriver;
use crate::world::World;

/// Everything needed to reproduce a mission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MissionRecord {
    pub config: MissionConfig,
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

/// A mission being driven, paired with the data it needs.
///
/// The driver takes `&GameData` on every call — the rules are a parameter, not
/// state — so it cannot implement the shared [`Simulation`](vellum_replay::Simulation)
/// trait directly. This carries the pair, which is what the trait wants:
/// something that advances when handed a command and nothing else.
pub struct MissionSim<'a> {
    data: &'a GameData,
    driver: TurnDriver,
}

impl<'a> MissionSim<'a> {
    pub fn new(data: &'a GameData, world: World) -> Self {
        Self {
            driver: TurnDriver::new(world, data),
            data,
        }
    }

    pub fn driver(&self) -> &TurnDriver {
        &self.driver
    }

    pub fn into_world(self) -> World {
        self.driver.into_world()
    }
}

impl vellum_replay::Simulation for MissionSim<'_> {
    type Command = Command;
    type Rejection = String;

    fn apply(&mut self, command: &Command) -> Result<(), String> {
        self.driver
            .submit(self.data, command)
            .map(|_| ())
            .map_err(|reason| reason.message().to_string())
    }

    fn is_over(&self) -> bool {
        self.driver.mission_over()
    }

    fn digest(&self) -> u64 {
        world_fingerprint(self.driver.world())
    }

    /// Murmur's actions can take several turns. The driver refuses a new
    /// command while one is in flight, so the shared replay loop pumps here
    /// between commands rather than submitting into a busy mission.
    fn needs_continuation(&self) -> bool {
        self.driver.player_busy()
    }

    fn continue_step(&mut self) {
        self.driver.continue_busy(self.data);
    }
}

/// Replays a record and returns the final world.
pub fn replay(data: &GameData, record: &MissionRecord) -> Result<World, ReplayError> {
    let world = generate(data, &record.config).map_err(ReplayError::Generation)?;
    let mut sim = MissionSim::new(data, world);
    vellum_replay::replay_into(&mut sim, &record.commands).map_err(|fault| {
        ReplayError::Diverged {
            at_command: fault.at_command,
            reason: fault.rejection,
        }
    })?;
    Ok(sim.into_world())
}

/// A stable fingerprint of the full world state (FNV-1a over the RON
/// serialisation). Two worlds with equal fingerprints went through
/// identical simulations.
///
/// The arithmetic is shared with the other game that reinvented it; the
/// *choice* of RON is not portable and stays here, because it is part of this
/// game's save format rather than a detail of hashing. Murmur fingerprints RON
/// text where rogue-hunter digests postcard bytes, and the two never agree on
/// a number for the same value.
pub fn world_fingerprint(world: &World) -> u64 {
    vellum_digest::digest_ron(world)
}

/// The share-code format for a recorded mission.
///
/// `MUR1-` is project-murmur, share-code format 1. Missions were already
/// reproducible from a [`MissionRecord`]; this only makes one small enough to
/// paste, which is what turns "it reproduces" into "here, try it yourself".
///
/// The prefix is what stops a code from the other game decoding into something
/// plausible, and the version in it is what will let a future format change
/// refuse an old code instead of misreading it.
pub const MISSION_CODEC: vellum_digest::ShareCodec = vellum_digest::ShareCodec::new("MUR1-");

impl MissionRecord {
    /// A pasteable code for this mission.
    pub fn share_code(&self) -> String {
        MISSION_CODEC.encode(self).unwrap_or_default()
    }

    /// Read a mission back from a share code.
    pub fn from_share_code(code: &str) -> Result<Self, vellum_digest::CodecError> {
        MISSION_CODEC.decode(code)
    }
}

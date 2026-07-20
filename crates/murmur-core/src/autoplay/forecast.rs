//! Where everyone will be, if nothing surprising happens.
//!
//! A planner that compares "walk over the trespass" against "fetch a
//! costume" has to price both, and the price of a route depends on who is
//! standing on it *when you get there* — thirty turns from now, not now.
//! So a route planner needs a forecast, and a forecast needs a model of
//! how the venue moves.
//!
//! # The model is the simulation
//!
//! Rather than approximate the AI, this clones the world and runs the real
//! turn loop forward on the copy with the player standing still. Every
//! divergence between the forecast and what actually happens is therefore
//! caused by the player's own interference — their moods raised, their
//! tiles blocked, their bodies found — and never by the model being a
//! worse account of the AI than the AI is.
//!
//! That is deliberate, and it is what makes this slice worth building
//! before any search on top of it. It measures the *ceiling*: the best any
//! predictor could do. A cheaper approximation can only be worse, so if
//! even this decays quickly then planning against a forecast is not viable
//! here and the search should not be written.
//!
//! # What it costs
//!
//! One world clone plus `horizon` simulated turns. That is far too
//! expensive to run every turn for every candidate route, which is a real
//! constraint on the design above it: a planner wants one forecast per
//! re-plan, shared across the whole search, not one per edge.

use std::collections::BTreeSet;

use crate::actions::Command;
use crate::data::GameData;
use crate::geom::Pos;
use crate::turn::TurnDriver;
use crate::world::{ActorId, World};

/// Predicted positions for the next `horizon` turns.
pub struct Forecast {
    /// `frames[offset][actor.0]` is where that actor is predicted to be
    /// `offset + 1` turns from the forecast's origin. `None` once an actor
    /// is dead, departed, or otherwise off the board.
    frames: Vec<Vec<Option<Pos>>>,
    /// The tiles under someone's gaze at each predicted turn. Precomputed
    /// once here rather than per query, because the point of a shared
    /// forecast is that the search prices thousands of edges against it and
    /// must never re-run a sight walk to do so.
    watched: Vec<BTreeSet<Pos>>,
}

impl Forecast {
    /// Runs the venue forward on a copy of the world, with the player
    /// standing still, and records where everyone goes.
    pub fn read(world: &World, data: &GameData, horizon: u32) -> Self {
        let mut driver = TurnDriver::new(world.clone(), data);
        let mut frames = Vec::with_capacity(horizon as usize);
        let mut watched = Vec::with_capacity(horizon as usize);
        for _ in 0..horizon {
            if driver.mission_over() {
                // Nothing further is predictable; hold the last reading so
                // callers can still index the whole horizon.
                frames.push(snapshot(driver.world()));
                watched.push(watched_tiles(driver.world(), data));
                continue;
            }
            if driver.player_busy() {
                driver.continue_busy(data);
            } else if driver.submit(data, &Command::Wait).is_err() {
                frames.push(snapshot(driver.world()));
                watched.push(watched_tiles(driver.world(), data));
                continue;
            }
            frames.push(snapshot(driver.world()));
            watched.push(watched_tiles(driver.world(), data));
        }
        Self { frames, watched }
    }

    /// Whether `pos` is predicted to be under someone's gaze `offset` turns
    /// from now, counting from one. Out past the horizon it answers
    /// `false`: the search leans on the heuristic there, not on a forecast
    /// it does not have.
    pub fn watched_at(&self, pos: Pos, offset: u32) -> bool {
        let Some(index) = offset.checked_sub(1) else {
            return false;
        };
        self.watched
            .get(index as usize)
            .is_some_and(|set| set.contains(&pos))
    }

    /// Where `actor` is expected to be `offset` turns from now, counting
    /// from one. `None` when the horizon does not reach that far, or the
    /// actor is no longer on the board.
    pub fn position(&self, actor: ActorId, offset: u32) -> Option<Pos> {
        let index = usize::try_from(offset.checked_sub(1)?).ok()?;
        *self.frames.get(index)?.get(actor.0 as usize)?
    }

    /// How many turns the forecast covers.
    pub fn horizon(&self) -> u32 {
        self.frames.len() as u32
    }
}

/// Every tile a living witness can see this frame. Any role: staff and
/// guests raise alarms as readily as guards, and a route that threads a
/// crowded bar to dodge one guard is not the quiet route it looks.
fn watched_tiles(world: &World, data: &GameData) -> BTreeSet<Pos> {
    let mut seen = BTreeSet::new();
    for actor in &world.actors {
        if actor.is_player() || !actor.alive() || actor.departed || actor.hidden_in.is_some() {
            continue;
        }
        seen.extend(crate::perception::npc_visible_tiles(world, data, actor.id));
    }
    seen
}

fn snapshot(world: &World) -> Vec<Option<Pos>> {
    world
        .actors
        .iter()
        .map(|actor| (actor.alive() && !actor.departed).then_some(actor.pos))
        .collect()
}

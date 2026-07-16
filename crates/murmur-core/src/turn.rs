//! The turn driver.
//!
//! Owns the world and the prepared action store. Each cycle: translate one
//! valid queued player command into an action, freeze it with the already
//! prepared AI actions, resolve the turn simultaneously, run perception,
//! and prepare the next turn's AI actions. The driver performs exactly one
//! turn per accepted command (plus continuation turns while a multi-turn
//! player action is in progress) and never advances time on a rejection.

use serde::{Deserialize, Serialize};

use crate::actions::{
    Command, PreparedAction, RejectReason, TurnEvents, intent_duration, resolve_turn, translate,
};
use crate::ai::prepare_npc_actions;
use crate::data::GameData;
use crate::perception;
use crate::world::World;

/// Holds controller-neutral actions prepared for one specific future turn.
/// Once frozen, a batch does not distinguish human, AI, replay, or test
/// sources.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PreparedActionStore {
    actions: Vec<PreparedAction>,
}

impl PreparedActionStore {
    pub fn actions(&self) -> &[PreparedAction] {
        &self.actions
    }
}

/// The authoritative driver for one mission.
#[derive(Clone, Debug)]
pub struct TurnDriver {
    world: World,
    store: PreparedActionStore,
    /// Every accepted command, in order — the replay record.
    accepted: Vec<Command>,
}

/// What one driver step produced.
#[derive(Clone, Debug)]
pub struct TurnReport {
    pub events: TurnEvents,
    /// Perception messages (alarms, screams, propagation).
    pub perception: Vec<String>,
}

impl TurnDriver {
    /// Wraps a freshly generated world and prepares the first turn's AI
    /// actions.
    pub fn new(mut world: World, data: &GameData) -> Self {
        let actions = prepare_npc_actions(&mut world, data);
        Self {
            world,
            store: PreparedActionStore { actions },
            accepted: Vec::new(),
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// Mutable world access for scenario setup in tests and tools. Not a
    /// gameplay API: play mutates the world only through commands.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Consumes the driver, yielding the final world.
    pub fn into_world(self) -> World {
        self.world
    }

    /// The accepted-command record for deterministic replay.
    pub fn accepted_commands(&self) -> &[Command] {
        &self.accepted
    }

    /// True while a multi-turn player action is still resolving; the queue
    /// must not submit another command yet.
    pub fn player_busy(&self) -> bool {
        self.store
            .actions
            .iter()
            .any(|p| p.actor == self.world.player)
    }

    /// True once the mission has ended.
    pub fn mission_over(&self) -> bool {
        self.world.outcome.is_some()
    }

    /// Submits one player command for the upcoming turn.
    ///
    /// Rejection is pure: the world is untouched, no randomness is
    /// consumed, and the prepared AI actions for this turn stay exactly as
    /// they were.
    pub fn submit(
        &mut self,
        data: &GameData,
        command: &Command,
    ) -> Result<TurnReport, RejectReason> {
        debug_assert!(
            !self.player_busy(),
            "submit while a player action is in progress"
        );
        let intent = translate(&self.world, data, command)?;
        let remaining = intent_duration(data, &self.world, self.world.player, &intent);
        self.store.actions.push(PreparedAction {
            actor: self.world.player,
            intent,
            remaining,
        });
        self.accepted.push(*command);
        Ok(self.step(data))
    }

    /// Advances one turn while the player's multi-turn action continues.
    pub fn continue_busy(&mut self, data: &GameData) -> TurnReport {
        debug_assert!(self.player_busy(), "continue_busy without a busy player");
        self.step(data)
    }

    fn step(&mut self, data: &GameData) -> TurnReport {
        let events = resolve_turn(&mut self.world, data, &mut self.store.actions);
        let perception_messages = if self.world.outcome.is_none() {
            perception::update(&mut self.world, data)
        } else {
            Vec::new()
        };
        // Prepare the next turn's AI actions; in-progress multi-turn
        // actions (the player's, and any NPC's) carry over.
        if self.world.outcome.is_none() {
            let mut next = prepare_npc_actions(&mut self.world, data);
            next.retain(|n| !self.store.actions.iter().any(|p| p.actor == n.actor));
            self.store.actions.extend(next);
            self.store.actions.sort_by_key(|p| p.actor);
        } else {
            self.store.actions.clear();
        }
        TurnReport {
            events,
            perception: perception_messages,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::ActionResult as AR;
    use crate::data::GameData;
    use crate::generator::generate;
    use crate::geom::Dir4;
    use crate::map::TileKind;
    use crate::world::Hands;

    fn driver(seed: u64) -> (GameData, TurnDriver) {
        let data = GameData::embedded().unwrap();
        let world = generate(&data, seed).unwrap();
        let driver = TurnDriver::new(world, &data);
        (data, driver)
    }

    /// A direction the player can actually walk from spawn.
    fn open_direction(driver: &TurnDriver) -> Dir4 {
        let world = driver.world();
        let from = world.player_actor().pos;
        Dir4::ALL
            .into_iter()
            .find(|d| {
                let dest = from.step(*d);
                matches!(world.map.tile(dest), TileKind::Floor)
                    && world.furniture_at(dest).is_none()
                    && world.standing_actor_at(dest).is_none()
            })
            .expect("spawn has at least one open side")
    }

    #[test]
    fn valid_move_advances_exactly_one_turn() {
        let (data, mut driver) = driver(3);
        let dir = open_direction(&driver);
        let before = driver.world().player_actor().pos;
        let report = driver.submit(&data, &Command::Move(dir)).unwrap();
        assert_eq!(driver.world().turn, 1);
        assert_eq!(driver.world().player_actor().pos, before.step(dir));
        assert_eq!(report.events.player_result, Some(AR::Completed));
    }

    #[test]
    fn rejection_is_pure_and_advances_nothing() {
        let (data, mut driver) = driver(3);
        // Find a blocked direction (wall) from spawn.
        let world = driver.world();
        let from = world.player_actor().pos;
        let blocked = Dir4::ALL.into_iter().find(|d| {
            matches!(
                world.map.tile(from.step(*d)),
                TileKind::Wall | TileKind::Void
            )
        });
        let Some(blocked) = blocked else {
            return; // no wall adjacent on this seed; other seeds cover it
        };
        let world_before = ron::to_string(driver.world()).unwrap();
        let store_before = driver.store.actions.clone();
        let result = driver.submit(&data, &Command::Move(blocked));
        assert!(result.is_err());
        assert_eq!(
            ron::to_string(driver.world()).unwrap(),
            world_before,
            "rejection must not mutate the world"
        );
        assert_eq!(
            driver.store.actions, store_before,
            "rejection must keep prepared AI actions"
        );
        assert_eq!(driver.world().turn, 0);
    }

    #[test]
    fn wait_passes_time_and_npcs_follow_routines() {
        let (data, mut driver) = driver(9);
        let npc_positions_before: Vec<_> = driver
            .world()
            .actors
            .iter()
            .filter(|a| !a.is_player())
            .map(|a| a.pos)
            .collect();
        for _ in 0..12 {
            driver.submit(&data, &Command::Wait).unwrap();
        }
        assert_eq!(driver.world().turn, 12);
        let npc_positions_after: Vec<_> = driver
            .world()
            .actors
            .iter()
            .filter(|a| !a.is_player())
            .map(|a| a.pos)
            .collect();
        assert_ne!(
            npc_positions_before, npc_positions_after,
            "routines should move at least one NPC in a dozen turns"
        );
    }

    #[test]
    fn draw_then_holster_round_trips() {
        let (data, mut driver) = driver(3);
        driver.submit(&data, &Command::DrawOrHolster).unwrap();
        assert!(matches!(
            driver.world().player_actor().hands,
            Hands::Drawn(_)
        ));
        driver.submit(&data, &Command::DrawOrHolster).unwrap();
        assert_eq!(driver.world().player_actor().hands, Hands::Free);
    }

    #[test]
    fn toggle_crouch_flips_state_each_turn() {
        let (data, mut driver) = driver(3);
        driver.submit(&data, &Command::ToggleCrouch).unwrap();
        assert!(driver.world().player_actor().crouched);
        driver.submit(&data, &Command::ToggleCrouch).unwrap();
        assert!(!driver.world().player_actor().crouched);
    }

    #[test]
    fn shoot_requires_drawn_weapon_and_kills_in_sight() {
        let (data, mut driver) = driver(3);
        // Aim at the nearest NPC the player can see; walk the world until
        // one is visible from spawn (turn 0 usually has several).
        let world = driver.world();
        let target = world
            .actors
            .iter()
            .filter(|a| !a.is_player() && a.alive())
            .find(|a| {
                a.pos.floor == world.player_actor().pos.floor
                    && world
                        .player_actor()
                        .pos
                        .chebyshev(a.pos)
                        .is_some_and(|d| d <= data.tuning.pistol_range)
                    && crate::map::line_of_sight(
                        world.player_actor().pos,
                        a.pos,
                        world.sight_blocker(false),
                    )
            })
            .map(|a| a.id);
        let Some(target) = target else { return };

        // Holstered: rejected.
        assert!(matches!(
            driver.submit(&data, &Command::Shoot(target)),
            Err(RejectReason::WeaponNotDrawn)
        ));
        driver.submit(&data, &Command::DrawOrHolster).unwrap();
        let report = driver.submit(&data, &Command::Shoot(target));
        if let Ok(report) = report {
            assert_eq!(report.events.player_result, Some(AR::Completed));
            assert!(!driver.world().actor(target).alive());
            let pistol = driver
                .world()
                .carried_items(driver.world().player)
                .find(|i| i.spec == "silenced-pistol")
                .unwrap();
            assert_eq!(pistol.charges, data.tuning.pistol_rounds - 1);
        }
    }

    #[test]
    fn mission_ends_when_target_dies_and_player_extracts() {
        let (data, mut driver) = driver(3);
        // The player spawns on the entrance extraction tile. Kill the
        // target directly (full playthroughs live in the replay suite):
        // standing on an exit with the target dead ends the mission on the
        // next resolved turn.
        let target = driver.world().target;
        driver.world.actor_mut(target).condition = crate::world::BodyCondition::Dead;
        assert!(
            driver
                .world()
                .extraction_tiles
                .contains(&driver.world().player_actor().pos),
            "seed 3 spawns the player on the entrance exit"
        );
        let report = driver.submit(&data, &Command::Wait).unwrap();
        assert_eq!(
            driver.world().outcome,
            Some(crate::world::MissionOutcome::Extracted),
            "extraction with a dead target must end the mission: {:?}",
            report.events.messages
        );
    }

    #[test]
    fn carrying_a_body_halves_movement_cadence() {
        let (data, mut driver) = driver(3);
        // Kill an adjacent-ish NPC via the world, carry it, and time a step.
        let victim = driver
            .world()
            .actors
            .iter()
            .filter(|a| !a.is_player())
            .min_by_key(|a| {
                a.pos
                    .chebyshev(driver.world().player_actor().pos)
                    .unwrap_or(i16::MAX)
            })
            .map(|a| a.id)
            .unwrap();
        {
            let world = &mut driver.world;
            let player_pos = world.actor(world.player).pos;
            world.actor_mut(victim).condition = crate::world::BodyCondition::Dead;
            world.actor_mut(victim).pos = player_pos;
        }
        driver.submit(&data, &Command::CarryBody(victim)).unwrap();
        assert!(matches!(
            driver.world().player_actor().hands,
            Hands::CarryingBody(_)
        ));
        let dir = open_direction(&driver);
        let turn_before = driver.world().turn;
        let report = driver.submit(&data, &Command::Move(dir)).unwrap();
        assert_eq!(report.events.player_result, Some(AR::InProgress));
        assert!(driver.player_busy());
        let report2 = driver.continue_busy(&data);
        assert_eq!(report2.events.player_result, Some(AR::Completed));
        assert_eq!(
            driver.world().turn,
            turn_before + 2,
            "a carried step takes two turns"
        );
    }
}

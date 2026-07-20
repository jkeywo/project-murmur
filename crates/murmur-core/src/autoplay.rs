//! The goal-driven autoplayer.
//!
//! Drives a mission headlessly: closes on the target, takes the kill from
//! behind, tidies the body, and walks out through an extraction tile. Used to
//! mint golden replays, exercise the command surface in CI, and prove
//! generated missions are completable in play rather than only on paper.
//! Fully deterministic for a given world.
//!
//! **It does not follow the certified route.** [`crate::planner`] proves a
//! mission is completable by closing capabilities over the layout; this bot
//! ignores that proof entirely and plays from world state. That independence
//! is the point. A bot that replayed the planner's own reasoning could only
//! ever confirm the planner is self-consistent, whereas one that reaches
//! [`MissionOutcome::Extracted`] by its own route is evidence the proof
//! describes a mission a player could actually finish.
//!
//! The bot is written as a priority list of candidate commands rather than a
//! plan, because command rejection in this game is *pure* — a refused command
//! costs no turn, no randomness, and no prepared NPC action (see
//! [`TurnDriver::submit`]). Guessing is therefore free, so the bot proposes
//! the best thing it can think of, then the next, and lets the rulebook be the
//! authority on what is legal. That also means every rejection path gets
//! exercised on every seed, which is most of the value of running this in CI.
//!
//! The bot never issues [`Command::Cheat`]. The debug switches are commands
//! like any other and would be faithfully recorded into a replay, which would
//! quietly make a golden fixture a record of a game nobody can play.

use crate::access;
use crate::actions::Command;
use crate::data::{GameData, Role, Zone};
use crate::geom::{Dir4, Pos};
use crate::map::TileKind;
use crate::turn::TurnDriver;
use crate::world::{
    ActorId, BodyCondition, FurnitureId, FurnitureKind, Hands, MissionOutcome, Mood, World,
};

/// Hard cap on turns. A mission that has not resolved by here is stalled in a
/// way the stall detector did not catch, and the run is reported unfinished
/// rather than left to spin.
const MAX_TURNS: u32 = 4000;

/// Consecutive turns without observable progress before the bot gives up.
/// Generous because waiting is sometimes correct: the kill needs the target's
/// back turned, and standing still until a patrol passes is real play.
const MAX_STALLS: u32 = 120;

/// Turns spent failing to reach the target before the bot stops being subtle
/// and reaches for the firearm. Long enough that a normal stealth approach is
/// never cut short, short enough that a mission the quiet route cannot solve
/// still gets tested against the loud one.
const PATIENCE: u32 = 400;

/// What one driven mission did.
#[derive(Clone, Debug)]
pub struct AutoplayReport {
    /// `None` when the bot never reached an ending.
    pub outcome: Option<MissionOutcome>,
    /// Turns advanced, including continuation turns of multi-turn actions.
    pub turns: u32,
    /// Every command the driver accepted, in order — the replay record.
    pub commands: Vec<Command>,
    /// Set when the bot gave up rather than reaching an outcome.
    pub stalled: bool,
}

impl AutoplayReport {
    /// The mission ended the way a contract wants it to end.
    pub fn won(&self) -> bool {
        self.outcome == Some(MissionOutcome::Extracted)
    }
}

/// Drives `driver` to an outcome, or until it stalls.
pub fn autoplay(data: &GameData, driver: &mut TurnDriver) -> AutoplayReport {
    let mut bot = Bot {
        turns: 0,
        frustration: 0,
        looted_bodies: Vec::new(),
        looted_wardrobes: Vec::new(),
    };
    let mut stalls = 0u32;
    let mut last = progress(driver.world());

    while bot.turns < MAX_TURNS && !driver.mission_over() {
        // A multi-turn action already in flight is not a decision point: the
        // player is committed, and the only legal thing is to let it run.
        if driver.player_busy() {
            driver.continue_busy(data);
            bot.turns += 1;
            continue;
        }

        let wanted = bot.candidates(driver.world(), data);
        let mut acted = false;
        for command in wanted {
            if driver.submit(data, &command).is_ok() {
                bot.accepted(&command);
                acted = true;
                break;
            }
        }
        if !acted {
            // Waiting is always legal, so this cannot loop forever without
            // the stall detector noticing.
            let _ = driver.submit(data, &Command::Wait);
        }
        bot.turns += 1;

        let now = progress(driver.world());
        if now == last {
            stalls += 1;
            if stalls >= MAX_STALLS {
                return bot.report(driver, true);
            }
        } else {
            stalls = 0;
            last = now;
        }
    }

    let stalled = !driver.mission_over();
    bot.report(driver, stalled)
}

struct Bot {
    turns: u32,
    /// Turns spent unable to close on a living target. Drives the escalation
    /// from garrote to firearm.
    frustration: u32,
    /// Bodies and wardrobes already stripped.
    ///
    /// Changing clothes is a *swap*: the old set goes back where the new one
    /// came from. Both ends therefore stay stocked forever, so an ungated bot
    /// strips the same corpse every turn for the rest of the mission, each
    /// swap succeeding and nothing changing. Recorded per source rather than
    /// as a single "done" flag so that genuinely upgrading — staff clothes,
    /// then a guard's — is still allowed.
    looted_bodies: Vec<ActorId>,
    looted_wardrobes: Vec<FurnitureId>,
}

/// How far the bot will carry a body to hide it. Short on purpose: see
/// [`Bot::after_the_kill`].
const STOW_RADIUS: i16 = 6;

impl Bot {
    /// Told what the driver actually took, so one-shot decisions are recorded
    /// against what happened rather than against what was merely offered.
    fn accepted(&mut self, command: &Command) {
        match command {
            Command::TakeDisguiseFromBody(id) => self.looted_bodies.push(*id),
            Command::TakeDisguiseFromWardrobe(id) => self.looted_wardrobes.push(*id),
            _ => {}
        }
    }

    fn report(&self, driver: &TurnDriver, stalled: bool) -> AutoplayReport {
        AutoplayReport {
            outcome: driver.world().outcome.clone(),
            turns: self.turns,
            commands: driver.accepted_commands().to_vec(),
            stalled,
        }
    }

    /// Commands worth trying this turn, best first. Every one may be refused;
    /// the caller takes the first the rulebook accepts.
    fn candidates(&mut self, world: &World, data: &GameData) -> Vec<Command> {
        let mut out = Vec::new();
        let player = world.player_actor();
        let target = world.actor(world.target);
        let target_dead = target.condition == BodyCondition::Dead;

        if target_dead {
            self.frustration = 0;
            self.after_the_kill(&mut out, world, data);
            return out;
        }

        self.frustration += 1;

        // The kill, offered whenever it would not be seen.
        //
        // It is refused unless the player is exactly behind the target with
        // free hands, so it costs nothing to ask, and asking is the only way
        // to catch the one turn where a walking target's back happens to be
        // turned. But a garrote taken in front of a witness is a garrote that
        // ends with the courier dead a dozen turns later: the whole room turns
        // and there is no walking out of it. Waiting for the pair of you to be
        // alone is the difference between a contract and an incident.
        //
        // Patience is the escape hatch. A target who is never once unobserved
        // still has to die, and a bot that waited forever would report a
        // finishable mission as a stall.
        // Offered unconditionally, and that is a measured decision rather than
        // a simplification. Holding the garrote until nobody was watching was
        // tried twice — once for any witness, once for guards only — and both
        // lost ground: waiting means shadowing the target across the venue,
        // and following someone around a building is its own kind of
        // conspicuous. Arrests rose by more than the witnessed kills avoided
        // (437 and 303 permille against 456 for taking the shot). The venue
        // punishes loitering harder than it punishes decisiveness, so the bot
        // takes the kill the moment the rulebook allows it.
        out.push(Command::Garrote(world.target));

        // Hands must be free for the kill, so a body picked up before the
        // target is dead is put away immediately — into a container if one is
        // to hand, on the floor otherwise. Hunting with full hands is the one
        // state from which nothing can progress.
        if matches!(player.hands, Hands::CarryingBody(_)) {
            for id in empty_containers_beside(world, player.pos) {
                out.push(Command::HideBody(id));
            }
            out.push(Command::DropBody(None));
        }

        // Patience exhausted: take the shot. This is the escalation that
        // proves a mission the quiet route cannot solve is still winnable, and
        // it exercises the firearm path stealth runs never reach. It is load
        // bearing — removing it costs twenty permille and triples the stalls,
        // because a mission stealth cannot solve otherwise just runs out the
        // clock. Escalating early when already hunted was tried and measured
        // as noise, so the trigger stays the simple one.
        if self.frustration > PATIENCE {
            if matches!(player.hands, Hands::Free) {
                out.push(Command::DrawOrHolster);
            }
            out.push(Command::Shoot(world.target));
        }

        self.acquire(&mut out, world, data, player.pos);
        // Dressing for the job comes before walking to it. Access rules never
        // block movement — the sim lets you walk anywhere you physically can —
        // so a bot that ignores them strolls into a secure wing in civilian
        // clothes and is killed for it. Which is the correct outcome, and why
        // the wardrobe has to come first.
        self.dress_for_the_job(&mut out, world, data);
        if !self.approach(&mut out, world, data) {
            // No route at all. Not a lost mission — an unequipped one.
            self.seek_capability(&mut out, world, data);
        }
        out
    }

    /// If the kill site is off-limits in what the player is wearing, go and
    /// find clothes that cover it before going anywhere near it.
    ///
    /// This is the social-stealth route the planner certifies, played rather
    /// than proved: the bot looks for a wardrobe whose disguise grants the
    /// zone it needs, walks to it, and changes. Returns whether it found one
    /// worth crossing the venue for.
    fn dress_for_the_job(&self, out: &mut Vec<Command>, world: &World, data: &GameData) -> bool {
        let target_pos = world.actor(world.target).pos;
        if access::verdict_for_pos(world, data, world.player, target_pos).is_allowed() {
            return false;
        }
        let Some(room) = world.room_at(target_pos) else {
            return false;
        };
        let (needed_zone, needed_template) = (room.zone, room.template.clone());

        for furniture in &world.furniture {
            if furniture.kind != FurnitureKind::Wardrobe
                || self.looted_wardrobes.contains(&furniture.id)
            {
                continue;
            }
            let Some(spec) = furniture.disguise.as_ref().and_then(|id| data.disguise(id)) else {
                continue;
            };
            let covers =
                spec.zones.contains(&needed_zone) || spec.extra_rooms.contains(&needed_template);
            if !covers {
                continue;
            }
            // Standing beside it already: change here.
            out.push(Command::TakeDisguiseFromWardrobe(furniture.id));
            for dir in Dir4::ALL {
                let beside = furniture.pos.step(dir);
                if let Some(step) = first_step_staying_legal(world, data, beside) {
                    out.push(Command::Move(step));
                }
            }
            return true;
        }
        false
    }

    /// The tidy-up and the walk out. Heat is what turns a clean kill into a
    /// bad debrief, so the body goes somewhere if somewhere is close; the
    /// exit wins the moment it stops being close.
    fn after_the_kill(&mut self, out: &mut Vec<Command>, world: &World, data: &GameData) {
        let player = world.player_actor();

        // The one thing worth doing under threat: shut a body into whatever
        // is already within arm's reach. It costs a single turn and it is
        // what stops the corpse being found.
        if matches!(player.hands, Hands::CarryingBody(_)) {
            for id in empty_containers_beside(world, player.pos) {
                out.push(Command::HideBody(id));
            }
        }

        // Everything else about tidiness is for when nobody is looking. The
        // kill itself is what raises the alarm, so the turns immediately
        // after it are the most dangerous in the mission — standing over the
        // body to rob it is how a won mission becomes a dead courier. When
        // anyone nearby has stopped being relaxed, the exit is the only plan.
        if alarm_nearby(world) {
            self.walk_out(out, world, data);
            return;
        }

        // Rob the corpse on the way past — free, and it exercises the loot
        // path against a body rather than a live mark.
        for dir in Dir4::ALL {
            if let Some(body) = world.body_at(player.pos.step(dir)) {
                out.push(Command::Pickpocket(body.id));
                if !self.looted_bodies.contains(&body.id) {
                    out.push(Command::TakeDisguiseFromBody(body.id));
                }
            }
        }

        // Hiding the body is worth exactly as much as it is close.
        //
        // Carrying a corpse across the venue to find a crate was measured at
        // seventy permille of missions lost: the detour happens in the most
        // dangerous turns of the run, in whatever room the kill happened, with
        // both hands full and no way to take another. Refusing to carry at all
        // scores better than carrying far. So the bot only picks a body up
        // when somewhere to put it is within [`STOW_RADIUS`], and otherwise
        // leaves it where it fell and goes — worse for heat, much better for
        // getting paid.
        let stowable = nearest_empty_container(world, player.pos, STOW_RADIUS);
        if matches!(player.hands, Hands::CarryingBody(_)) {
            match stowable {
                Some(container) => {
                    for dir in Dir4::ALL {
                        if let Some(step) =
                            first_step_staying_legal(world, data, container.step(dir))
                        {
                            out.push(Command::Move(step));
                            break;
                        }
                    }
                }
                // Nothing close enough to be worth it. Put it down and go.
                None => out.push(Command::DropBody(None)),
            }
        } else if stowable.is_some() {
            for dir in Dir4::ALL {
                if let Some(body) = world.body_at(player.pos.step(dir)) {
                    out.push(Command::CarryBody(body.id));
                }
            }
            if let Some(body) = world.body_at(player.pos) {
                out.push(Command::CarryBody(body.id));
            }
        }

        self.walk_out(out, world, data);
    }

    /// Head for the door. Extraction tiles are tried in world order so the
    /// choice is a pure function of the world rather than of arrival order.
    fn walk_out(&self, out: &mut Vec<Command>, world: &World, data: &GameData) {
        for exit in &world.extraction_tiles {
            if self.push_route(out, world, data, *exit) {
                return;
            }
        }
    }

    /// Offer a step towards `goal`, and then the sidesteps.
    ///
    /// The pathfinder does not model actors — deliberately, because
    /// simultaneous resolution arbitrates collisions and a path that treated
    /// every patrolling guard as a wall would flicker every turn. The cost is
    /// that its chosen step can be refused by whoever is standing there, and a
    /// bot that offers only that one step then has nothing to do but wait. It
    /// waits forever, because the guard is at a post and is not going to move.
    ///
    /// So the ideal step goes first and the rest follow, nearest-first, which
    /// is enough to walk around a stationary obstruction without needing to
    /// know it is there. Ties break on [`Dir4::ALL`] order so the choice stays
    /// a pure function of the world.
    fn push_route(
        &self,
        out: &mut Vec<Command>,
        world: &World,
        data: &GameData,
        goal: Pos,
    ) -> bool {
        let Some(primary) = first_step_staying_legal(world, data, goal) else {
            return false;
        };
        let from = world.player_actor().pos;

        // A door in the way is opened deliberately rather than bumped, so the
        // locked case surfaces as a rejection the bot can answer with
        // lockpicks instead of a silent failure to move.
        let ahead = from.step(primary);
        if let TileKind::Door(door) = world.map.tile(ahead)
            && !world.door(door).open
        {
            out.push(Command::OpenDoor(door));
            if world.carries(world.player, data, |spec| spec.lockpick) {
                out.push(Command::PickLock(door));
            }
        }
        out.push(Command::Move(primary));

        let mut alternatives: Vec<(i16, usize, Dir4)> = Dir4::ALL
            .iter()
            .enumerate()
            .filter(|(_, dir)| **dir != primary)
            .filter_map(|(order, dir)| {
                let landing = from.step(*dir);
                if world.blocks_move(landing) {
                    return None;
                }
                // Across a storey there is no distance to compare, so the
                // sidesteps simply keep their declared order.
                let distance = landing.chebyshev(goal).unwrap_or(i16::MAX);
                Some((distance, order, *dir))
            })
            .collect();
        alternatives.sort();
        for (_, _, dir) in alternatives {
            out.push(Command::Move(dir));
        }
        true
    }

    /// Opportunistic pickups: anything adjacent that widens where the player
    /// may legally walk. Disguises and keys are what turn a blocked route
    /// into a route, so the pathfinder gets stronger every time one lands.
    fn acquire(&self, out: &mut Vec<Command>, world: &World, data: &GameData, pos: Pos) {
        // A wardrobe beside us is a change of clothes and usually a change of
        // zone. Cheapest legitimate access there is.
        for id in adjacent_furniture(world, pos, FurnitureKind::Wardrobe) {
            if !self.looted_wardrobes.contains(&id) {
                out.push(Command::TakeDisguiseFromWardrobe(id));
            }
        }

        // Machines are the authored opportunities — fuse boxes, hoists. The
        // bot does not reason about what they do; it presses them when it
        // passes one, which is enough to keep the interact path live.
        for id in adjacent_furniture(world, pos, FurnitureKind::Machine) {
            out.push(Command::Interact(id));
        }

        // Lift keys from anyone standing next to us — but only from someone
        // actually carrying one. Robbing every passer-by on the off-chance was
        // measured and is a bad habit: it is attempted every turn the bot
        // brushes past anybody, each attempt is a chance to be noticed, and
        // almost none of them are carrying anything that opens a door.
        for dir in Dir4::ALL {
            if let Some(other) = world.standing_actor_at(pos.step(dir))
                && !other.is_player()
                && world.carries(other.id, data, |spec| {
                    spec.unlocks.is_some() || spec.invitation || spec.staff_pass
                })
            {
                out.push(Command::Pickpocket(other.id));
            }
        }

        // A noisemaker is worth throwing when the player is stuck and can see
        // somewhere worth emptying. Thrown at the target's tile: whoever is
        // watching them is exactly who is in the way.
        let has_noisemaker = world.carries(world.player, data, |spec| spec.noisemaker);
        if has_noisemaker && self.frustration > PATIENCE / 4 {
            out.push(Command::ThrowNoisemaker(world.actor(world.target).pos));
        }
    }

    /// Close on the tile the kill is taken from, opening or picking whatever
    /// stands in the way. Returns whether any route to the target was found.
    fn approach(&self, out: &mut Vec<Command>, world: &World, data: &GameData) -> bool {
        let target = world.actor(world.target);

        // The garrote is taken from directly behind the target, so that tile
        // — not the target's own — is where the bot is trying to stand.
        let goal = target
            .facing
            .map(|facing| target.pos.step(facing.opposite()))
            .unwrap_or(target.pos);

        // Crouching close in: quieter, and it exercises the stance rule
        // against the perception model rather than in isolation. The two
        // thresholds are hysteresis, not fussiness — a single one flips back
        // and forth every turn while chasing a walking target, and the bot
        // spends the whole mission standing up and crouching down again.
        // ...but only where crouching is not itself the suspicious act. A
        // courier crouch-walking across a crowded dance floor is reported by
        // the first person who looks up, which is the game being right: the
        // stance that hides you in a back corridor is what gives you away in
        // public. Crouch only off the public floor.
        let player = world.player_actor();
        let distance = player.pos.chebyshev(goal);
        let in_public = world
            .room_at(player.pos)
            .is_none_or(|room| room.zone == Zone::Public);
        match (player.crouched, distance) {
            (false, Some(d)) if d <= 4 && !in_public => out.push(Command::ToggleCrouch),
            (true, _) if in_public => out.push(Command::ToggleCrouch),
            (true, Some(d)) if d > 6 => out.push(Command::ToggleCrouch),
            (true, None) => out.push(Command::ToggleCrouch),
            _ => {}
        }

        let mut routed = false;
        for candidate in [goal, target.pos] {
            if self.push_route(out, world, data, candidate) {
                routed = true;
                break;
            }
        }
        routed
    }

    /// Called when there is no route to the target at all.
    ///
    /// The pathfinder refuses a locked door the player cannot open, so "no
    /// path" does not mean the mission is impossible — it means the player is
    /// not yet equipped to walk it. What unlocks a route in this game is
    /// clothes and keys, so the bot goes and gets some. This is the physical
    /// half of the same idea as [`Self::dress_for_the_job`]: that one reacts
    /// to a destination it may not stand on, this one to a door it cannot
    /// open.
    ///
    /// Each branch returns on its first reachable candidate. Pathfinding is a
    /// Dijkstra over the whole venue, and offering every wardrobe in the
    /// building every turn would cost more than the mission is worth.
    fn seek_capability(&self, out: &mut Vec<Command>, world: &World, data: &GameData) {
        // Clothes first: they are the master key, and taking a set is legal.
        for furniture in &world.furniture {
            if furniture.kind != FurnitureKind::Wardrobe
                || self.looted_wardrobes.contains(&furniture.id)
            {
                continue;
            }
            out.push(Command::TakeDisguiseFromWardrobe(furniture.id));
            for dir in Dir4::ALL {
                if let Some(step) = first_step_staying_legal(world, data, furniture.pos.step(dir)) {
                    out.push(Command::Move(step));
                    return;
                }
            }
        }

        // Then keys, off whoever is carrying one. Guards hold the doors that
        // matter, which is also why lifting from them is the risky version.
        for actor in &world.actors {
            if actor.is_player() || !actor.alive() || actor.departed {
                continue;
            }
            if !world.carries(actor.id, data, |spec| spec.unlocks.is_some()) {
                continue;
            }
            out.push(Command::Pickpocket(actor.id));
            for dir in Dir4::ALL {
                if let Some(step) = first_step_staying_legal(world, data, actor.pos.step(dir)) {
                    out.push(Command::Move(step));
                    return;
                }
            }
        }
    }
}

/// Adjacent furniture of one kind, in world order.
fn adjacent_furniture(world: &World, pos: Pos, kind: FurnitureKind) -> Vec<FurnitureId> {
    Dir4::ALL
        .iter()
        .filter_map(|dir| world.furniture_at(pos.step(*dir)))
        .filter(|furniture| furniture.kind == kind)
        .map(|furniture| furniture.id)
        .collect()
}

/// The first step of a route that would rather go the long way round than be
/// somewhere it has no business being.
///
/// [`first_step_towards`](crate::path::first_step_towards) answers a physical
/// question — can this actor walk there — and it is the right function for
/// NPCs, who are all where they belong. The player is not, and access rules
/// never block movement: the sim lets you stroll into a secure wing in a
/// barman's jacket and then has you arrested for it. A bot routed on physical
/// passability alone takes the short way through three restricted rooms and is
/// caught in the second one, which is the game working correctly.
///
/// So both things it would rather avoid are priced rather than forbidden, and
/// a room that must be crossed still can be. Trespass costs more than being
/// seen, because being seen is not itself an offence — it is only what turns
/// one into an incident.
///
/// The guard's-eye view is built once per route rather than asked per tile:
/// the per-tile question costs a cone test, a range test and a line-of-sight
/// walk for every guard, which inside a search over a multi-storey venue is
/// tens of thousands of sight walks per turn.
fn first_step_staying_legal(world: &World, data: &GameData, goal: Pos) -> Option<Dir4> {
    /// What one tile of trespass is worth in plain steps.
    const TRESPASS_COST: u32 = 30;
    /// What one tile under a guard's gaze is worth.
    ///
    /// The exact figure is not load-bearing and should not be treated as
    /// tuned: 4, 8, 15 and 25 all measure within one mission of each other
    /// across the 320-mission corpus. What matters is that the term exists at
    /// all, which is worth about fifty permille. Prefer any legal unwatched
    /// route; how strongly hardly registers.
    const WATCHED_COST: u32 = 8;

    let watched = guarded_tiles(world, data);
    crate::path::first_step_priced(world, data, world.player, goal, |pos| {
        let mut cost = 0;
        if !access::verdict_for_pos(world, data, world.player, pos).is_allowed() {
            cost += TRESPASS_COST;
        }
        if watched.contains(&pos) {
            cost += WATCHED_COST;
        }
        cost
    })
}

/// Every tile a living guard can currently see.
///
/// Built once per route rather than asked per tile; see
/// [`first_step_staying_legal`].
fn guarded_tiles(world: &World, data: &GameData) -> std::collections::BTreeSet<Pos> {
    let mut watched = std::collections::BTreeSet::new();
    for actor in &world.actors {
        if actor.is_player()
            || actor.role != Some(Role::Guard)
            || !actor.alive()
            || actor.departed
            || actor.hidden_in.is_some()
        {
            continue;
        }
        watched.extend(crate::perception::npc_visible_tiles(world, data, actor.id));
    }
    watched
}

/// Whether anyone who can see roughly this far has stopped being relaxed.
///
/// Deliberately coarse. The bot has no access to what any individual NPC
/// believes, and should not — it is a player, not the perception model. What
/// it can fairly notice is that the room has turned, which is what a person
/// playing would notice too.
fn alarm_nearby(world: &World) -> bool {
    let player = world.player_actor().pos;
    world.actors.iter().any(|actor| {
        !actor.is_player()
            && actor.alive()
            && !actor.departed
            && actor
                .ai
                .as_ref()
                .is_some_and(|ai| !matches!(ai.mood, Mood::Relaxed))
            && actor.pos.chebyshev(player).is_some_and(|d| d <= 12)
    })
}

/// The nearest empty container within `radius` on this storey, in world order.
fn nearest_empty_container(world: &World, from: Pos, radius: i16) -> Option<Pos> {
    world
        .furniture
        .iter()
        .filter(|furniture| furniture.kind == FurnitureKind::Container && furniture.body.is_none())
        .filter_map(|furniture| {
            let distance = furniture.pos.chebyshev(from)?;
            (distance <= radius).then_some((distance, furniture.pos))
        })
        .min()
        .map(|(_, pos)| pos)
}

/// Adjacent containers with nothing in them yet. A container already holding
/// a body will refuse a second, so offering it is a wasted candidate.
fn empty_containers_beside(world: &World, pos: Pos) -> Vec<FurnitureId> {
    Dir4::ALL
        .iter()
        .filter_map(|dir| world.furniture_at(pos.step(*dir)))
        .filter(|furniture| furniture.kind == FurnitureKind::Container && furniture.body.is_none())
        .map(|furniture| furniture.id)
        .collect()
}

/// The observable state the stall detector watches. Deliberately coarse: the
/// turn counter is excluded because it always advances, which would make
/// every stall look like progress.
fn progress(world: &World) -> (Pos, bool, bool, usize, u16, Option<ActorId>, String) {
    let player = world.player_actor();
    (
        player.pos,
        world.actor(world.target).condition == BodyCondition::Dead,
        player.crouched,
        world.carried_items(world.player).count(),
        world.mission_heat,
        match player.hands {
            Hands::CarryingBody(id) => Some(id),
            _ => None,
        },
        player.worn_disguise.clone(),
    )
}

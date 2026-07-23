//! NPC controllers.
//!
//! A controller-neutral AI slot: after each resolved turn, every eligible
//! NPC derives exactly one primitive [`ActionIntent`] for the *next* turn
//! from its routine, mood, memory, and the world. The intents go into the
//! same prepared-action store as the player's action; resolution cannot
//! tell them apart.
//!
//! Cadence: relaxed NPCs act on staggered alternating turns
//! (`(turn + actor id) % relaxed_cadence == 0`); suspicious,
//! investigating, alerted, escorting, fleeing, and combat NPCs prepare an
//! action every turn.

use crate::actions::{ActionIntent, PreparedAction, intent_duration};
use crate::data::GameData;
use crate::geom::{Dir4, Pos};
use crate::path::first_step_towards;
use crate::perception::{StandDown, stand_down};
use crate::world::{ActorId, DetailRole, Hands, Mood, Protection, World};

/// Prepares one action per eligible NPC for the upcoming turn.
pub fn prepare_npc_actions(world: &mut World, data: &GameData) -> Vec<PreparedAction> {
    let cadence = u64::from(data.tuning.relaxed_cadence.max(1));
    let ids: Vec<ActorId> = world
        .actors
        .iter()
        .filter(|a| !a.is_player() && a.alive() && !a.departed && a.ai.is_some())
        .map(|a| a.id)
        .collect();

    let mut prepared = Vec::new();
    for id in ids {
        let mood = world.actor(id).ai.as_ref().map(|ai| ai.mood).unwrap();
        if mood == Mood::Relaxed && (u64::from(world.turn) + u64::from(id.0)) % cadence != 0 {
            continue;
        }
        // A standing detail assignment replaces only the *relaxed*
        // behaviour. Anything that raised this guard's mood — a noise, a
        // body, the player behaving oddly — outranks the escort, and when
        // perception calms them back to Relaxed they resume escorting with
        // no special handling. That is the whole reason the assignment is
        // orthogonal to mood rather than a mood of its own.
        let escorting = world.actor(id).ai.as_ref().and_then(|ai| ai.detail.clone());
        // A leash is another orthogonal assignment, read only while calm.
        // Fear outranks it exactly as it outranks a detail: a follower that
        // was frightened falls through to its mood behaviour and does not
        // act on the leash this turn, but the assignment is never cleared,
        // so it resumes trailing the moment perception calms it.
        let led_by_player =
            world.actor(id).ai.as_ref().and_then(|ai| ai.following) == Some(world.player);
        let intent = match mood {
            Mood::Relaxed if escorting.is_some() => {
                escort_intent(world, data, id, &escorting.unwrap())
            }
            Mood::Relaxed if led_by_player => follow_intent(world, data, id),
            Mood::Relaxed => routine_intent(world, data, id),
            Mood::Suspicious => watch_intent(world, id),
            Mood::Investigating => investigate_intent(world, data, id),
            Mood::Alerted => pursue_intent(world, data, id),
            Mood::Combat => combat_intent(world, data, id),
            Mood::Fleeing => flee_intent(world, data, id),
            Mood::Escorting => ActionIntent::Wait,
        };
        let remaining = intent_duration(data, world, id, &intent);
        prepared.push(PreparedAction {
            actor: id,
            intent,
            remaining,
        });
    }
    prepared
}

/// Follow the generated routine: walk to the current stop, linger for its
/// wait count, then advance to the next stop, forever.
fn routine_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let (goal, arrived) = {
        let actor = world.actor(id);
        let ai = actor.ai.as_ref().unwrap();
        match ai.routine.get(ai.routine_index) {
            None => return ActionIntent::Wait,
            Some(step) => (step.pos, actor.pos == step.pos),
        }
    };
    if arrived {
        let actor = world.actor_mut(id);
        let ai = actor.ai.as_mut().unwrap();
        if ai.wait_remaining > 0 {
            ai.wait_remaining -= 1;
            return ActionIntent::Wait;
        }
        let step_wait = ai.routine[ai.routine_index].wait;
        ai.routine_index = (ai.routine_index + 1) % ai.routine.len();
        ai.wait_remaining = step_wait;
        advance_schedule(ai);
        return ActionIntent::Wait;
    }
    match first_step_towards(world, data, id, goal) {
        Some(dir) => ActionIntent::Step(dir),
        None => ActionIntent::Wait,
    }
}

/// A person on the player's leash closes on the player and trails one tile
/// behind. The goal is the player's own tile; NPCs never displace the
/// player, so the follower settles on whichever adjacent tile its shortest
/// path reaches and holds there, stepping again only once the player has
/// moved away and opened a gap. Deterministic — a plain shortest-step
/// toward a fixed goal, no RNG and nothing chosen by proximity.
fn follow_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let goal = world.actor(world.player).pos;
    match first_step_towards(world, data, id, goal) {
        Some(dir) => ActionIntent::Step(dir),
        None => ActionIntent::Wait,
    }
}

/// Formation offsets, in a fixed order. Slots are handed out by ascending
/// actor id and read from this table, never chosen by proximity — "the
/// nearest free side" would depend on iteration order and quietly break
/// determinism.
const FORMATION: [Dir4; 4] = [Dir4::North, Dir4::South, Dir4::East, Dir4::West];

/// Where a guard waits while the principal is somewhere it does not
/// follow: on a free tile outside the room the principal is in.
///
/// Each slot gets a *distinct* tile. Handing the whole detail the same
/// post means only one guard can ever stand it and the rest mill about
/// outside pathing at an occupied tile forever.
fn no_follow_post(world: &World, principal: Pos, slot: u8) -> Option<Pos> {
    let room = world.room_at(principal)?;
    let mut candidates: Vec<Pos> = Vec::new();
    for pos in world.map.floor_positions(room.floor) {
        let crate::map::TileKind::Door(id) = world.map.tile(pos) else {
            continue;
        };
        if !room.doors.contains(&id) {
            continue;
        }
        for dir in FORMATION {
            let outside = pos.step(dir);
            if room.bounds.contains(outside.x, outside.y) {
                continue;
            }
            if matches!(world.map.tile(outside), crate::map::TileKind::Floor)
                && world.furniture_at(outside).is_none()
                && !candidates.contains(&outside)
            {
                candidates.push(outside);
            }
            // A second rank, so a three-guard detail is not fighting over
            // the two tiles either side of one door.
            for second in FORMATION {
                let back = outside.step(second);
                if room.bounds.contains(back.x, back.y) {
                    continue;
                }
                if matches!(world.map.tile(back), crate::map::TileKind::Floor)
                    && world.furniture_at(back).is_none()
                    && !candidates.contains(&back)
                {
                    candidates.push(back);
                }
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }
    Some(candidates[usize::from(slot) % candidates.len()])
}

/// The tile this guard should stand on beside its principal.
///
/// A slot is a *preference*, not an entitlement: in a corridor the north
/// and south slots are walls, and a guard that paths at a wall never
/// arrives and never gives up. Slots are therefore tried from the guard's
/// own index onwards, wrapping — a fixed order, so which guard ends up
/// where is still a function of actor id alone and replay is unaffected.
fn formation_tile(world: &World, id: ActorId, principal: Pos, slot: u8) -> Option<Pos> {
    for offset in 0..FORMATION.len() {
        let dir = FORMATION[(usize::from(slot) + offset) % FORMATION.len()];
        let tile = principal.step(dir);
        if !matches!(world.map.tile(tile), crate::map::TileKind::Floor) {
            continue;
        }
        if world.furniture_at(tile).is_some() {
            continue;
        }
        // Another guard already holding this side is fine to walk towards
        // only if it is this guard.
        match world.standing_actor_at(tile) {
            Some(other) if other.id != id => continue,
            _ => return Some(tile),
        }
    }
    None
}

/// Escort the principal: hold a formation slot beside them in public, and
/// stand off at a post while they take a beat guards do not follow.
///
/// Adjacency denial is not implemented here and needs no code: a guard is
/// not displaceable, so a guard standing on a tile beside the principal
/// denies that tile to the player outright. The garrote needs to stand
/// directly behind the target, and the formation occupies exactly those
/// tiles.
fn escort_intent(
    world: &mut World,
    data: &GameData,
    id: ActorId,
    detail: &DetailRole,
) -> ActionIntent {
    let DetailRole::Bodyguard {
        principal, slot, ..
    } = detail;
    let principal = *principal;
    let slot = *slot;
    if !world.actor(principal).alive() || world.actor(principal).departed {
        // Nothing left to guard; fall back to the guard's own routine.
        return routine_intent(world, data, id);
    }

    let principal_pos = world.actor(principal).pos;
    let no_follow = world
        .actor(principal)
        .ai
        .as_ref()
        .and_then(|ai| ai.schedule.as_ref())
        .and_then(|s| s.current())
        .is_some_and(|b| b.no_follow || b.protection == Protection::Alone);

    let goal = if no_follow {
        // Resolve the post once and remember it, so the detail does not
        // drift while the principal is out of sight.
        let (cached, waited) = match world
            .actor(id)
            .ai
            .as_ref()
            .and_then(|ai| ai.detail.as_ref())
        {
            Some(DetailRole::Bodyguard { post, waited, .. }) => (*post, *waited),
            None => (None, 0),
        };

        // Escort-search: a detail waits, but not forever. Once the clock
        // runs out the guard goes in to check on its principal, which is
        // what stops a private beat being an unlimited window and gives
        // the player a reason to hurry rather than simply to wait.
        if waited >= data.tuning.escort_search_turns {
            return match first_step_towards(world, data, id, principal_pos) {
                Some(dir) => ActionIntent::Step(dir),
                None => ActionIntent::Wait,
            };
        }

        // The clock measures how long the *principal* has been out of
        // sight, not how long this guard has had its feet on a particular
        // tile. Tying it to the tile means a guard that cannot reach its
        // post — crowded out, or across a door it must queue for — waits
        // forever and the window never closes.
        let post = cached.or_else(|| no_follow_post(world, principal_pos, slot));
        if let Some(ai) = world.actor_mut(id).ai.as_mut()
            && let Some(DetailRole::Bodyguard {
                post: p, waited: w, ..
            }) = ai.detail.as_mut()
        {
            *p = post;
            *w = w.saturating_add(1);
        }
        match post {
            Some(post) => post,
            None => return ActionIntent::Wait,
        }
    } else {
        // Back in formation; forget any post from the last private beat.
        if let Some(ai) = world.actor_mut(id).ai.as_mut()
            && let Some(DetailRole::Bodyguard {
                post: p, waited: w, ..
            }) = ai.detail.as_mut()
        {
            *p = None;
            *w = 0;
        }
        // While the principal is walking, the detail *trails* instead of
        // ringing. A ring around a moving principal contests its next
        // tile every turn — the winners shuffle hands the tile to a guard
        // about half the time, and the escorted target crawls through its
        // own day at a fraction of its budgeted pace. Trailing guards aim
        // only at tiles the principal has already vacated, so the column
        // moves at full speed; the ring forms again the moment it stops.
        let standing = world
            .actor(principal)
            .ai
            .as_ref()
            .and_then(|ai| ai.routine.get(ai.routine_index))
            .is_none_or(|step| step.pos == principal_pos);
        if standing {
            match formation_tile(world, id, principal_pos, slot) {
                Some(tile) => tile,
                // Nowhere to stand: hold position rather than crowd.
                // Walking at an impossible tile is what a naive slot
                // lookup does, and it strands the guard forever.
                None => return ActionIntent::Wait,
            }
        } else {
            let back = world
                .actor(principal)
                .facing
                .map(|f| f.opposite())
                .unwrap_or(Dir4::South);
            let mut trail = principal_pos;
            for _ in 0..=usize::from(slot) {
                let next = trail.step(back);
                if !matches!(world.map.tile(next), crate::map::TileKind::Floor) {
                    break;
                }
                trail = next;
            }
            if trail == principal_pos {
                // No room behind (a doorway, a corner): hang back where
                // we are rather than crowd the principal's path.
                return ActionIntent::Wait;
            }
            trail
        }
    };

    if world.actor(id).pos == goal {
        return ActionIntent::Wait;
    }
    match first_step_towards(world, data, id, goal) {
        Some(dir) => ActionIntent::Step(dir),
        None => ActionIntent::Wait,
    }
}

/// Keeps the beat index in step with the routine index it mirrors, and
/// honours a pending resume.
///
/// Beats are index-aligned with the routine by construction, but nothing
/// advanced the beat index at runtime until now: `schedule.current()` sat
/// on beat zero forever, so `Protection` was never observed in play and
/// the target was permanently "escorted" as far as any runtime rule could
/// tell. The escort tests passed only because they set the index by hand.
fn advance_schedule(ai: &mut crate::world::AiState) {
    let routine_index = ai.routine_index;
    let Some(schedule) = ai.schedule.as_mut() else {
        return;
    };
    if schedule.beats.is_empty() {
        return;
    }
    // A beat reached by summons hands control back to the interrupted day
    // rather than running on from where it jumped to, so the cycle keeps
    // every beat it had.
    if let Some(resume) = schedule.resume_index.take() {
        schedule.index = resume;
    } else {
        schedule.index = routine_index % schedule.beats.len();
    }
    schedule.dwell_remaining = schedule.beats[schedule.index].dwell;
}

/// Suspicious: stand and face the trouble spot, letting suspicion evolve.
fn watch_intent(world: &World, id: ActorId) -> ActionIntent {
    let actor = world.actor(id);
    let focus = actor.ai.as_ref().unwrap().focus;
    match focus.and_then(|f| Dir4::towards(actor.pos, f)) {
        Some(dir) if actor.facing != Some(dir) => ActionIntent::TurnFacing(dir),
        _ => ActionIntent::Wait,
    }
}

/// Investigating: walk to the remembered spot, look around, then relax.
fn investigate_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let (focus, at_focus) = {
        let actor = world.actor(id);
        let ai = actor.ai.as_ref().unwrap();
        match ai.focus {
            None => {
                // Nothing left to check.
                stand_down(world, id, StandDown::NothingToCheck);
                return ActionIntent::Wait;
            }
            Some(focus) => (focus, actor.pos == focus || actor.pos.is_adjacent(focus)),
        }
    };
    if at_focus {
        let linger_done = {
            let ai = world.actor_mut(id).ai.as_mut().unwrap();
            if ai.wait_remaining == 0 {
                ai.wait_remaining = data.tuning.investigate_linger;
            }
            ai.wait_remaining -= 1;
            ai.wait_remaining == 0
        };
        if linger_done {
            stand_down(world, id, StandDown::Concluded);
            return ActionIntent::Wait;
        }
        // Look around while lingering: rotate the facing.
        let facing = world.actor(id).facing.unwrap_or(Dir4::North);
        return ActionIntent::TurnFacing(next_clockwise(facing));
    }
    match first_step_towards(world, data, id, focus) {
        Some(dir) => ActionIntent::Step(dir),
        None => {
            stand_down(world, id, StandDown::Unreachable);
            ActionIntent::Wait
        }
    }
}

/// Alerted guards pursue and attempt arrest; other alerted actors flee.
fn pursue_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let armed = world
        .actor(id)
        .role
        .and_then(|r| data.role_spec(r))
        .is_some_and(|s| s.armed);
    if !armed {
        return flee_intent(world, data, id);
    }
    let player_pos = world.player_actor().pos;
    let adjacent = world.actor(id).pos.is_adjacent(player_pos);
    if adjacent && world.player_actor().alive() {
        return if world.player_violence_witnessed {
            ActionIntent::MeleeAttack(world.player)
        } else {
            ActionIntent::Arrest(world.player)
        };
    }
    chase_focus(world, data, id)
}

/// Combat: armed actors close and strike lethally.
fn combat_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let armed = world
        .actor(id)
        .role
        .and_then(|r| data.role_spec(r))
        .is_some_and(|s| s.armed);
    if !armed {
        return flee_intent(world, data, id);
    }
    let player_pos = world.player_actor().pos;
    if world.actor(id).pos.is_adjacent(player_pos) && world.player_actor().alive() {
        return ActionIntent::MeleeAttack(world.player);
    }
    chase_focus(world, data, id)
}

/// Walk toward the remembered focus; when it goes stale with nothing
/// found, fall back to the routine while staying on alert.
fn chase_focus(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let focus = world.actor(id).ai.as_ref().unwrap().focus;
    if let Some(focus) = focus {
        if world.actor(id).pos == focus {
            let ai = world.actor_mut(id).ai.as_mut().unwrap();
            ai.focus = None;
            let facing = world.actor(id).facing.unwrap_or(Dir4::North);
            return ActionIntent::TurnFacing(next_clockwise(facing));
        }
        if let Some(dir) = first_step_towards(world, data, id, focus) {
            return ActionIntent::Step(dir);
        }
    }
    // No focus: keep moving through the routine, still alert.
    routine_intent(world, data, id)
}

/// Fleeing actors run for the nearest extraction exit and cower there.
/// Whether this actor still has at least one bodyguard on its feet.
fn has_live_detail(world: &World, principal: ActorId) -> bool {
    world.actors.iter().any(|a| {
        a.alive()
            && !a.departed
            && a.ai
                .as_ref()
                .and_then(|ai| ai.detail.as_ref())
                .is_some_and(|DetailRole::Bodyguard { principal: p, .. }| *p == principal)
    })
}

/// Rooms a principal can be put behind, deepest tier first. Rooms are
/// taken in generation order, never by distance, so the choice does not
/// depend on where the panic started.
fn safe_room_tiles(world: &World, from: Pos) -> Vec<Pos> {
    let mut tiles = Vec::new();
    for zone in [crate::data::Zone::Personal, crate::data::Zone::Secure] {
        for room in world.rooms.iter().filter(|r| r.zone == zone) {
            for w in &room.waypoints {
                if w.pos != from {
                    tiles.push(w.pos);
                }
            }
        }
    }
    tiles
}

fn flee_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let pos = world.actor(id).pos;

    // Evacuate: a principal with a detail left on its feet is not a
    // civilian running for the street — the detail walks them somewhere
    // defensible and further in. This deliberately makes the fire alarm
    // double-edged: it empties the crowd, which helps, and it hardens the
    // target, which does not. If nothing defensible is reachable the
    // principal flees like anyone else, and a target that reaches the
    // street still ends the mission.
    if has_live_detail(world, id) {
        for safe in safe_room_tiles(world, pos) {
            if pos == safe {
                return ActionIntent::Wait;
            }
            if let Some(dir) = first_step_towards(world, data, id, safe) {
                return ActionIntent::Step(dir);
            }
        }
        // Nothing defensible within reach — most of the deep rooms are
        // locked to this principal too. A guarded principal still does not
        // bolt for the street: the detail holds them where they are. Only
        // a principal whose detail is gone runs, which is what keeps the
        // target-escapes-ends-the-mission rule alive without letting a
        // panic hand the player a loss they could not prevent.
        return ActionIntent::Wait;
    }

    let mut exits = world.extraction_tiles.clone();
    exits.sort_by_key(|e| {
        // Storeys apart cost proportionally: with three or more floors a
        // flat penalty would rank the top floor and the next one down as
        // equally far away.
        e.chebyshev(pos).map(i32::from).unwrap_or(i32::MAX / 2)
            + i32::from(e.floor.abs_diff(pos.floor)) * 100
    });
    for exit in exits {
        if pos == exit {
            return ActionIntent::Wait;
        }
        if let Some(dir) = first_step_towards(world, data, id, exit) {
            return ActionIntent::Step(dir);
        }
    }
    ActionIntent::Wait
}

fn next_clockwise(dir: Dir4) -> Dir4 {
    match dir {
        Dir4::North => Dir4::East,
        Dir4::East => Dir4::South,
        Dir4::South => Dir4::West,
        Dir4::West => Dir4::North,
    }
}

/// True when an actor's hands are usable for a new weapon or body action.
pub fn hands_free(world: &World, id: ActorId) -> bool {
    world.actor(id).hands == Hands::Free
}

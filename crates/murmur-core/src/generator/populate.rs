//! Population: actors, routines, and item placement.

use crate::data::{GameData, Role, WaypointKind};
use crate::geom::{Dir4, Pos};
use crate::rng::Pcg32;
use crate::world::{
    Actor, ActorId, AiState, BodyCondition, Hands, ItemId, ItemInstance, ItemLocation, Mood, Room,
    RoutineStep, Waypoint,
};

use super::layout::{Layout, waypoints_of_kinds};

pub struct Population {
    pub actors: Vec<Actor>,
    pub items: Vec<ItemInstance>,
    pub player: ActorId,
    pub target: ActorId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopulateError(pub String);

/// Whether every door into this room is locked. Zone permission says an
/// NPC is *allowed* in a room; it says nothing about whether they can open
/// the door, and only one member of a role carries any given key. A
/// routine stop nobody can walk to is a stuck NPC, so sealed rooms are
/// kept out of routine pools entirely and stay quiet — which is what a
/// locked room should be.
fn is_sealed(layout: &Layout, room: &Room) -> bool {
    !room.doors.is_empty()
        && room
            .doors
            .iter()
            .all(|d| layout.doors[d.0 as usize].locked_by.is_some())
}

/// Rooms an NPC in `disguise` may legitimately spend time in. Mirrors the
/// access rules: zone from the permission matrix, plus authored extra
/// rooms, minus anything sealed behind a lock.
fn rooms_for_disguise<'a>(
    data: &GameData,
    layout: &'a Layout,
    disguise: &str,
    with_invitation: bool,
) -> Vec<&'a Room> {
    let Some(spec) = data.disguise(disguise) else {
        return Vec::new();
    };
    layout
        .rooms
        .iter()
        .filter(|room| {
            (spec.zones.contains(&room.zone)
                || spec.extra_rooms.contains(&room.template)
                || (with_invitation
                    && spec.secure_with_invitation
                    && room.zone == crate::data::Zone::Secure))
                && !is_sealed(layout, room)
        })
        .collect()
}

fn generated_name(data: &GameData, rng: &mut Pcg32) -> String {
    format!(
        "{} {}",
        rng.pick(&data.names.first),
        rng.pick(&data.names.last)
    )
}

/// Builds a routine of `steps` waypoints drawn from `pool`, avoiding
/// immediate repeats where possible.
fn build_routine(
    pool: &[Waypoint],
    steps: usize,
    wait_min: u16,
    wait_max: u16,
    rng: &mut Pcg32,
) -> Vec<RoutineStep> {
    let mut routine = Vec::new();
    let mut last: Option<Pos> = None;
    for _ in 0..steps {
        if pool.is_empty() {
            break;
        }
        let mut waypoint = *rng.pick(pool);
        if pool.len() > 1 {
            for _ in 0..4 {
                if Some(waypoint.pos) != last {
                    break;
                }
                waypoint = *rng.pick(pool);
            }
        }
        last = Some(waypoint.pos);
        routine.push(RoutineStep {
            pos: waypoint.pos,
            wait: rng.range_inclusive(wait_min.into(), wait_max.into()) as u16,
        });
    }
    routine
}

/// A free standing spot for spawning: the preferred tile or the nearest
/// plain-floor, unoccupied tile in a deterministic outward scan. Nobody
/// spawns in a doorway or on the stairs.
fn spawn_spot(preferred: Pos, layout: &Layout, taken: &[Pos]) -> Pos {
    let open = |pos: Pos| {
        matches!(layout.map.tile(pos), crate::map::TileKind::Floor)
            && !taken.contains(&pos)
            && !layout.furniture.iter().any(|f| f.pos == pos)
    };
    if open(preferred) {
        return preferred;
    }
    for radius in 1..8i16 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let pos = Pos::new(preferred.floor, preferred.x + dx, preferred.y + dy);
                if open(pos) {
                    return pos;
                }
            }
        }
    }
    preferred
}

pub fn populate(
    data: &GameData,
    layout: &Layout,
    venue: &crate::data::VenueSpec,
    constraint: Option<&crate::contract::Constraint>,
    loadout: &[String],
    heat: u8,
    rng: &mut Pcg32,
) -> Result<Population, PopulateError> {
    let mut actors: Vec<Actor> = Vec::new();
    let mut items: Vec<ItemInstance> = Vec::new();
    let mut taken: Vec<Pos> = Vec::new();

    let entrance_exit = layout
        .extraction_tiles
        .first()
        .copied()
        .ok_or_else(|| PopulateError("no extraction tiles were generated".into()))?;

    // Actor 0: the player. Civilian clothes, no facing, no role.
    let player_id = ActorId(0);
    let player_pos = spawn_spot(entrance_exit, layout, &taken);
    taken.push(player_pos);
    actors.push(Actor {
        id: player_id,
        name: "you".to_string(),
        role: None,
        pos: player_pos,
        facing: None,
        condition: BodyCondition::Healthy,
        hp: data.tuning.player_max_hp,
        worn_disguise: "civilian".to_string(),
        hands: Hands::Free,
        crouched: false,
        is_target: false,
        is_vip: false,
        ai: None,
        hidden_in: None,
        departed: false,
        killed_by_player: false,
        discovery_counted: false,
    });

    // The target: staff model with a richer generated schedule.
    let target_id = ActorId(actors.len() as u32);
    let mut target_keys: Vec<String> = Vec::new();
    {
        let spec = &data.population.target;
        let disguise = spec.disguise.clone().unwrap_or_else(|| "staff".to_string());
        let rooms = rooms_for_disguise(data, layout, &disguise, false);
        let public_stops: Vec<Pos> = waypoints_of_kinds(
            rooms
                .iter()
                .copied()
                .filter(|r| r.zone == crate::data::Zone::Public),
            &[WaypointKind::Work, WaypointKind::Idle, WaypointKind::Social],
        )
        .iter()
        .map(|w| w.pos)
        .collect();

        // The target's day is a cycle of beats: escorted in public, alone
        // in private. A private-kill contract narrows where the alone beat
        // may fall, so it is incorporated before generation rather than
        // checked afterwards.
        let private_kill = matches!(constraint, Some(crate::contract::Constraint::PrivateKill));
        let schedule =
            super::schedule::build_schedule(data, layout, &public_stops, private_kill, rng)
                .map_err(|e| PopulateError(e.0))?;
        let routine = super::schedule::routine_for(&schedule);
        super::schedule::assert_aligned(&schedule, &routine).map_err(|e| PopulateError(e.0))?;

        // A person is not locked out of the rooms their own day takes them
        // through. This is the only key granted by need rather than by
        // role, and it makes the target's key worth lifting.
        for beat in &schedule.beats {
            for room in layout
                .rooms
                .iter()
                .filter(|r| r.floor == beat.pos.floor && r.bounds.contains(beat.pos.x, beat.pos.y))
            {
                for door in &room.doors {
                    if let Some(key) = &layout.doors[door.0 as usize].locked_by
                        && !target_keys.contains(key)
                    {
                        target_keys.push(key.clone());
                    }
                }
            }
        }

        let preferred = routine
            .first()
            .map(|s| s.pos)
            .ok_or_else(|| PopulateError("target routine is empty".into()))?;
        let pos = spawn_spot(preferred, layout, &taken);
        taken.push(pos);
        // The target is drawn from the staff model: a staff role with a
        // richer schedule than ordinary staff.
        let staff_roles = [Role::Bartender, Role::Cleaner, Role::Technician];
        actors.push(Actor {
            id: target_id,
            name: generated_name(data, rng),
            role: Some(*rng.pick(&staff_roles)),
            pos,
            facing: Some(*rng.pick(&Dir4::ALL)),
            condition: BodyCondition::Healthy,
            hp: 1,
            worn_disguise: disguise,
            hands: Hands::Free,
            crouched: false,
            is_target: true,
            is_vip: false,
            ai: Some(AiState {
                routine,
                routine_index: 0,
                wait_remaining: 0,
                mood: Mood::Relaxed,
                suspicion: 0,
                focus: None,
                knows_player_hostile: false,
                schedule: Some(schedule),
                detail: None,
            }),
            hidden_in: None,
            departed: false,
            killed_by_player: false,
            discovery_counted: false,
        });
    }

    // Role populations.
    let mut vip_civilians = rng.range_inclusive(
        data.population.vip_civilians_min.into(),
        data.population.vip_civilians_max.into(),
    );
    for role_spec in &data.population.roles {
        // Venue overrides shape the crowd; the role behaviour is shared.
        let (count_min, count_max) = venue
            .role_counts
            .iter()
            .find(|o| o.role == role_spec.role)
            .map(|o| (o.count_min, o.count_max))
            .unwrap_or((role_spec.count_min, role_spec.count_max));
        let mut count = rng.range_inclusive(count_min.into(), count_max.into());
        // Persistent district heat hardens the venue: extra guards on
        // shift, capped so an area never locks out.
        if role_spec.role == Role::Guard {
            count += u32::from(heat.min(data.tuning.heat_extra_guard_cap));
        }
        for _ in 0..count {
            let is_vip = role_spec.role == Role::Civilian && vip_civilians > 0;
            if is_vip {
                vip_civilians -= 1;
            }
            let disguise = role_spec
                .disguise
                .clone()
                .unwrap_or_else(|| "civilian".to_string());
            let rooms = rooms_for_disguise(data, layout, &disguise, is_vip);
            let mut pool = waypoints_of_kinds(rooms.iter().copied(), &role_spec.waypoint_kinds);
            if pool.is_empty() {
                pool = waypoints_of_kinds(
                    rooms.iter().copied(),
                    &[
                        WaypointKind::Social,
                        WaypointKind::Work,
                        WaypointKind::Patrol,
                        WaypointKind::Idle,
                    ],
                );
            }
            if is_vip {
                // VIP guests spend most of their time in the lounge.
                let vip_pool: Vec<Waypoint> = waypoints_of_kinds(
                    rooms
                        .iter()
                        .copied()
                        .filter(|r| r.zone == crate::data::Zone::Secure),
                    &[WaypointKind::Social, WaypointKind::Idle],
                );
                if !vip_pool.is_empty() {
                    pool.extend(vip_pool.iter().copied());
                    pool.extend(vip_pool.iter().copied());
                }
            }
            let steps = rng.range_inclusive(3, 5) as usize;
            let routine = build_routine(&pool, steps, role_spec.wait_min, role_spec.wait_max, rng);
            let preferred = routine.first().map(|s| s.pos).unwrap_or(entrance_exit);
            let pos = spawn_spot(preferred, layout, &taken);
            taken.push(pos);
            let id = ActorId(actors.len() as u32);
            actors.push(Actor {
                id,
                name: generated_name(data, rng),
                role: Some(role_spec.role),
                pos,
                facing: Some(*rng.pick(&Dir4::ALL)),
                condition: BodyCondition::Healthy,
                hp: if role_spec.armed { 2 } else { 1 },
                worn_disguise: disguise,
                hands: Hands::Free,
                crouched: false,
                is_target: false,
                is_vip,
                ai: Some(AiState {
                    routine,
                    routine_index: 0,
                    wait_remaining: 0,
                    mood: Mood::Relaxed,
                    suspicion: 0,
                    focus: None,
                    knows_player_hostile: false,
                    schedule: None,
                    detail: None,
                }),
                hidden_in: None,
                departed: false,
                killed_by_player: false,
                discovery_counted: false,
            });
        }
    }

    // The player's loadout: campaign equipment carried in, charged per
    // its spec (pistol rounds, noisemaker uses).
    for spec_id in loadout {
        let spec = data
            .item(spec_id)
            .ok_or_else(|| PopulateError(format!("loadout item '{spec_id}' is unknown")))?;
        items.push(ItemInstance {
            id: ItemId(items.len() as u32),
            spec: spec.id.clone(),
            location: ItemLocation::CarriedBy(player_id),
            charges: spec.charges,
        });
    }

    // The target's detail. Guards are picked in ascending actor id and
    // handed formation slots in order — never "the nearest guards", which
    // would depend on iteration order and break replay. Guards are not
    // displaceable, so a detail in formation denies the player the tiles
    // beside the target without a line of movement code.
    let escort_slots = usize::from(data.tuning.escort_slots);
    let guards: Vec<ActorId> = actors
        .iter()
        .filter(|a| a.role == Some(Role::Guard))
        .map(|a| a.id)
        .take(escort_slots)
        .collect();
    let mut detail: Vec<ActorId> = Vec::new();
    let target_pos = actors[target_id.0 as usize].pos;
    for (slot, guard) in guards.into_iter().enumerate() {
        if let Some(ai) = actors[guard.0 as usize].ai.as_mut() {
            ai.detail = Some(crate::world::DetailRole::Bodyguard {
                principal: target_id,
                slot: slot as u8,
                post: None,
                waited: 0,
            });
            detail.push(guard);
        }
        // Spawn the detail already in formation. Left to walk there from
        // wherever population dropped them, the guards need dozens of
        // turns to close, and the target stands unescorted through exactly
        // the opening minutes when the player is nearest the entrance —
        // which made every protection rule downstream irrelevant in play.
        let ring: Vec<Pos> = Dir4::ALL
            .into_iter()
            .map(|d| target_pos.step(d))
            .filter(|p| {
                matches!(layout.map.tile(*p), crate::map::TileKind::Floor)
                    && !layout.furniture.iter().any(|f| f.pos == *p)
            })
            .collect();
        if let Some(spot) = ring.iter().find(|p| !taken.contains(p)) {
            let spot = *spot;
            actors[guard.0 as usize].pos = spot;
            taken.push(spot);
        }
    }

    // World items. Explicit generation rules per spec kind:
    //  - purchasable equipment never generates into the venue;
    //  - invitations: one per tagged VIP civilian;
    //  - keys: carried by one deterministic-random member of their role,
    //    or placed on the floor of a staff room if the role is absent.
    for item_spec in &data.items {
        if item_spec.purchasable {
            continue;
        }
        if item_spec.invitation {
            let vip_ids: Vec<ActorId> = actors.iter().filter(|a| a.is_vip).map(|a| a.id).collect();
            for vip in vip_ids {
                items.push(ItemInstance {
                    id: ItemId(items.len() as u32),
                    spec: item_spec.id.clone(),
                    location: ItemLocation::CarriedBy(vip),
                    charges: 0,
                });
            }
            continue;
        }
        if let Some(role) = item_spec.carried_by {
            let holders: Vec<ActorId> = actors
                .iter()
                .filter(|a| a.role == Some(role))
                .map(|a| a.id)
                .collect();
            if holders.is_empty() {
                return Err(PopulateError(format!(
                    "item '{}' needs a {} to carry it and none was generated",
                    item_spec.id,
                    role.name()
                )));
            }
            items.push(ItemInstance {
                id: ItemId(items.len() as u32),
                spec: item_spec.id.clone(),
                location: ItemLocation::CarriedBy(*rng.pick(&holders)),
                charges: 0,
            });
            // A detail can follow its principal anywhere the principal
            // goes, so it carries the same keys. Without this a bodyguard
            // simply cannot reach a locked private beat — it stands across
            // the building pathing at a door it may not open, and the
            // escort-search clock runs out with nobody able to act on it.
            // It also puts those keys on someone the player can follow and
            // pickpocket, which is the intended way in.
            if target_keys.contains(&item_spec.id) {
                for guard in &detail {
                    items.push(ItemInstance {
                        id: ItemId(items.len() as u32),
                        spec: item_spec.id.clone(),
                        location: ItemLocation::CarriedBy(*guard),
                        charges: 0,
                    });
                }
            }
            // The target carries its *own copy* of any key its day needs.
            // It must be a copy: the role's key is the one the player can
            // plan around, and moving it onto the target would strand it
            // inside the very room it opens, since the target is only
            // pickpocketable at an alone beat — which is in that room.
            if target_keys.contains(&item_spec.id) {
                items.push(ItemInstance {
                    id: ItemId(items.len() as u32),
                    spec: item_spec.id.clone(),
                    location: ItemLocation::CarriedBy(target_id),
                    charges: 0,
                });
            }
        } else if let Some(room_template) = &item_spec.placed_in {
            let candidates: Vec<Pos> = layout
                .rooms
                .iter()
                .filter(|r| &r.template == room_template)
                .flat_map(|r| r.waypoints.iter().map(|w| w.pos))
                .collect();
            if candidates.is_empty() {
                return Err(PopulateError(format!(
                    "item '{}' has nowhere to be placed",
                    item_spec.id
                )));
            }
            items.push(ItemInstance {
                id: ItemId(items.len() as u32),
                spec: item_spec.id.clone(),
                location: ItemLocation::Ground(*rng.pick(&candidates)),
                charges: 0,
            });
        }
    }

    Ok(Population {
        actors,
        items,
        player: player_id,
        target: target_id,
    })
}

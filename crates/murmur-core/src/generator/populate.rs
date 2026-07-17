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

/// Rooms an NPC in `disguise` may legitimately spend time in. Mirrors the
/// access rules: zone from the permission matrix, plus authored extra
/// rooms.
fn rooms_for_disguise<'a>(
    data: &GameData,
    rooms: &'a [Room],
    disguise: &str,
    with_invitation: bool,
) -> Vec<&'a Room> {
    let Some(spec) = data.disguise(disguise) else {
        return Vec::new();
    };
    rooms
        .iter()
        .filter(|room| {
            spec.zones.contains(&room.zone)
                || spec.extra_rooms.contains(&room.template)
                || (with_invitation
                    && spec.secure_with_invitation
                    && room.zone == crate::data::Zone::Secure)
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
    {
        let spec = &data.population.target;
        let disguise = spec.disguise.clone().unwrap_or_else(|| "staff".to_string());
        let rooms = rooms_for_disguise(data, &layout.rooms, &disguise, false);
        let pool = waypoints_of_kinds(
            rooms.iter().copied(),
            &[WaypointKind::Work, WaypointKind::Idle, WaypointKind::Social],
        );
        let steps = rng.range_inclusive(spec.schedule_min.into(), spec.schedule_max.into());
        let mut routine = build_routine(&pool, steps as usize, 6, 14, rng);

        // A private-kill contract is incorporated before generation: the
        // target's schedule must visit personal-tier space.
        if matches!(constraint, Some(crate::contract::Constraint::PrivateKill)) {
            let visits_personal = |step: &RoutineStep| {
                layout.rooms.iter().any(|r| {
                    r.zone == crate::data::Zone::Personal
                        && r.floor == step.pos.floor
                        && r.bounds.contains(step.pos.x, step.pos.y)
                })
            };
            if !routine.iter().any(visits_personal) {
                let personal_pool = waypoints_of_kinds(
                    layout
                        .rooms
                        .iter()
                        .filter(|r| r.zone == crate::data::Zone::Personal),
                    &[WaypointKind::Work, WaypointKind::Idle, WaypointKind::Social],
                );
                if personal_pool.is_empty() {
                    return Err(PopulateError(
                        "private-kill contract but no personal-tier waypoints".into(),
                    ));
                }
                let stop = rng.pick(&personal_pool);
                routine.push(RoutineStep {
                    pos: stop.pos,
                    wait: rng.range_inclusive(8, 16) as u16,
                });
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
        let mut count = rng.range_inclusive(role_spec.count_min.into(), role_spec.count_max.into());
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
            let rooms = rooms_for_disguise(data, &layout.rooms, &disguise, is_vip);
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

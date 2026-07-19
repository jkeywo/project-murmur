//! The generated reachability proof.
//!
//! Two obligations from the foundation:
//!
//! 1. **Physical**: every room is physically reachable from every other
//!    room (ignoring locks and access rules).
//! 2. **Dependency-safe access**: starting as a civilian at the entrance
//!    with no items, there is a progression — disguises taken from actors
//!    whose schedules pass through areas you can already reach, or from
//!    wardrobes; keys likewise — that eventually reaches every room. A
//!    required key source must be reachable without that key; a required
//!    disguise must be obtainable without already wearing it. When a
//!    disguise has no obtainable source, the generator may add a wardrobe
//!    in an already-reachable room that allows one.
//!
//! The proof is a fixpoint closure over (reachable tiles, owned disguises,
//! owned keys, invitation), patched with wardrobes until it covers every
//! room or fails the generation attempt.

use crate::data::GameData;
use crate::geom::{Dir4, Pos};
use crate::map::TileKind;
use crate::rng::Pcg32;
use crate::world::{Actor, FurnitureKind, ItemInstance, ItemLocation, Room};

use super::layout::{Layout, insert_wardrobe};
use super::populate::Population;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofError(pub String);

/// What the proof established, kept on the world for tests and audits.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProofReport {
    pub obtainable_disguises: Vec<String>,
    pub obtainable_keys: Vec<String>,
    /// (room name, disguise) pairs for wardrobes the proof added.
    pub wardrobes_added: Vec<(String, String)>,
}

/// How movement is gated during a reachability closure.
#[derive(Clone, Copy)]
pub(crate) enum AccessModel<'a> {
    /// Physical connectivity: locks and zones ignored.
    Free,
    /// Trespass allowed, locks respected: the physical-stealth and
    /// violence route model. Lockpicks defeat any lock.
    LocksOnly { keys: &'a [String], lockpicks: bool },
    /// Full legitimacy: zones by disguise, locks by key, invitation for
    /// the secure tier, forged pass for the staff tier. The social route
    /// model (picking locks in view is never legitimate).
    Legitimate {
        disguises: &'a [String],
        keys: &'a [String],
        invitation: bool,
        staff_pass: bool,
    },
}

pub(crate) struct TileSet {
    width: i16,
    height: i16,
    bits: Vec<bool>,
}

impl TileSet {
    fn new(width: u16, height: u16, floors: u8) -> Self {
        Self {
            width: width as i16,
            height: height as i16,
            bits: vec![false; usize::from(width) * usize::from(height) * usize::from(floors)],
        }
    }

    fn index(&self, pos: Pos) -> Option<usize> {
        if pos.x < 0 || pos.y < 0 || pos.x >= self.width || pos.y >= self.height {
            return None;
        }
        Some(
            (usize::from(pos.floor) * usize::try_from(self.height).unwrap()
                + usize::try_from(pos.y).unwrap())
                * usize::try_from(self.width).unwrap()
                + usize::try_from(pos.x).unwrap(),
        )
    }

    pub(crate) fn contains(&self, pos: Pos) -> bool {
        self.index(pos).map(|i| self.bits[i]).unwrap_or(false)
    }

    fn insert(&mut self, pos: Pos) -> bool {
        match self.index(pos) {
            Some(i) if !self.bits[i] => {
                self.bits[i] = true;
                true
            }
            _ => false,
        }
    }
}

/// Every position an actor's schedule touches (spawn plus routine stops).
pub(crate) fn schedule_positions(actor: &Actor) -> Vec<Pos> {
    let mut positions = vec![actor.pos];
    if let Some(ai) = &actor.ai {
        positions.extend(ai.routine.iter().map(|s| s.pos));
    }
    positions
}

/// Breadth-first reachability from `start` under an access model.
pub(crate) fn reachable_tiles(
    data: &GameData,
    layout: &Layout,
    start: Pos,
    access: AccessModel<'_>,
) -> TileSet {
    let mut seen = TileSet::new(
        layout.map.width(),
        layout.map.height(),
        layout.map.floor_count(),
    );
    let lock_permits = |pos: Pos, keys: &[String]| -> bool {
        if let TileKind::Door(id) = layout.map.tile(pos)
            && let Some(key) = &layout.doors[id.0 as usize].locked_by
            && !keys.contains(key)
            && !keys
                .iter()
                .any(|k| data.item(k).is_some_and(|s| s.master_key))
        {
            return false;
        }
        true
    };
    let permits = |pos: Pos| -> bool {
        match access {
            AccessModel::Free => true,
            AccessModel::LocksOnly { keys, lockpicks } => lockpicks || lock_permits(pos, keys),
            AccessModel::Legitimate {
                disguises,
                keys,
                invitation,
                staff_pass,
            } => {
                if !lock_permits(pos, keys) {
                    return false;
                }
                // Room interiors demand zone permission from some owned
                // disguise.
                let Some(room) = layout
                    .rooms
                    .iter()
                    .find(|r| r.floor == pos.floor && r.bounds.contains(pos.x, pos.y))
                else {
                    return true;
                };
                (staff_pass && room.zone == crate::data::Zone::Staff)
                    || disguises.iter().any(|d| {
                        data.disguise(d).is_some_and(|spec| {
                            spec.zones.contains(&room.zone)
                                || spec.extra_rooms.contains(&room.template)
                                || (invitation
                                    && spec.secure_with_invitation
                                    && room.zone == crate::data::Zone::Secure)
                        })
                    })
            }
        }
    };
    let passable = |pos: Pos| -> bool {
        match layout.map.tile(pos) {
            TileKind::Wall | TileKind::Void => false,
            TileKind::Floor | TileKind::Stairs(_) | TileKind::Door(_) => {
                layout.furniture.iter().all(|f| f.pos != pos) && permits(pos)
            }
        }
    };

    let mut frontier = std::collections::VecDeque::new();
    if passable(start) && seen.insert(start) {
        frontier.push_back(start);
    }
    while let Some(pos) = frontier.pop_front() {
        for dir in Dir4::ALL {
            let next = layout.map.resolve_step_destination(pos.step(dir));
            if passable(next) && seen.insert(next) {
                frontier.push_back(next);
            }
        }
    }
    seen
}

fn room_reachable(room: &Room, seen: &TileSet) -> bool {
    let b = room.bounds;
    (b.y..b.y + b.h)
        .flat_map(|y| (b.x..b.x + b.w).map(move |x| Pos::new(room.floor, x, y)))
        .any(|pos| seen.contains(pos))
}

/// Runs the physical proof: all rooms and stairs mutually connected when
/// locks and access rules are ignored.
pub fn prove_physical(data: &GameData, layout: &Layout, start: Pos) -> Result<(), ProofError> {
    let seen = reachable_tiles(data, layout, start, AccessModel::Free);
    for room in &layout.rooms {
        if !room_reachable(room, &seen) {
            return Err(ProofError(format!(
                "room '{}' is physically unreachable",
                room.name
            )));
        }
    }
    for pos in &layout.extraction_tiles {
        if !seen.contains(*pos) {
            return Err(ProofError(format!(
                "extraction tile {pos:?} is physically unreachable"
            )));
        }
    }
    Ok(())
}

/// The result of a capability closure: everything obtainable, the tiles
/// reachable with it, and a narrated acquisition order.
pub(crate) struct ClosureOutcome {
    pub disguises: Vec<String>,
    pub keys: Vec<String>,
    pub seen: TileSet,
    /// Human-readable acquisition steps, in dependency order.
    pub events: Vec<String>,
}

fn room_name_at(layout: &Layout, pos: Pos) -> String {
    layout
        .rooms
        .iter()
        .find(|r| r.floor == pos.floor && r.bounds.contains(pos.x, pos.y))
        .map(|r| r.name.clone())
        .unwrap_or_else(|| crate::tr!("room.corridor.name").to_string())
}

/// Fixpoint closure over (reachable tiles, disguises, keys, invitation).
/// With `zone_free` the movement model is trespass-with-locks (the
/// physical and violence route model); otherwise full legitimacy (the
/// social route model).
pub(crate) fn capability_closure(
    data: &GameData,
    layout: &Layout,
    population: &Population,
    start: Pos,
    zone_free: bool,
    assume_catalogue: bool,
) -> ClosureOutcome {
    let mut disguises: Vec<String> = vec!["civilian".to_string()];
    let mut keys: Vec<String> = Vec::new();
    let mut invitation = false;
    let mut events: Vec<String> = Vec::new();

    // Loadout capabilities the player walks in with.
    let player_kit = |flag: fn(&crate::data::ItemSpec) -> bool| {
        population.items.iter().any(|item| {
            item.location == ItemLocation::CarriedBy(population.player)
                && data.item(&item.spec).is_some_and(flag)
        })
    };
    let catalogue_kit = |flag: fn(&crate::data::ItemSpec) -> bool| {
        assume_catalogue && data.items.iter().any(|s| s.purchasable && flag(s))
    };
    let lockpicks = player_kit(|s| s.lockpick) || catalogue_kit(|s| s.lockpick);
    let staff_pass = player_kit(|s| s.staff_pass) || catalogue_kit(|s| s.staff_pass);
    if player_kit(|s| s.invitation) || catalogue_kit(|s| s.invitation) {
        invitation = true;
    }

    loop {
        let access = if zone_free {
            AccessModel::LocksOnly {
                keys: &keys,
                lockpicks,
            }
        } else {
            AccessModel::Legitimate {
                disguises: &disguises,
                keys: &keys,
                invitation,
                staff_pass,
            }
        };
        let seen = reachable_tiles(data, layout, start, access);
        let mut grew = false;

        // Disguises from actors whose schedules cross reachable space.
        for actor in &population.actors {
            if actor.is_player() {
                continue;
            }
            if schedule_positions(actor)
                .iter()
                .any(|pos| seen.contains(*pos))
                && !disguises.contains(&actor.worn_disguise)
            {
                events.push(format!(
                    "take the {} disguise from {}",
                    actor.worn_disguise, actor.name
                ));
                disguises.push(actor.worn_disguise.clone());
                grew = true;
            }
        }
        // Disguises from wardrobes in reachable rooms.
        for furniture in &layout.furniture {
            if furniture.kind == FurnitureKind::Wardrobe
                && let Some(disguise) = &furniture.disguise
                && !disguises.contains(disguise)
                && Dir4::ALL
                    .into_iter()
                    .any(|d| seen.contains(furniture.pos.step(d)))
            {
                events.push(format!(
                    "take the {disguise} disguise from the wardrobe in {}",
                    room_name_at(layout, furniture.pos)
                ));
                disguises.push(disguise.clone());
                grew = true;
            }
        }
        // Key-cache machines dispense their key when reachable.
        for furniture in &layout.furniture {
            if furniture.kind == FurnitureKind::Machine
                && let Some(spec) = furniture
                    .machine
                    .as_deref()
                    .and_then(|s| data.opportunity(s))
                && let crate::data::OpportunityEffect::PlaceKey { item } = &spec.effect
                && !keys.contains(item)
                && Dir4::ALL
                    .into_iter()
                    .any(|d| seen.contains(furniture.pos.step(d)))
            {
                events.push(format!(
                    "lift the {} from the {}",
                    data.item(item).map(|s| s.name.as_str()).unwrap_or(item),
                    spec.name
                ));
                keys.push(item.clone());
                grew = true;
            }
        }
        // Keys and invitations from carriers and the ground.
        for item in &population.items {
            let spec = data.item(&item.spec).expect("item specs validated at load");
            let (obtainable, source) = match item.location {
                ItemLocation::Ground(pos) => (
                    seen.contains(pos),
                    format!("in {}", room_name_at(layout, pos)),
                ),
                ItemLocation::CarriedBy(holder) => {
                    let holder = &population.actors[holder.0 as usize];
                    (
                        !holder.is_player()
                            && schedule_positions(holder)
                                .iter()
                                .any(|pos| seen.contains(*pos)),
                        format!("from {}", holder.name),
                    )
                }
            };
            if !obtainable {
                continue;
            }
            if spec.unlocks.is_some() && !keys.contains(&spec.id) {
                events.push(format!("lift the {} {source}", spec.name));
                keys.push(spec.id.clone());
                grew = true;
            }
            if spec.invitation && !invitation {
                events.push(format!("lift the {} {source}", spec.name));
                invitation = true;
                grew = true;
            }
        }

        if !grew {
            return ClosureOutcome {
                disguises,
                keys,
                seen,
                events,
            };
        }
    }
}

/// Proves dependency-safe access, adding wardrobes where a disguise has no
/// obtainable source, and returns the proof report.
pub fn prove_progression(
    data: &GameData,
    layout: &mut Layout,
    population: &Population,
    start: Pos,
    rng: &mut Pcg32,
) -> Result<ProofReport, ProofError> {
    let mut wardrobes_added: Vec<(String, String)> = Vec::new();

    for _attempt in 0..=data.disguises.len() {
        let outcome = capability_closure(data, layout, population, start, false, false);
        let (disguises, keys, seen) = (outcome.disguises, outcome.keys, outcome.seen);

        let unreachable: Vec<&Room> = layout
            .rooms
            .iter()
            .filter(|room| !room_reachable(room, &seen))
            .collect();
        if unreachable.is_empty() {
            return Ok(ProofReport {
                obtainable_disguises: disguises,
                obtainable_keys: keys,
                wardrobes_added,
            });
        }

        // Find a disguise that would unlock an unreachable room and is not
        // yet obtainable, then stock a wardrobe with it in reachable space.
        let mut patched = false;
        for room in &unreachable {
            let candidates: Vec<&crate::data::DisguiseSpec> = data
                .disguises
                .iter()
                .filter(|spec| {
                    !disguises.contains(&spec.id)
                        && (spec.zones.contains(&room.zone)
                            || spec.extra_rooms.contains(&room.template))
                })
                .collect();
            let Some(disguise) = candidates.first() else {
                continue;
            };
            let wardrobe_rooms: Vec<usize> = layout
                .rooms
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    data.room_template(&r.template)
                        .is_some_and(|t| t.wardrobe_allowed)
                        && room_reachable(r, &seen)
                })
                .map(|(index, _)| index)
                .collect();
            for room_index in wardrobe_rooms {
                let room = layout.rooms[room_index].clone();
                let extraction = layout.extraction_tiles.clone();
                if insert_wardrobe(
                    &room,
                    &layout.map,
                    &extraction,
                    &mut layout.furniture,
                    &disguise.id,
                    rng,
                ) {
                    wardrobes_added.push((room.name.clone(), disguise.id.clone()));
                    patched = true;
                    break;
                }
            }
            if patched {
                break;
            }
        }

        if !patched {
            let names: Vec<&str> = unreachable.iter().map(|r| r.name.as_str()).collect();
            return Err(ProofError(format!(
                "no progression reaches: {} (and no wardrobe placement can fix it)",
                names.join(", ")
            )));
        }
    }

    Err(ProofError(
        "wardrobe patching failed to converge".to_string(),
    ))
}

/// Convenience for tests: the items a population holds, by spec id.
pub fn item_spec_ids(items: &[ItemInstance]) -> Vec<&str> {
    items.iter().map(|i| i.spec.as_str()).collect()
}

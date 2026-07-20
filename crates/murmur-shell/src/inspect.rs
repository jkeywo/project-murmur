//! Inspection prose: the one-line descriptions the inspection line shows
//! for a tile or an inventory slot. Pure text-building over world state —
//! it costs no turn and produces no simulation action, following the
//! precedent look set.

use murmur_core::data::GameData;
use murmur_core::geom::Pos;
use murmur_core::map::TileKind;
use murmur_core::world::{FurnitureKind, World};
use murmur_core::{tr, trf};

/// A one-line description of an inspected tile, honest about what the
/// player can currently see. `explored` is whether the player has ever
/// seen the tile; unseen and unexplored tiles stay a mystery.
pub(crate) fn describe(
    world: &World,
    data: &GameData,
    pos: Pos,
    visible: bool,
    explored: bool,
) -> String {
    if !visible && !explored {
        return tr!("ui.mission.tile.unseen").to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(room) = world.room_at(pos) {
        let zone_label = data
            .venue(&world.venue)
            .map(|v| v.zone_label(room.zone))
            .unwrap_or_else(|| room.zone.name());
        parts.push(murmur_core::loc::fmt(
            "ui.mission.tile.room",
            &[("room", &room.name), ("zone", zone_label)],
        ));
    } else if matches!(world.map.tile(pos), TileKind::Floor | TileKind::Stairs(_)) {
        parts.push(tr!("ui.mission.tile.corridor").to_string());
    }
    match world.map.tile(pos) {
        TileKind::Wall => parts.push(tr!("ui.mission.tile.wall").to_string()),
        TileKind::Stairs(_) => parts.push(tr!("ui.mission.tile.stairs").to_string()),
        TileKind::Door(id) => {
            let door = world.door(id);
            let state = if door.open {
                tr!("ui.mission.tile.door.open")
            } else {
                tr!("ui.mission.tile.door.closed")
            };
            if door.locked_by.is_some() {
                parts.push(trf!("ui.mission.tile.door.locked", state = state));
            } else {
                parts.push(state.to_string());
            }
        }
        _ => {}
    }
    if world.extraction_tiles.contains(&pos) {
        parts.push(tr!("ui.mission.tile.exit").to_string());
    }
    if visible {
        if let Some(actor) = world.standing_actor_at(pos) {
            let role = actor
                .role
                .map(|r| r.name().to_string())
                .unwrap_or_else(|| tr!("ui.mission.tile.you").to_string());
            let mood = actor
                .ai
                .as_ref()
                .map(|ai| ai.mood.label())
                .unwrap_or_default();
            if actor.is_target {
                parts.push(murmur_core::loc::fmt(
                    "ui.mission.tile.target",
                    &[("name", &actor.name), ("role", &role), ("mood", mood)],
                ));
            } else if actor.is_player() {
                parts.push(tr!("ui.mission.tile.you").to_string());
            } else {
                parts.push(murmur_core::loc::fmt(
                    "ui.mission.tile.actor",
                    &[("name", &actor.name), ("role", &role), ("mood", mood)],
                ));
            }
        }
        if let Some(body) = world.body_at(pos) {
            parts.push(trf!("ui.mission.tile.body", name = body.name));
        }
        for item in world.items_at(pos) {
            if let Some(spec) = data.item(&item.spec) {
                parts.push(spec.name.clone());
            }
        }
        if let Some(furniture) = world.furniture_at(pos) {
            let described = match furniture.kind {
                FurnitureKind::LowCover => tr!("ui.mission.tile.low_cover").to_string(),
                FurnitureKind::Container => {
                    if furniture.body.is_some() {
                        tr!("ui.mission.tile.container_full").to_string()
                    } else {
                        tr!("ui.mission.tile.container").to_string()
                    }
                }
                FurnitureKind::Wardrobe => match &furniture.disguise {
                    Some(d) => trf!(
                        "ui.mission.tile.wardrobe",
                        disguise = data.disguise(d).map(|s| s.name.as_str()).unwrap_or(d)
                    ),
                    None => tr!("ui.mission.tile.wardrobe_empty").to_string(),
                },
                FurnitureKind::Machine => {
                    let spec = furniture
                        .machine
                        .as_deref()
                        .and_then(|id| data.opportunity(id));
                    match spec {
                        Some(spec) if furniture.used => {
                            trf!("ui.mission.tile.machine_spent", name = spec.name)
                        }
                        Some(spec) => murmur_core::loc::fmt(
                            "ui.mission.tile.machine",
                            &[
                                ("name", &spec.name),
                                ("presentation", &spec.presentation),
                                ("risk", &spec.risk),
                            ],
                        ),
                        None => tr!("ui.mission.tile.machinery").to_string(),
                    }
                }
            };
            parts.push(described);
        }
    }
    if parts.is_empty() {
        tr!("ui.mission.tile.nothing").to_string()
    } else {
        parts.join(", ")
    }
}

/// What an inventory slot holds. Items are passive: carrying one is what
/// enables its verb, so the useful thing to report is which key it
/// unlocks rather than a flavour line. The key names come from the
/// keymap table, so this description follows any rebinding automatically.
pub(crate) fn slot_text(world: &World, data: &GameData, slot: usize) -> Option<String> {
    let Some(item) = world.carried_items(world.player).nth(slot) else {
        return Some(trf!("ui.mission.slot_line.empty", n = slot + 1));
    };
    let Some(spec) = data.item(&item.spec) else {
        return Some(murmur_core::loc::fmt(
            "ui.mission.slot_line.unknown",
            &[("n", &(slot + 1).to_string()), ("id", &item.spec)],
        ));
    };
    let mut notes: Vec<String> = Vec::new();
    let mut enables = |key: char| {
        if let Some(action) = crate::keymap::action(key) {
            notes.push(murmur_core::loc::fmt(
                "ui.mission.item.enables",
                &[("action", action.label()), ("key", &action.key.to_string())],
            ));
        }
    };
    if spec.firearm {
        enables('f');
    } else if spec.weapon {
        enables('g');
    }
    if spec.lockpick {
        enables('l');
    }
    if spec.noisemaker {
        enables('t');
    }
    if spec.invitation {
        notes.push(tr!("ui.mission.item.invitation").to_string());
    }
    if spec.staff_pass {
        notes.push(tr!("ui.mission.item.staff_pass").to_string());
    }
    if spec.master_key {
        notes.push(tr!("ui.mission.item.master_key").to_string());
    } else if spec.unlocks.is_some() {
        notes.push(tr!("ui.mission.item.one_lock").to_string());
    }
    if item.charges > 0 {
        notes.push(trf!("ui.mission.item.charges", count = item.charges));
    }
    let detail = if notes.is_empty() {
        tr!("ui.mission.item.no_use").to_string()
    } else {
        notes.join(", ")
    };
    Some(murmur_core::loc::fmt(
        "ui.mission.slot_line",
        &[
            ("n", &(slot + 1).to_string()),
            ("name", &spec.name),
            ("detail", &detail),
        ],
    ))
}

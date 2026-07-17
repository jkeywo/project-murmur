//! NPC perception: sight, detection, suspicion, and alert propagation.
//!
//! Runs once after each resolved turn, before the next turn's actions are
//! prepared, so everything perceived here shapes an NPC's *next* choice —
//! recipients of propagated alerts cannot react in the turn they learn.
//!
//! Vision is line of sight restricted by a facing cone and range (shorter
//! into dim rooms); the player has no facing and no cone. Guards detect
//! illegal access, visible weapons, bodies, unconscious people, and
//! gunshots; suspicion grows over time and decays when nothing is wrong.
//! Blood and missing-target detection are deliberately deferred.

use crate::access::verdict_at;
use crate::data::{GameData, Lighting, Role};
use crate::geom::{Pos, in_cone};
use crate::map::line_of_sight;
use crate::world::{ActorId, Hands, IncidentKind, Mood, World};

/// True when NPC `viewer` currently sees `target_pos` (cone, range with
/// room lighting, and line of sight; low cover hides crouched targets).
pub fn npc_sees(
    world: &World,
    data: &GameData,
    viewer: ActorId,
    target_pos: Pos,
    target_crouched: bool,
) -> bool {
    let viewer_ref = world.actor(viewer);
    let Some(facing) = viewer_ref.facing else {
        return false;
    };
    if !in_cone(
        viewer_ref.pos,
        facing,
        target_pos,
        data.tuning.cone_num,
        data.tuning.cone_den,
    ) {
        return false;
    }
    let lighting = world
        .room_at(target_pos)
        .map(|r| r.lighting)
        .unwrap_or(Lighting::Bright);
    let range = match lighting {
        Lighting::Bright => data.tuning.vision_range,
        Lighting::Dim => data.tuning.vision_range_dim,
    };
    match viewer_ref.pos.chebyshev(target_pos) {
        Some(d) if d <= range => {}
        _ => return false,
    }
    let crouched = target_crouched || viewer_ref.crouched;
    line_of_sight(viewer_ref.pos, target_pos, world.sight_blocker(crouched))
}

/// Every tile an NPC can currently see (cone, lighting-dependent range,
/// line of sight), for presentation overlays: inspecting an NPC shows
/// exactly what the perception rules let them see against a standing
/// target.
pub fn npc_visible_tiles(world: &World, data: &GameData, viewer: ActorId) -> Vec<Pos> {
    let viewer_ref = world.actor(viewer);
    let range = data.tuning.vision_range.max(data.tuning.vision_range_dim);
    let origin = viewer_ref.pos;
    let mut tiles = Vec::new();
    for y in (origin.y - range)..=(origin.y + range) {
        for x in (origin.x - range)..=(origin.x + range) {
            let pos = Pos::new(origin.floor, x, y);
            if world.map.in_bounds(pos) && npc_sees(world, data, viewer, pos, false) {
                tiles.push(pos);
            }
        }
    }
    tiles
}

/// One perceived problem, for the log.
fn note(messages: &mut Vec<String>, text: String) {
    if !messages.contains(&text) {
        messages.push(text);
    }
}

/// Updates every NPC's suspicion, mood, memory, and knowledge from what
/// they perceived this turn, then propagates suspicion and alerts one hop.
pub fn update(world: &mut World, data: &GameData) -> Vec<String> {
    let mut messages = Vec::new();
    let player_id = world.player;
    let player_pos = world.player_actor().pos;
    let player_crouched = world.player_actor().crouched;
    let player_alive = world.player_actor().alive();

    let npc_ids: Vec<ActorId> = world
        .actors
        .iter()
        .filter(|a| !a.is_player() && a.alive() && !a.departed && a.ai.is_some())
        .map(|a| a.id)
        .collect();

    // Pass 1: direct perception.
    for id in npc_ids.iter().copied() {
        let role = world.actor(id).role.unwrap_or(Role::Civilian);
        let sees_player = player_alive && npc_sees(world, data, id, player_pos, player_crouched);

        let mut gain: u16 = 0;
        let mut alarm: Option<&'static str> = None; // instant-alert causes
        let mut flee: bool = false;

        if sees_player {
            let player_disguise = world.actor(player_id).worn_disguise.clone();
            let disguise_spec = data.disguise(&player_disguise);

            // Illegal access (guards).
            if role == Role::Guard && !verdict_at(world, data, player_id).is_allowed() {
                gain += data.tuning.gain_illegal_zone;
            }
            // Crouching is inherently suspicious when observed (guards).
            if role == Role::Guard && player_crouched {
                gain += data.tuning.gain_crouching;
            }
            // Suspicious observers recognise an impostor's disguise.
            if disguise_spec.is_some_and(|d| d.suspicious_observers.contains(&role)) {
                gain += data.tuning.gain_disguise_recognised;
            }
            // A visible drawn weapon that the disguise does not legitimise.
            if matches!(world.actor(player_id).hands, Hands::Drawn(_))
                && !disguise_spec.is_some_and(|d| d.drawn_weapon_legal)
            {
                if role == Role::Guard {
                    alarm = Some("a drawn weapon");
                } else {
                    flee = true;
                }
            }
            // A carried body is unmissable evidence.
            if matches!(world.actor(player_id).hands, Hands::CarryingBody(_)) {
                if role == Role::Guard {
                    alarm = Some("someone hauling a body");
                } else {
                    flee = true;
                }
            }
        }

        // Bodies and unconscious people lying in sight.
        let evidence_pos: Option<Pos> = world
            .actors
            .iter()
            .filter(|a| a.is_visible_body() && !world.is_carried(a.id))
            .map(|a| a.pos)
            .find(|pos| npc_sees(world, data, id, *pos, false));
        // Violence incidents seen as they happen.
        let violence_pos: Option<Pos> = world
            .incidents
            .iter()
            .filter(|i| i.kind == IncidentKind::Violence)
            .map(|i| i.pos)
            .find(|pos| npc_sees(world, data, id, *pos, false));
        // Gunshots heard within radius, sight unneeded.
        let gunshot_pos: Option<Pos> = world
            .incidents
            .iter()
            .filter(|i| i.kind == IncidentKind::Gunshot)
            .find(|i| {
                world
                    .actor(id)
                    .pos
                    .chebyshev(i.pos)
                    .is_some_and(|d| d <= i.radius)
            })
            .map(|i| i.pos);

        if violence_pos.is_some() {
            world.player_violence_witnessed = true;
        }

        let ai_focus_default = if sees_player { Some(player_pos) } else { None };
        let name = world.actor(id).name.clone();
        let tuning = &data.tuning;
        let is_guard = role == Role::Guard;

        let actor = world.actor_mut(id);
        let ai = actor.ai.as_mut().unwrap();

        if let Some(pos) = violence_pos.or(evidence_pos) {
            if is_guard {
                if ai.mood != Mood::Combat {
                    ai.mood = Mood::Alerted;
                }
                ai.knows_player_hostile = true;
                ai.suspicion = tuning.suspicion_max;
                ai.focus = Some(pos);
                note(&mut messages, format!("{name} raises the alarm!"));
            } else {
                ai.mood = Mood::Fleeing;
                ai.focus = Some(pos);
                note(&mut messages, format!("{name} screams and runs"));
            }
        } else if let Some(cause) = alarm {
            if ai.mood != Mood::Combat {
                ai.mood = Mood::Alerted;
            }
            ai.knows_player_hostile = true;
            ai.suspicion = tuning.suspicion_max;
            ai.focus = Some(player_pos);
            note(&mut messages, format!("{name} spots {cause}!"));
        } else if flee {
            ai.mood = Mood::Fleeing;
            ai.focus = Some(player_pos);
            note(&mut messages, format!("{name} backs away in fear"));
        } else if let Some(pos) = gunshot_pos {
            if is_guard {
                if matches!(ai.mood, Mood::Relaxed | Mood::Suspicious) {
                    ai.mood = Mood::Investigating;
                    ai.suspicion = ai.suspicion.max(tuning.suspicion_investigate_at);
                }
                ai.focus = Some(pos);
                note(&mut messages, format!("{name} heard something"));
            } else if !matches!(ai.mood, Mood::Fleeing) {
                ai.mood = Mood::Fleeing;
                ai.focus = Some(pos);
                note(&mut messages, format!("{name} flinches at the noise"));
            }
        } else if gain > 0 {
            ai.suspicion = (ai.suspicion + gain).min(tuning.suspicion_max);
            ai.focus = ai_focus_default.or(ai.focus);
            if ai.suspicion >= tuning.suspicion_max {
                if is_guard {
                    if ai.mood != Mood::Combat {
                        ai.mood = Mood::Alerted;
                    }
                    ai.knows_player_hostile = true;
                    note(&mut messages, format!("{name} sees through you!"));
                } else {
                    ai.mood = Mood::Fleeing;
                    note(&mut messages, format!("{name} wants no part of this"));
                }
            } else if ai.suspicion >= tuning.suspicion_investigate_at {
                if matches!(ai.mood, Mood::Relaxed | Mood::Suspicious) {
                    ai.mood = Mood::Investigating;
                    note(&mut messages, format!("{name} comes to take a look"));
                }
            } else if ai.suspicion >= tuning.suspicion_suspicious_at && ai.mood == Mood::Relaxed {
                ai.mood = Mood::Suspicious;
                note(&mut messages, format!("{name} is watching you"));
            }
        } else {
            // Nothing wrong in sight: suspicion cools.
            ai.suspicion = ai.suspicion.saturating_sub(tuning.suspicion_decay);
            if ai.mood == Mood::Suspicious && ai.suspicion == 0 {
                ai.mood = Mood::Relaxed;
                ai.focus = None;
            }
        }

        // Alerted pursuers refresh their memory whenever they see you.
        if sees_player {
            let ai = world.actor_mut(id).ai.as_mut().unwrap();
            if matches!(ai.mood, Mood::Alerted | Mood::Combat) {
                ai.focus = Some(player_pos);
            }
        }
    }

    // Pass 2: propagation, one hop, from a snapshot of current moods.
    // A suspicious guard makes guards in line of sight suspicious. An
    // alert guard makes guards alert and civilians flee. Recipients get
    // only the source's focus (the incident location) or the source's own
    // position — never the live player position.
    let snapshot: Vec<(ActorId, Mood, Option<Pos>, Pos, bool)> = world
        .actors
        .iter()
        .filter(|a| !a.is_player() && a.alive() && !a.departed)
        .filter_map(|a| {
            a.ai.as_ref()
                .map(|ai| (a.id, ai.mood, ai.focus, a.pos, a.role == Some(Role::Guard)))
        })
        .collect();

    for (source_id, mood, focus, source_pos, source_is_guard) in snapshot.iter().copied() {
        if !source_is_guard {
            continue;
        }
        let spreads_alert = matches!(mood, Mood::Alerted | Mood::Combat);
        let spreads_suspicion = mood == Mood::Suspicious;
        if !spreads_alert && !spreads_suspicion {
            continue;
        }
        let shared = focus.unwrap_or(source_pos);
        for (recipient_id, recipient_mood, _, _, recipient_is_guard) in snapshot.iter().copied() {
            if recipient_id == source_id {
                continue;
            }
            // The recipient must see the source actor.
            if !npc_sees(world, data, recipient_id, source_pos, false) {
                continue;
            }
            let tuning = &data.tuning;
            if spreads_alert {
                if recipient_is_guard {
                    if !matches!(recipient_mood, Mood::Alerted | Mood::Combat) {
                        let name = world.actor(recipient_id).name.clone();
                        let ai = world.actor_mut(recipient_id).ai.as_mut().unwrap();
                        ai.mood = Mood::Alerted;
                        ai.knows_player_hostile = true;
                        ai.suspicion = tuning.suspicion_max;
                        ai.focus = Some(shared);
                        note(&mut messages, format!("{name} joins the hunt"));
                    }
                } else if !matches!(recipient_mood, Mood::Fleeing) {
                    let ai = world.actor_mut(recipient_id).ai.as_mut().unwrap();
                    ai.mood = Mood::Fleeing;
                    ai.focus = Some(shared);
                }
            } else if spreads_suspicion && recipient_is_guard && recipient_mood == Mood::Relaxed {
                let ai = world.actor_mut(recipient_id).ai.as_mut().unwrap();
                ai.mood = Mood::Suspicious;
                ai.suspicion = ai.suspicion.max(tuning.suspicion_suspicious_at);
                ai.focus = Some(shared);
            }
        }
    }

    // Guards already in combat stay lethal once violence was witnessed.
    if world.player_violence_witnessed {
        for id in npc_ids {
            let is_armed_guard = world.actor(id).role == Some(Role::Guard);
            let ai = world.actor_mut(id).ai.as_mut().unwrap();
            if is_armed_guard && ai.mood == Mood::Alerted {
                ai.mood = Mood::Combat;
            }
        }
    }

    messages
}

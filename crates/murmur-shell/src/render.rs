//! Mission rendering.
//!
//! Each map tile is one terminal cell (compact map; NPC facing is shown
//! through the inspection overlay rather than glyph markers). Floors are
//! tinted by access zone, inspecting an NPC highlights every tile their
//! perception currently covers, and the sidebar's action list doubles as
//! a clickable palette. Presentation reads the latest completed world
//! state and never influences simulation results.

use std::collections::HashSet;

use murmur_core::data::{GameData, Zone};
use murmur_core::geom::Pos;
use murmur_core::map::TileKind;
use murmur_core::perception::npc_visible_tiles;
use murmur_core::world::{Actor, FurnitureKind, Hands, Mood, World};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use murmur_core::{tr, trf};

use crate::keymap;
use crate::mission::{InputMode, LogKind, Mission, UiLayout};

/// Renders the mission and returns the layout used, for mouse hit-testing.
pub fn draw_mission(frame: &mut Frame, data: &GameData, mission: &Mission) -> UiLayout {
    let mut ui = UiLayout::default();
    let outer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(32)])
        .split(frame.area());
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(8)])
        .split(outer[0]);

    draw_map(frame, data, mission, left[0], &mut ui);
    draw_log(frame, data, mission, left[1]);
    draw_sidebar(frame, data, mission, outer[1], &mut ui);
    // Overlays draw last, over the map they explain. Help is reachable at
    // any point in a run: the key list should never be something you have
    // to abandon a mission to read.
    if *mission.mode() == InputMode::Help {
        // Over the whole frame, not just the map: the list has to fit
        // without scrolling on the shortest terminal we support, and
        // nothing behind it is worth reading while it is up.
        draw_help(frame, frame.area());
    }
    ui
}

/// The full key list, generated from the keymap table so it cannot drift
/// from what the keys actually do.
fn draw_help(frame: &mut Frame, area: Rect) {
    // No blank separators and no repeated heading: the underlined
    // category titles carry the grouping, and every row saved is a row
    // that does not get cut off on a short terminal.
    let mut lines: Vec<Line> = Vec::new();
    let key_style = Style::default()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD);
    for category in keymap::Category::ALL {
        let mut entries = keymap::in_category(category).peekable();
        if entries.peek().is_none() {
            continue;
        }
        lines.push(Line::styled(
            category.title(),
            Style::default().add_modifier(Modifier::UNDERLINED),
        ));
        for entry in entries {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<10}", entry.key), key_style),
                Span::raw(entry.help()),
            ]));
        }
    }
    lines.push(Line::styled(
        tr!("keymap.category.controls"),
        Style::default().add_modifier(Modifier::UNDERLINED),
    ));
    for entry in keymap::CONTROLS {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<10}", entry.0), key_style),
            Span::raw(keymap::control_help(entry)),
        ]));
    }

    // Clear first: the map underneath would otherwise show through the
    // gaps between lines. The way out lives in the title, where it cannot
    // be the thing that scrolls off.
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::LightMagenta))
                .title(tr!("ui.mission.help.title")),
        ),
        area,
    );
}

fn mood_color(mood: Mood) -> Color {
    match mood {
        Mood::Relaxed => Color::White,
        Mood::Suspicious => Color::Yellow,
        Mood::Investigating => Color::LightYellow,
        Mood::Alerted => Color::Red,
        Mood::Escorting => Color::Red,
        Mood::Combat => Color::LightRed,
        Mood::Fleeing => Color::Magenta,
    }
}

/// True for the states that mean someone is actively a problem. Bold on
/// the map, and listed by name in the sidebar.
fn mood_is_hostile(mood: Mood) -> bool {
    matches!(
        mood,
        Mood::Investigating | Mood::Alerted | Mood::Escorting | Mood::Combat
    )
}

/// Visible floor tint by access zone; explored-but-unseen uses the dim
/// variant of the same hue so the zones stay readable on the memory map.
fn zone_floor_color(zone: Zone, visible: bool) -> Color {
    match (zone, visible) {
        (Zone::Public, true) => Color::Rgb(120, 120, 120),
        (Zone::Public, false) => Color::Rgb(60, 60, 60),
        (Zone::Staff, true) => Color::Rgb(90, 130, 210),
        (Zone::Staff, false) => Color::Rgb(45, 65, 105),
        (Zone::Secure, true) => Color::Rgb(190, 100, 210),
        (Zone::Secure, false) => Color::Rgb(95, 50, 105),
        (Zone::Personal, true) => Color::Rgb(215, 95, 95),
        (Zone::Personal, false) => Color::Rgb(105, 48, 48),
    }
}

fn actor_cell(data: &GameData, actor: &Actor) -> (char, Style) {
    if actor.is_player() {
        let color = if matches!(actor.hands, Hands::Drawn(_)) {
            Color::LightGreen
        } else {
            Color::Green
        };
        return ('@', Style::default().fg(color).add_modifier(Modifier::BOLD));
    }
    let glyph = if actor.is_target {
        'T'
    } else {
        actor
            .role
            .and_then(|r| data.role_spec(r))
            .map(|s| s.glyph)
            .unwrap_or('?')
    };
    let mood = actor.ai.as_ref().map(|ai| ai.mood).unwrap_or(Mood::Relaxed);
    let mut style = Style::default().fg(if actor.is_target {
        Color::Cyan
    } else {
        mood_color(mood)
    });
    // Anyone who has become a problem is bold as well as red, so the
    // warning survives a palette that renders those hues badly. Colour
    // alone should not be the only carrier of "this one has noticed you".
    if actor.is_target || mood_is_hostile(mood) {
        style = style.add_modifier(Modifier::BOLD);
    }
    (glyph, style)
}

fn tile_cell(
    data: &GameData,
    world: &World,
    mission: &Mission,
    pos: Pos,
    visible: bool,
) -> (char, Style) {
    let explored = mission.is_explored(pos);
    if !visible && !explored {
        return (' ', Style::default());
    }
    let dim = Style::default().fg(Color::Rgb(70, 70, 70));

    if visible {
        if let Some(actor) = world.standing_actor_at(pos) {
            return actor_cell(data, actor);
        }
        if world.body_at(pos).is_some() {
            return ('%', Style::default().fg(Color::LightRed));
        }
        if let Some(item) = world.items_at(pos).next() {
            let glyph = data.item(&item.spec).map(|s| s.glyph).unwrap_or('?');
            return (glyph, Style::default().fg(Color::LightCyan));
        }
    }
    if let Some(furniture) = world.furniture_at(pos) {
        let (glyph, color) = match furniture.kind {
            FurnitureKind::LowCover => ('=', Color::Yellow),
            FurnitureKind::Container => ('O', Color::Gray),
            FurnitureKind::Wardrobe => ('W', Color::LightBlue),
            FurnitureKind::Machine => {
                let glyph = furniture
                    .machine
                    .as_deref()
                    .and_then(|id| data.opportunity(id))
                    .map(|s| s.glyph)
                    .unwrap_or('&');
                (glyph, Color::LightYellow)
            }
        };
        let style = if visible {
            Style::default().fg(color)
        } else {
            dim
        };
        return (glyph, style);
    }
    if world.extraction_tiles.contains(&pos) {
        let style = if visible {
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD)
        } else {
            dim
        };
        return ('X', style);
    }
    match world.map.tile(pos) {
        TileKind::Void => (' ', Style::default()),
        TileKind::Wall => {
            let style = if visible {
                Style::default().fg(Color::Gray)
            } else {
                dim
            };
            ('#', style)
        }
        TileKind::Floor => {
            let zone = world.zone_at(pos);
            ('.', Style::default().fg(zone_floor_color(zone, visible)))
        }
        TileKind::Stairs(_) => {
            let style = if visible {
                Style::default().fg(Color::LightBlue)
            } else {
                dim
            };
            ('<', style)
        }
        TileKind::Door(id) => {
            let door = world.door(id);
            let glyph = if door.open {
                '/'
            } else if door.locked_by.is_some() {
                '*'
            } else {
                '+'
            };
            let style = if visible {
                Style::default().fg(Color::White)
            } else {
                dim
            };
            (glyph, style)
        }
    }
}

fn draw_map(frame: &mut Frame, data: &GameData, mission: &Mission, area: Rect, ui: &mut UiLayout) {
    let world = mission.world();
    let focus = match mission.mode() {
        InputMode::Look(cursor) => *cursor,
        InputMode::ThrowTarget(cursor) => *cursor,
        InputMode::TargetSelect { candidates, index } => world.actor(candidates[*index]).pos,
        _ => world.player_actor().pos,
    };
    let visible: HashSet<Pos> = crate::fov::visible_tiles(world, data).into_iter().collect();

    // Inspecting a visible NPC overlays every tile they can currently see.
    let inspected = mission.inspected_tile();
    let npc_gaze: HashSet<Pos> = inspected
        .filter(|pos| visible.contains(pos))
        .and_then(|pos| world.standing_actor_at(pos))
        .filter(|actor| !actor.is_player())
        .map(|actor| {
            npc_visible_tiles(world, data, actor.id)
                .into_iter()
                .collect()
        })
        .unwrap_or_default();

    let cols = i32::from(area.width.saturating_sub(2)).max(1);
    let rows = i32::from(area.height.saturating_sub(2)).max(1);
    let map_w = i32::from(world.map.width());
    let map_h = i32::from(world.map.height());
    let mut origin_x = i32::from(focus.x) - cols / 2;
    let mut origin_y = i32::from(focus.y) - rows / 2;
    origin_x = origin_x.clamp(-1, (map_w - cols).max(-1));
    origin_y = origin_y.clamp(-1, (map_h - rows).max(-1));

    ui.map_x = area.x + 1;
    ui.map_y = area.y + 1;
    ui.map_w = cols as u16;
    ui.map_h = rows as u16;
    ui.origin = Some(Pos::new(focus.floor, origin_x as i16, origin_y as i16));

    let selected = inspected.or(match mission.mode() {
        InputMode::TargetSelect { candidates, index } => Some(world.actor(candidates[*index]).pos),
        _ => None,
    });

    let gaze_bg = Color::Rgb(72, 58, 14);
    let mut lines: Vec<Line> = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        let mut spans: Vec<Span> = Vec::with_capacity(cols as usize);
        for col in 0..cols {
            let x = origin_x + col;
            let y = origin_y + row;
            if x < 0 || y < 0 || x >= map_w || y >= map_h {
                spans.push(Span::raw(" "));
                continue;
            }
            let pos = Pos::new(focus.floor, x as i16, y as i16);
            let (glyph, mut style) = tile_cell(data, world, mission, pos, visible.contains(&pos));
            if npc_gaze.contains(&pos) {
                style = style.bg(gaze_bg);
            }
            if selected == Some(pos) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            spans.push(Span::styled(glyph.to_string(), style));
        }
        lines.push(Line::from(spans));
    }
    // Two-storey venues keep the plain "upper floor"; taller ones number
    // their storeys, since "upper" stops being unambiguous above two.
    let floor_name = match (focus.floor, world.map.floor_count()) {
        (0, _) => tr!("ui.mission.panel.map.ground").to_string(),
        (_, 2) => tr!("ui.mission.panel.map.upper").to_string(),
        (n, _) => trf!("ui.mission.panel.map.numbered", n = n),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {floor_name} "));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Severity styling for the event log. Alarms are bold as well as red so
/// the distinction survives a colourblind palette or a terminal that
/// renders red poorly.
fn log_style(kind: LogKind) -> Style {
    match kind {
        LogKind::Routine => Style::default().fg(Color::Gray),
        LogKind::Notice => Style::default().fg(Color::LightYellow),
        LogKind::Alarm => Style::default()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD),
    }
}

fn draw_log(frame: &mut Frame, data: &GameData, mission: &Mission, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    match mission.mode() {
        InputMode::Pending(action) => {
            lines.push(Line::styled(
                action.prompt().to_string(),
                Style::default().fg(Color::LightCyan),
            ));
        }
        InputMode::ThrowTarget(_) => {
            lines.push(Line::styled(
                tr!("mission.prompt.throw").to_string(),
                Style::default().fg(Color::LightCyan),
            ));
        }
        InputMode::TargetSelect { candidates, index } => {
            let target = mission.world().actor(candidates[*index]);
            lines.push(Line::styled(
                murmur_core::loc::fmt(
                    "mission.prompt.shoot",
                    &[
                        ("name", &target.name),
                        ("n", &(index + 1).to_string()),
                        ("total", &candidates.len().to_string()),
                    ],
                ),
                Style::default().fg(Color::LightCyan),
            ));
        }
        InputMode::Confirm { prompt, .. } => {
            lines.push(Line::styled(
                (*prompt).to_string(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        }
        _ => {
            // A slot the player just asked about wins the line; otherwise
            // it reports whatever they are pointing at.
            if let Some(slot) = mission.inspected_slot_text(data) {
                lines.push(Line::styled(slot, Style::default().fg(Color::LightCyan)));
            } else if let Some(pos) = mission.inspected_tile() {
                let visible = crate::fov::visible_tiles(mission.world(), data).contains(&pos);
                lines.push(Line::styled(
                    trf!(
                        "ui.mission.here",
                        what = mission.describe(data, pos, visible)
                    ),
                    Style::default().fg(Color::LightCyan),
                ));
            }
        }
    }
    let budget = usize::from(area.height).saturating_sub(2 + lines.len());
    let start = mission.log().len().saturating_sub(budget);
    for entry in &mission.log()[start..] {
        lines.push(Line::styled(entry.display(), log_style(entry.kind)));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(tr!("ui.mission.panel.events"));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_sidebar(
    frame: &mut Frame,
    data: &GameData,
    mission: &Mission,
    area: Rect,
    ui: &mut UiLayout,
) {
    let world = mission.world();
    let player = world.player_actor();
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::styled(
        murmur_core::loc::fmt(
            "ui.mission.status",
            &[
                ("turn", &world.turn.to_string()),
                ("seed", &world.seed.to_string()),
            ],
        ),
        Style::default().fg(Color::DarkGray),
    ));
    lines.push(Line::raw(""));

    // Vitals and stance.
    let hearts = "♥".repeat(usize::from(player.hp))
        + &"·".repeat(usize::from(
            data.tuning.player_max_hp.saturating_sub(player.hp),
        ));
    lines.push(Line::from(vec![
        Span::raw(tr!("ui.mission.health")),
        Span::styled(hearts, Style::default().fg(Color::LightRed)),
        Span::raw(if player.crouched {
            tr!("ui.mission.crouched")
        } else {
            ""
        }),
    ]));

    // What you are wearing, prominently: it decides where you belong.
    let disguise_name = data
        .disguise(&player.worn_disguise)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| player.worn_disguise.clone());
    lines.push(Line::from(vec![
        Span::raw(tr!("ui.mission.wearing")),
        Span::styled(
            disguise_name,
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let hands = match player.hands {
        Hands::Free => tr!("ui.mission.hands.free").to_string(),
        Hands::CarryingBody(body) => trf!("ui.mission.hands.body", name = world.actor(body).name),
        Hands::Drawn(id) => {
            // Read the drawn weapon by its exact id: the garrote is also
            // a weapon, so a name-based lookup could grab the wrong one.
            let weapon = world.carried_items(world.player).find(|i| i.id == id);
            let name = weapon
                .and_then(|i| data.item(&i.spec))
                .map(|s| s.name.clone())
                .unwrap_or_else(|| tr!("ui.mission.hands.unknown_weapon").to_string());
            let ammo = weapon.map(|i| i.charges).unwrap_or(0);
            murmur_core::loc::fmt(
                "ui.mission.hands.drawn",
                &[("weapon", &name), ("ammo", &ammo.to_string())],
            )
        }
    };
    lines.push(Line::from(hands));

    // Legitimacy of the current position.
    let verdict = murmur_core::access::verdict_at(world, data, world.player);
    let (legit, color) = match verdict {
        murmur_core::access::AccessVerdict::Allowed => {
            (tr!("ui.mission.area.legitimate"), Color::Green)
        }
        murmur_core::access::AccessVerdict::AllowedByInvitation => {
            (tr!("ui.mission.area.invited"), Color::Green)
        }
        murmur_core::access::AccessVerdict::AllowedByRoomGrant => {
            (tr!("ui.mission.area.staff_access"), Color::Green)
        }
        murmur_core::access::AccessVerdict::AllowedByPass => {
            (tr!("ui.mission.area.pass"), Color::Green)
        }
        murmur_core::access::AccessVerdict::Illegal(_) => {
            (tr!("ui.mission.area.trespassing"), Color::Red)
        }
    };
    lines.push(Line::styled(legit, Style::default().fg(color)));

    // The venue's security posture, driven by mission heat.
    let (alert_label, alert_color) = match world.heat_tier {
        0 => (tr!("ui.mission.venue.quiet"), Color::DarkGray),
        1 => (tr!("ui.mission.venue.wary"), Color::Yellow),
        _ => (tr!("ui.mission.venue.backup"), Color::Red),
    };
    lines.push(Line::styled(alert_label, Style::default().fg(alert_color)));

    // Target intel: the schedule state the whole mission turns on. Your
    // handler feeds you this — it is briefing-grade knowledge, not
    // something the player must deduce from pixels.
    let target = world.actor(world.target);
    let (intel, intel_color) = if !target.alive() {
        (tr!("ui.mission.target.down").to_string(), Color::DarkGray)
    } else {
        match target
            .ai
            .as_ref()
            .and_then(|ai| ai.schedule.as_ref())
            .and_then(|s| s.current())
            .map(|b| b.protection)
        {
            Some(murmur_core::world::Protection::Alone) => (
                trf!("ui.mission.target.alone", name = target.name),
                Color::LightGreen,
            ),
            _ => (
                trf!("ui.mission.target.escorted", name = target.name),
                Color::LightRed,
            ),
        }
    };
    lines.push(Line::styled(intel, Style::default().fg(intel_color)));

    // The contract's mandatory constraint, and whether it still holds.
    if let Some(constraint) = &world.constraint {
        if world.constraint_breach.is_some() {
            lines.push(Line::styled(
                tr!("ui.mission.contract.breached").to_string(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        } else {
            lines.push(Line::styled(
                trf!(
                    "ui.mission.contract.intact",
                    condition = constraint.short(data, &world.venue)
                ),
                Style::default().fg(Color::LightCyan),
            ));
        }
    }
    lines.push(Line::raw(""));

    // Inventory: six visible slots.
    lines.push(Line::styled(
        tr!("ui.mission.inventory.title"),
        Style::default().add_modifier(Modifier::UNDERLINED),
    ));
    let carried: Vec<String> = world
        .carried_items(world.player)
        .map(|i| {
            let name = data
                .item(&i.spec)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| i.spec.clone());
            if i.charges > 0 {
                murmur_core::loc::fmt(
                    "ui.mission.inventory.slot_charges",
                    &[
                        ("n", ""),
                        ("name", &name),
                        ("charges", &i.charges.to_string()),
                    ],
                )
            } else {
                name
            }
        })
        .collect();
    for slot in 0..murmur_core::actions::INVENTORY_SLOTS {
        match carried.get(slot) {
            Some(name) => lines.push(Line::from(murmur_core::loc::fmt(
                "ui.mission.inventory.slot",
                &[("n", &(slot + 1).to_string()), ("name", name)],
            ))),
            None => lines.push(Line::styled(
                trf!("ui.mission.inventory.empty", n = slot + 1),
                Style::default().fg(Color::DarkGray),
            )),
        }
    }
    lines.push(Line::raw(""));

    // Who you can see, and what each of them is doing. A bare count told
    // you that you were in trouble but not with whom, which is the part
    // you can actually act on. Reuses the shooting target list, so this
    // panel and the map agree about who is visible.
    lines.push(Line::styled(
        tr!("ui.mission.insight.title"),
        Style::default().add_modifier(Modifier::UNDERLINED),
    ));
    let seen = crate::fov::visible_actors(world, data);
    if seen.is_empty() {
        lines.push(Line::styled(
            tr!("ui.mission.insight.nobody"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    // Nearest first, and only as many as the panel can show without
    // pushing the action palette off the bottom.
    for id in seen.iter().take(4) {
        let actor = world.actor(*id);
        let mood = actor.ai.as_ref().map(|ai| ai.mood).unwrap_or(Mood::Relaxed);
        let distance = world.player_actor().pos.chebyshev(actor.pos);
        let range = distance.map(|d| format!(" {d}")).unwrap_or_default();
        let mut style = Style::default().fg(mood_color(mood));
        if mood_is_hostile(mood) {
            style = style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::styled(
            murmur_core::loc::fmt(
                "ui.mission.insight.actor",
                &[
                    ("name", &actor.name),
                    ("range", &range),
                    ("mood", mood.label()),
                ],
            ),
            style,
        ));
    }
    if seen.len() > 4 {
        lines.push(Line::styled(
            trf!("ui.mission.insight.more", count = seen.len() - 4),
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Anyone hunting you may well be out of sight, so this stays a count
    // over the whole venue rather than only what you can see.
    let hunting = world
        .actors
        .iter()
        .filter(|a| {
            a.alive()
                && !a.departed
                && a.ai
                    .as_ref()
                    .is_some_and(|ai| matches!(ai.mood, Mood::Alerted | Mood::Combat))
        })
        .count();
    if hunting > 0 {
        lines.push(Line::styled(
            trf!("ui.mission.hunting", count = hunting),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    let target = world.actor(world.target);
    if !target.alive() {
        lines.push(Line::styled(
            tr!("ui.mission.target_dead"),
            Style::default().fg(Color::LightGreen),
        ));
        lines.push(Line::from(tr!("ui.mission.reach_exit")));
    }
    lines.push(Line::raw(""));

    // Clickable action palette, two per row.
    lines.push(Line::styled(
        tr!("ui.mission.actions.title"),
        Style::default().add_modifier(Modifier::UNDERLINED),
    ));
    let palette_start_row = area.y + 1 + lines.len() as u16;
    let key_style = Style::default()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD);
    for (pair_index, pair) in keymap::ACTIONS.chunks(2).enumerate() {
        let row = palette_start_row + pair_index as u16;
        let mut spans: Vec<Span> = Vec::new();
        let mut column = area.x + 1;
        for entry in pair {
            let (key, label) = (entry.key, entry.label());
            let text = format!("{key} {label}");
            let width = text.chars().count() as u16;
            let padded = format!("{text:<16}");
            ui.rows.push(
                row,
                column,
                column + width - 1,
                crate::ShellInput::Char(key),
            );
            // Actions with no valid target right now are dimmed; they
            // still click through and report why they can't be used.
            let available = mission.action_available(data, key);
            let (kstyle, lstyle) = if available {
                (key_style, Style::default())
            } else {
                let dim = Style::default().fg(Color::DarkGray);
                (dim, dim)
            };
            spans.push(Span::styled(format!("{} ", key), kstyle));
            spans.push(Span::styled(padded[2..].to_string(), lstyle));
            column += 16;
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        tr!("ui.mission.footer.move"),
        Style::default().fg(Color::DarkGray),
    ));
    lines.push(Line::styled(
        trf!("ui.mission.footer.speed", speed = mission.speed().label()),
        Style::default().fg(Color::DarkGray),
    ));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(tr!("ui.mission.panel.agent"));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

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
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::mission::{InputMode, Mission, UiLayout};

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
    ui
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
    if actor.is_target {
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
        TileKind::Stairs => {
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
    let focus = match &mission.mode {
        InputMode::Look(cursor) => *cursor,
        InputMode::TargetSelect { candidates, index } => world.actor(candidates[*index]).pos,
        _ => world.player_actor().pos,
    };
    let visible: HashSet<Pos> = mission.visible_tiles(data).into_iter().collect();

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

    let selected = inspected.or(match &mission.mode {
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
    let floor_name = if focus.floor == 0 {
        "ground floor"
    } else {
        "upper floor"
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {floor_name} "));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_log(frame: &mut Frame, data: &GameData, mission: &Mission, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    match &mission.mode {
        InputMode::Pending(action) => {
            lines.push(Line::styled(
                action.prompt().to_string(),
                Style::default().fg(Color::LightCyan),
            ));
        }
        InputMode::TargetSelect { candidates, index } => {
            let target = mission.world().actor(candidates[*index]);
            lines.push(Line::styled(
                format!(
                    "aim: {} ({}/{}) - Enter or click fires, Esc cancels",
                    target.name,
                    index + 1,
                    candidates.len()
                ),
                Style::default().fg(Color::LightCyan),
            ));
        }
        _ => {
            if let Some(pos) = mission.inspected_tile() {
                let visible = mission.visible_tiles(data).contains(&pos);
                lines.push(Line::styled(
                    format!("here: {}", mission.describe(data, pos, visible)),
                    Style::default().fg(Color::LightCyan),
                ));
            }
        }
    }
    let budget = usize::from(area.height).saturating_sub(2 + lines.len());
    let start = mission.log.len().saturating_sub(budget);
    for message in &mission.log[start..] {
        lines.push(Line::raw(message.clone()));
    }
    let block = Block::default().borders(Borders::ALL).title(" events ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The clickable action palette: label and key for each verb.
const ACTIONS: &[(char, &str)] = &[
    ('.', "wait"),
    ('c', "crouch"),
    ('r', "draw/holster"),
    ('g', "garrote"),
    ('f', "shoot"),
    ('p', "pickpocket"),
    ('d', "disguise"),
    ('b', "carry/drop"),
    ('h', "hide body"),
    ('o', "open door"),
    ('k', "close door"),
    (';', "look"),
];

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
        format!("turn {:>5}   seed {}", world.turn, world.seed),
        Style::default().fg(Color::DarkGray),
    ));
    lines.push(Line::raw(""));

    // Vitals and stance.
    let hearts = "♥".repeat(usize::from(player.hp))
        + &"·".repeat(usize::from(
            data.tuning.player_max_hp.saturating_sub(player.hp),
        ));
    lines.push(Line::from(vec![
        Span::raw("health "),
        Span::styled(hearts, Style::default().fg(Color::LightRed)),
        Span::raw(if player.crouched { "   crouched" } else { "" }),
    ]));

    // What you are wearing, prominently: it decides where you belong.
    let disguise_name = data
        .disguise(&player.worn_disguise)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| player.worn_disguise.clone());
    lines.push(Line::from(vec![
        Span::raw("wearing "),
        Span::styled(
            disguise_name,
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let hands = match player.hands {
        Hands::Free => "hands free".to_string(),
        Hands::CarryingBody(body) => format!("carrying {}", world.actor(body).name),
        Hands::Drawn(_) => {
            let ammo = world
                .carried_items(world.player)
                .find(|i| data.item(&i.spec).is_some_and(|s| s.weapon))
                .map(|i| i.charges)
                .unwrap_or(0);
            format!("pistol drawn ({ammo} rounds)")
        }
    };
    lines.push(Line::from(hands));

    // Legitimacy of the current position.
    let verdict = murmur_core::access::verdict_at(world, data, world.player);
    let (legit, color) = match verdict {
        murmur_core::access::AccessVerdict::Allowed => ("area: legitimate", Color::Green),
        murmur_core::access::AccessVerdict::AllowedByInvitation => ("area: invited", Color::Green),
        murmur_core::access::AccessVerdict::AllowedByRoomGrant => {
            ("area: staff access", Color::Green)
        }
        murmur_core::access::AccessVerdict::Illegal(_) => ("area: TRESPASSING", Color::Red),
    };
    lines.push(Line::styled(legit, Style::default().fg(color)));

    // The contract's mandatory constraint, and whether it still holds.
    if let Some(constraint) = &world.constraint {
        if world.constraint_breach.is_some() {
            lines.push(Line::styled(
                "contract: BREACHED".to_string(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        } else {
            lines.push(Line::styled(
                format!("contract: {}", constraint.short()),
                Style::default().fg(Color::LightCyan),
            ));
        }
    }
    lines.push(Line::raw(""));

    // Inventory: six visible slots.
    lines.push(Line::styled(
        "inventory",
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
                format!("{name} ({})", i.charges)
            } else {
                name
            }
        })
        .collect();
    for slot in 0..murmur_core::actions::INVENTORY_SLOTS {
        match carried.get(slot) {
            Some(name) => lines.push(Line::from(format!(" {}. {name}", slot + 1))),
            None => lines.push(Line::styled(
                format!(" {}. -", slot + 1),
                Style::default().fg(Color::DarkGray),
            )),
        }
    }
    lines.push(Line::raw(""));

    // Threat summary.
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
            format!("!! {hunting} hunting you"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    let target = world.actor(world.target);
    if !target.alive() {
        lines.push(Line::styled(
            "target eliminated",
            Style::default().fg(Color::LightGreen),
        ));
        lines.push(Line::from("reach an X exit"));
    }
    lines.push(Line::raw(""));

    // Clickable action palette, two per row.
    lines.push(Line::styled(
        "actions (click or key)",
        Style::default().add_modifier(Modifier::UNDERLINED),
    ));
    let palette_start_row = area.y + 1 + lines.len() as u16;
    let key_style = Style::default()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD);
    for (pair_index, pair) in ACTIONS.chunks(2).enumerate() {
        let row = palette_start_row + pair_index as u16;
        let mut spans: Vec<Span> = Vec::new();
        let mut column = area.x + 1;
        for (key, label) in pair {
            let text = format!("{key} {label}");
            let width = text.chars().count() as u16;
            let padded = format!("{text:<16}");
            ui.actions.push((row, column, column + width - 1, *key));
            spans.push(Span::styled(format!("{} ", key), key_style));
            spans.push(Span::raw(padded[2..].to_string()));
            column += 16;
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "arrows move - hover inspects",
        Style::default().fg(Color::DarkGray),
    ));
    lines.push(Line::styled(
        format!("Esc cancel - [ ] speed: {}", mission.speed.label()),
        Style::default().fg(Color::DarkGray),
    ));

    let block = Block::default().borders(Borders::ALL).title(" agent ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

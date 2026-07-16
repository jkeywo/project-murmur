//! Mission rendering.
//!
//! Every map tile renders as two terminal cells, which squares up the
//! aspect ratio and gives NPCs room for their facing marker (`g>`, `g^`),
//! exactly as the foundation sketches them. Presentation reads the latest
//! completed world state and never influences simulation results.

use std::collections::HashSet;

use murmur_core::data::GameData;
use murmur_core::geom::Pos;
use murmur_core::map::TileKind;
use murmur_core::world::{Actor, FurnitureKind, Hands, Mood, World};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::mission::{InputMode, Mission};

pub fn draw_mission(frame: &mut Frame, data: &GameData, mission: &Mission) {
    let outer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(32)])
        .split(frame.area());
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(8)])
        .split(outer[0]);

    draw_map(frame, data, mission, left[0]);
    draw_log(frame, data, mission, left[1]);
    draw_sidebar(frame, data, mission, outer[1]);
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

fn actor_cell(data: &GameData, world: &World, actor: &Actor) -> (String, Style) {
    if actor.is_player() {
        let marker = if actor.crouched { '_' } else { ' ' };
        let color = if matches!(actor.hands, Hands::Drawn(_)) {
            Color::LightGreen
        } else {
            Color::Green
        };
        return (
            format!("@{marker}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        );
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
    let marker = actor.facing.map(|f| f.marker()).unwrap_or(' ');
    let mood = actor.ai.as_ref().map(|ai| ai.mood).unwrap_or(Mood::Relaxed);
    let mut style = Style::default().fg(if actor.is_target {
        Color::Cyan
    } else {
        mood_color(mood)
    });
    if actor.is_target {
        style = style.add_modifier(Modifier::BOLD);
    }
    let _ = world;
    (format!("{glyph}{marker}"), style)
}

fn tile_cell(
    data: &GameData,
    world: &World,
    mission: &Mission,
    pos: Pos,
    visible: bool,
) -> (String, Style) {
    let explored = mission.is_explored(pos);
    if !visible && !explored {
        return ("  ".to_string(), Style::default());
    }
    let dim = Style::default().fg(Color::DarkGray);

    if visible {
        if let Some(actor) = world.standing_actor_at(pos) {
            return actor_cell(data, world, actor);
        }
        if let Some(body) = world.body_at(pos) {
            let _ = body;
            return ("% ".to_string(), Style::default().fg(Color::LightRed));
        }
        if let Some(item) = world.items_at(pos).next() {
            let glyph = data.item(&item.spec).map(|s| s.glyph).unwrap_or('?');
            return (format!("{glyph} "), Style::default().fg(Color::LightCyan));
        }
    }
    if let Some(furniture) = world.furniture_at(pos) {
        let (text, color) = match furniture.kind {
            FurnitureKind::LowCover => ("==", Color::Yellow),
            FurnitureKind::Container => ("[]", Color::Gray),
            FurnitureKind::Wardrobe => ("{}", Color::LightBlue),
        };
        let style = if visible {
            Style::default().fg(color)
        } else {
            dim
        };
        return (text.to_string(), style);
    }
    if world.extraction_tiles.contains(&pos) {
        let style = if visible {
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD)
        } else {
            dim
        };
        return ("X ".to_string(), style);
    }
    let (text, bright): (&str, Color) = match world.map.tile(pos) {
        TileKind::Void => ("  ", Color::Black),
        TileKind::Wall => ("##", Color::Gray),
        TileKind::Floor => (". ", Color::DarkGray),
        TileKind::Stairs => ("< ", Color::LightBlue),
        TileKind::Door(id) => {
            let door = world.door(id);
            if door.open {
                ("/ ", Color::White)
            } else if door.locked_by.is_some() {
                ("* ", Color::LightYellow)
            } else {
                ("+ ", Color::White)
            }
        }
    };
    let style = if visible {
        Style::default().fg(bright)
    } else {
        dim
    };
    (text.to_string(), style)
}

fn draw_map(frame: &mut Frame, data: &GameData, mission: &Mission, area: Rect) {
    let world = mission.world();
    let focus = match &mission.mode {
        InputMode::Look(cursor) => *cursor,
        InputMode::TargetSelect { candidates, index } => world.actor(candidates[*index]).pos,
        _ => world.player_actor().pos,
    };
    let visible: HashSet<Pos> = mission.visible_tiles(data).into_iter().collect();

    let cols = i32::from(area.width / 2).max(1);
    let rows = i32::from(area.height).max(1);
    let map_w = i32::from(world.map.width());
    let map_h = i32::from(world.map.height());
    let mut origin_x = i32::from(focus.x) - cols / 2;
    let mut origin_y = i32::from(focus.y) - rows / 2;
    origin_x = origin_x.clamp(-1, (map_w - cols).max(-1));
    origin_y = origin_y.clamp(-1, (map_h - rows).max(-1));

    let selected = match &mission.mode {
        InputMode::TargetSelect { candidates, index } => Some(world.actor(candidates[*index]).pos),
        InputMode::Look(cursor) => Some(*cursor),
        _ => None,
    };

    let mut lines: Vec<Line> = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        let mut spans: Vec<Span> = Vec::with_capacity(cols as usize);
        for col in 0..cols {
            let x = origin_x + col;
            let y = origin_y + row;
            if x < 0 || y < 0 || x >= map_w || y >= map_h {
                spans.push(Span::raw("  "));
                continue;
            }
            let pos = Pos::new(focus.floor, x as i16, y as i16);
            let (text, mut style) = tile_cell(data, world, mission, pos, visible.contains(&pos));
            if selected == Some(pos) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            spans.push(Span::styled(text, style));
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
        InputMode::Look(cursor) => {
            let visible = mission.visible_tiles(data).contains(cursor);
            lines.push(Line::styled(
                format!("look: {}", mission.describe(data, *cursor, visible)),
                Style::default().fg(Color::LightCyan),
            ));
        }
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
                    "aim: {} ({}/{}) - Enter fires, Esc cancels",
                    target.name,
                    index + 1,
                    candidates.len()
                ),
                Style::default().fg(Color::LightCyan),
            ));
        }
        InputMode::Normal => {}
    }
    let budget = usize::from(area.height).saturating_sub(2 + lines.len());
    let start = mission.log.len().saturating_sub(budget);
    for message in &mission.log[start..] {
        lines.push(Line::raw(message.clone()));
    }
    let block = Block::default().borders(Borders::ALL).title(" events ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_sidebar(frame: &mut Frame, data: &GameData, mission: &Mission, area: Rect) {
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

    let disguise_name = data
        .disguise(&player.worn_disguise)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| player.worn_disguise.clone());
    lines.push(Line::from(format!("wearing {disguise_name}")));

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

    // The queue: capacity always visible.
    let state = if mission.queue.is_paused() {
        Span::styled(
            " PAUSED",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" running", Style::default().fg(Color::Green))
    };
    lines.push(Line::from(vec![
        Span::raw(format!(
            "queue {:>2}/{}",
            mission.queue.len(),
            mission.queue.capacity()
        )),
        state,
    ]));
    let mut bar = String::new();
    for index in 0..mission.queue.capacity() {
        bar.push(if index < mission.queue.len() {
            '#'
        } else {
            '·'
        });
    }
    lines.push(Line::styled(bar, Style::default().fg(Color::LightBlue)));
    lines.push(Line::from(format!("speed: {}", mission.speed.label())));

    // Threat summary.
    let hunting = world
        .actors
        .iter()
        .filter(|a| {
            a.alive()
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
    lines.push(Line::styled(
        "keys",
        Style::default().add_modifier(Modifier::UNDERLINED),
    ));
    for help in [
        "arrows move  . wait  c crouch",
        "o/k open/close  b body  h hide",
        "g garrote  f shoot  r draw",
        "p pickpocket  d disguise",
        "; look  Space pause  [ ] speed",
        "Bksp undo  Esc clear  Q abandon",
    ] {
        lines.push(Line::styled(help, Style::default().fg(Color::DarkGray)));
    }

    let block = Block::default().borders(Borders::ALL).title(" agent ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

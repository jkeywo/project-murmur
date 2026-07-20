//! Ratatui rendering for the non-gameplay interface modes. These are
//! interface modes only: they never advance simulation time.

use murmur_campaign::{CampaignState, ResolutionSummary};
use murmur_core::data::GameData;
use murmur_core::world::MissionFacts;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use murmur_core::{tr, trf};

use crate::keymap;
use crate::{DebriefHeadline, ScreenLayout, ShellInput, Tone};

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Splits a bordered panel's interior into a body and a fixed footer of
/// `rows` rows. Prompts live in the footer so they stay on screen — and
/// stay clickable at known rows — however long the body runs.
fn body_and_footer(area: Rect, rows: u16) -> (Rect, Rect) {
    let width = area.width.saturating_sub(2);
    let interior = area.height.saturating_sub(2);
    let footer_h = rows.min(interior);
    let body_h = interior - footer_h;
    (
        Rect {
            x: area.x + 1,
            y: area.y + 1,
            width,
            height: body_h,
        },
        Rect {
            x: area.x + 1,
            y: area.y + 1 + body_h,
            width,
            height: footer_h,
        },
    )
}

/// Word-wraps to `width` columns at build time, so panels that need
/// exact row positions (for clickable prompts) never rely on
/// render-time wrapping.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Pushes `text` as one or more wrapped lines.
fn push_wrapped(lines: &mut Vec<Line<'static>>, text: String, style: Style, width: usize) {
    for piece in wrap_text(&text, width) {
        lines.push(Line::styled(piece, style));
    }
}

/// Renders footer prompts and records each row as clickable.
fn render_footer(
    frame: &mut Frame,
    layout: &mut ScreenLayout,
    footer: Rect,
    alignment: Alignment,
    prompts: &[(ShellInput, String, Style)],
) {
    // A blank leading row separates the prompts from the body.
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];
    for (index, (input, text, style)) in prompts.iter().enumerate() {
        let row = footer.y + 1 + index as u16;
        if row < footer.y + footer.height {
            layout.push_row(footer, row, *input);
        }
        lines.push(Line::styled(text.clone(), *style));
    }
    frame.render_widget(Paragraph::new(lines).alignment(alignment), footer);
}

/// Guards the start screen's "start over": one save slot means a fresh
/// campaign destroys the existing one with no way back.
pub fn draw_confirm_new_campaign(frame: &mut Frame) -> ScreenLayout {
    let mut layout = ScreenLayout::default();
    let area = centered(frame.area(), 54, 11);
    let lines = vec![
        Line::styled(
            tr!("ui.confirm_new.title"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(tr!("ui.confirm_new.body.1")),
        Line::raw(tr!("ui.confirm_new.body.2")),
        Line::raw(tr!("ui.confirm_new.body.3")),
    ];
    let prompts = vec![
        (
            ShellInput::Char('y'),
            tr!("ui.confirm_new.prompt.yes").to_string(),
            Style::default().fg(Color::Red),
        ),
        (
            ShellInput::Esc,
            tr!("ui.confirm_new.prompt.no").to_string(),
            Style::default().fg(Color::LightGreen),
        ),
    ];
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red)),
        area,
    );
    let (body, footer) = body_and_footer(area, prompts.len() as u16 + 1);
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), body);
    render_footer(frame, &mut layout, footer, Alignment::Center, &prompts);
    layout
}

/// A compact "key label   key label" summary of every binding, packed to
/// `width` columns. Derived from the keymap table so the start screen and
/// the in-mission help can never disagree about what a key does.
fn keymap_summary(width: usize) -> Vec<String> {
    let mut cells: Vec<String> = keymap::ACTIONS
        .iter()
        .map(|a| format!("{} {}", a.key, a.label()))
        .collect();
    cells.extend(
        keymap::CONTROLS
            .iter()
            .map(|entry| format!("{} {}", entry.0, keymap::control_short(entry))),
    );
    // Packed to natural width rather than fixed columns: this panel is
    // centred, and padded cells centre raggedly.
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for cell in cells {
        let addition = if line.is_empty() {
            cell.chars().count()
        } else {
            cell.chars().count() + 3
        };
        if !line.is_empty() && line.chars().count() + addition > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push_str("   ");
        }
        line.push_str(&cell);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

pub fn draw_start(frame: &mut Frame, has_save: bool) -> ScreenLayout {
    let mut layout = ScreenLayout::default();
    let area = centered(frame.area(), 66, 30);
    let mut lines = vec![
        Line::styled(
            tr!("ui.start.title"),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(tr!("ui.start.pitch.1")),
        Line::raw(tr!("ui.start.pitch.2")),
        Line::raw(""),
        Line::raw(tr!("ui.start.rules.1")),
        Line::raw(tr!("ui.start.rules.2")),
        Line::raw(tr!("ui.start.rules.3")),
        Line::raw(tr!("ui.start.rules.4")),
        Line::raw(""),
        Line::styled(
            tr!("ui.start.keys_heading"),
            Style::default().add_modifier(Modifier::UNDERLINED),
        ),
    ];
    // Built from the keymap table rather than written out again, so this
    // list cannot drift from what the keys actually do. Press ? in a
    // mission for the same bindings with full descriptions.
    lines.extend(keymap_summary(60).into_iter().map(Line::raw));
    lines.extend([
        Line::raw(""),
        Line::raw(tr!("ui.start.help_hint")),
        Line::raw(""),
        Line::raw(tr!("ui.start.mouse.1")),
        Line::raw(tr!("ui.start.mouse.2")),
        Line::raw(tr!("ui.start.mouse.3")),
    ]);
    let green = Style::default().fg(Color::LightGreen);
    let mut prompts = vec![(
        ShellInput::Enter,
        if has_save {
            tr!("ui.start.prompt.continue").to_string()
        } else {
            tr!("ui.start.prompt.begin").to_string()
        },
        green,
    )];
    if has_save {
        prompts.push((
            ShellInput::Char('n'),
            tr!("ui.start.prompt.new").to_string(),
            green,
        ));
    }
    prompts.push((
        ShellInput::Char('q'),
        tr!("ui.start.prompt.quit").to_string(),
        Style::default().fg(Color::DarkGray),
    ));

    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
        area,
    );
    let (body, footer) = body_and_footer(area, prompts.len() as u16 + 1);
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), body);
    render_footer(frame, &mut layout, footer, Alignment::Center, &prompts);
    layout
}

/// The campaign hub: contract board, stash, shop, and district heat.
/// Records clickable rows; clicking one equals pressing its key.
pub fn draw_hub(
    frame: &mut Frame,
    data: &GameData,
    campaign: &CampaignState,
    accepting: Option<&crate::PendingAccept>,
    message: Option<&str>,
) -> ScreenLayout {
    let mut layout = ScreenLayout::default();
    let area = centered(frame.area(), 78, 34);
    let inner_x = area.x + 1;
    let mut lines: Vec<Line> = Vec::new();
    let key_style = Style::default()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD);

    // A row with a leading key, recorded for mouse hit-testing.
    let push_key_row = |lines: &mut Vec<Line>,
                        layout: &mut ScreenLayout,
                        key: char,
                        text: String,
                        style: Style| {
        let row = area.y + 1 + lines.len() as u16;
        let width = (text.chars().count() + 2) as u16;
        layout.push(row, inner_x, inner_x + width, ShellInput::Char(key));
        lines.push(Line::from(vec![
            Span::styled(format!("{key} "), key_style),
            Span::styled(text, style),
        ]));
    };

    lines.push(Line::styled(
        tr!("ui.hub.title"),
        Style::default()
            .fg(Color::LightYellow)
            .add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(murmur_core::loc::fmt(
        "ui.hub.wallet",
        &[
            ("cash", &campaign.cash.to_string()),
            ("contracts", &campaign.history.len().to_string()),
        ],
    )));
    lines.push(Line::from(
        campaign
            .district_heat
            .iter()
            .map(|(d, h)| {
                murmur_core::loc::fmt(
                    "ui.hub.district_heat",
                    &[("district", d), ("heat", &h.to_string())],
                )
            })
            .collect::<Vec<_>>()
            .join("   "),
    ));
    lines.push(Line::raw(""));

    if let Some(pending) = accepting {
        // Loadout selection.
        lines.push(Line::styled(
            murmur_core::loc::fmt(
                "ui.hub.loadout.title",
                &[
                    ("venue", &pending.offer.venue),
                    ("district", &pending.offer.district),
                ],
            ),
            Style::default().add_modifier(Modifier::UNDERLINED),
        ));
        for (index, item) in campaign.owned_equipment.iter().enumerate() {
            let key = (b'1' + index as u8) as char;
            let name = data
                .item(item)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| item.clone());
            let chosen = pending.chosen.contains(item);
            let marker = if chosen { "[x]" } else { "[ ]" };
            let style = if chosen {
                Style::default().fg(Color::LightGreen)
            } else {
                Style::default()
            };
            push_key_row(
                &mut lines,
                &mut layout,
                key,
                format!("{marker} {name}"),
                style,
            );
        }
        lines.push(Line::raw(""));
        let green = Style::default().fg(Color::LightGreen);
        for (input, text) in [
            (ShellInput::Enter, tr!("ui.hub.loadout.prompt.take")),
            (ShellInput::Esc, tr!("ui.hub.loadout.prompt.back")),
        ] {
            let row = area.y + 1 + lines.len() as u16;
            layout.push(row, inner_x, area.x + area.width - 2, input);
            lines.push(Line::styled(text.to_string(), green));
        }
    } else {
        // The contract board.
        lines.push(Line::styled(
            tr!("ui.hub.offers.title"),
            Style::default().add_modifier(Modifier::UNDERLINED),
        ));
        for (index, offer) in campaign.offers(data).iter().enumerate() {
            let key = (b'1' + index as u8) as char;
            push_key_row(
                &mut lines,
                &mut layout,
                key,
                murmur_core::loc::fmt(
                    "ui.hub.offer.line",
                    &[
                        ("venue", &offer.venue),
                        ("district", &offer.district),
                        ("payout", &offer.payout.to_string()),
                        ("heat", &offer.heat.to_string()),
                    ],
                ),
                Style::default().add_modifier(Modifier::BOLD),
            );
            lines.push(Line::styled(
                trf!("ui.hub.offer.hook", hook = offer.hook),
                Style::default().fg(Color::Gray),
            ));
            // The board stays compact with the chip; the full condition
            // is spelled out on the briefing before you commit.
            lines.push(Line::styled(
                trf!(
                    "ui.hub.offer.condition",
                    condition = offer.constraint.short(data, &offer.venue)
                ),
                Style::default().fg(Color::LightCyan),
            ));
        }
        push_key_row(
            &mut lines,
            &mut layout,
            'r',
            tr!("ui.hub.offer.pass").to_string(),
            Style::default().fg(Color::DarkGray),
        );
        lines.push(Line::raw(""));

        // The stash and the shop.
        lines.push(Line::styled(
            tr!("ui.hub.stash.title"),
            Style::default().add_modifier(Modifier::UNDERLINED),
        ));
        let stash: Vec<String> = campaign
            .owned_equipment
            .iter()
            .map(|i| {
                data.item(i)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| i.clone())
            })
            .collect();
        lines.push(Line::from(if stash.is_empty() {
            tr!("ui.hub.stash.empty").to_string()
        } else {
            stash.join(", ")
        }));
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            tr!("ui.hub.fence.title"),
            Style::default().add_modifier(Modifier::UNDERLINED),
        ));
        for (index, entry) in data.equipment.iter().enumerate() {
            let key = (b'a' + index as u8) as char;
            let name = data
                .item(&entry.item)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| entry.item.clone());
            let owned = campaign.owned_equipment.iter().any(|i| i == &entry.item);
            let (text, style) = if owned {
                (
                    trf!("ui.hub.fence.owned", name = name),
                    Style::default().fg(Color::DarkGray),
                )
            } else {
                (
                    murmur_core::loc::fmt(
                        "ui.hub.fence.forsale",
                        &[
                            ("name", &name),
                            ("price", &entry.price.to_string()),
                            ("approach", entry.approach.name()),
                        ],
                    ),
                    Style::default(),
                )
            };
            push_key_row(&mut lines, &mut layout, key, text, style);
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            tr!("ui.hub.footer"),
            Style::default().fg(Color::LightGreen),
        ));
    }

    if let Some(message) = message {
        lines.push(Line::styled(
            message.to_string(),
            Style::default().fg(Color::LightYellow),
        ));
    }

    let widget = Paragraph::new(lines).alignment(Alignment::Left).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(widget, area);
    layout
}

pub fn draw_briefing(
    frame: &mut Frame,
    data: &GameData,
    facts: &MissionFacts,
    offer: &murmur_campaign::ContractOffer,
    loadout: &[String],
    seed: u64,
) -> ScreenLayout {
    let mut layout = ScreenLayout::default();
    // The briefing is the longest panel and every field wraps, so it
    // takes what the terminal gives rather than a fixed height.
    let tall = frame.area().height.saturating_sub(2).clamp(20, 44);
    let area = centered(frame.area(), 74, tall);
    let width = usize::from(area.width.saturating_sub(2));
    let plain = Style::default();
    let loadout_names: Vec<String> = loadout
        .iter()
        .map(|i| {
            data.item(i)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| i.clone())
        })
        .collect();

    let mut lines: Vec<Line<'static>> = vec![
        Line::styled(
            tr!("ui.briefing.title"),
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            murmur_core::loc::fmt(
                "ui.briefing.subtitle",
                &[
                    ("seed", &seed.to_string()),
                    ("venue", &offer.venue),
                    ("district", &offer.district),
                ],
            ),
            Style::default().fg(Color::DarkGray),
        ),
        Line::raw(""),
    ];
    push_wrapped(
        &mut lines,
        trf!("ui.briefing.target", name = facts.target_name),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        trf!("ui.briefing.reason", hook = offer.hook),
        plain,
        width,
    );
    let locations = if facts.target_locations.is_empty() {
        tr!("ui.briefing.locations_unknown").to_string()
    } else {
        facts.target_locations.join(", ")
    };
    push_wrapped(
        &mut lines,
        trf!("ui.briefing.locations", rooms = locations),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        trf!(
            "ui.briefing.condition",
            condition = offer.constraint.describe(data, &offer.venue)
        ),
        Style::default().fg(Color::LightCyan),
        width,
    );
    push_wrapped(
        &mut lines,
        trf!("ui.briefing.payout", payout = offer.payout),
        plain,
        width,
    );
    lines.push(Line::raw(""));
    push_wrapped(
        &mut lines,
        murmur_core::loc::fmt(
            "ui.briefing.security",
            &[
                ("guards", &facts.guard_count.to_string()),
                ("staff", &facts.staff_count.to_string()),
                ("guests", &facts.civilian_count.to_string()),
            ],
        ),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        trf!(
            "ui.briefing.restricted",
            rooms = facts.restricted_rooms.join(", ")
        ),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        trf!(
            "ui.briefing.disguises",
            disguises = facts.available_disguises.join(", ")
        ),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        trf!("ui.briefing.containers", count = facts.container_count),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        trf!(
            "ui.briefing.extraction",
            exits = facts.extraction_exits.join(" or ")
        ),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        if facts.opportunities.is_empty() {
            tr!("ui.briefing.word.none").to_string()
        } else {
            trf!(
                "ui.briefing.word.some",
                hints = facts.opportunities.join("; ")
            )
        },
        plain,
        width,
    );
    lines.push(Line::raw(""));
    push_wrapped(
        &mut lines,
        if loadout_names.is_empty() {
            tr!("ui.briefing.loadout.none").to_string()
        } else {
            trf!("ui.briefing.loadout.some", items = loadout_names.join(", "))
        },
        plain,
        width,
    );
    lines.push(Line::raw(tr!("ui.briefing.entry")));

    let green = Style::default().fg(Color::LightGreen);
    let prompts = [
        (
            ShellInput::Enter,
            tr!("ui.briefing.prompt.go").to_string(),
            green,
        ),
        (
            ShellInput::Esc,
            tr!("ui.briefing.prompt.pass").to_string(),
            green,
        ),
    ];
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
        area,
    );
    let (body, footer) = body_and_footer(area, prompts.len() as u16 + 1);
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Left), body);
    render_footer(frame, &mut layout, footer, Alignment::Left, &prompts);
    layout
}

pub fn draw_debrief(
    frame: &mut Frame,
    headline: DebriefHeadline,
    summary: &ResolutionSummary,
    campaign: &CampaignState,
    turns: u32,
    seed: u64,
) -> ScreenLayout {
    let mut layout = ScreenLayout::default();
    let area = centered(frame.area(), 62, 20);
    let color = match headline.tone() {
        Tone::Good => Color::LightGreen,
        Tone::Mixed => Color::Yellow,
        Tone::Bad => Color::LightRed,
    };
    let mut lines = vec![
        Line::styled(
            headline.text().to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::from(trf!(
            "ui.debrief.result",
            result = summary.result.describe()
        )),
        Line::from(trf!("ui.debrief.payout", payout = summary.payout)),
    ];
    if let Some(reason) = &summary.breach_reason {
        lines.push(Line::styled(
            trf!("ui.debrief.breach", reason = reason),
            Style::default().fg(Color::LightRed),
        ));
    }
    if summary.fine > 0 {
        lines.push(Line::styled(
            trf!("ui.debrief.fine", fine = summary.fine),
            Style::default().fg(Color::LightRed),
        ));
    }
    if !summary.confiscated.is_empty() {
        lines.push(Line::styled(
            trf!(
                "ui.debrief.confiscated",
                items = summary.confiscated.join(", ")
            ),
            Style::default().fg(Color::LightRed),
        ));
    }
    if summary.district_heat_change > 0 {
        lines.push(Line::styled(
            tr!("ui.debrief.heat_rose").to_string(),
            Style::default().fg(Color::Yellow),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(trf!("ui.debrief.cash", cash = campaign.cash)));
    lines.push(Line::styled(
        murmur_core::loc::fmt(
            "ui.debrief.footer",
            &[("turns", &turns.to_string()), ("seed", &seed.to_string())],
        ),
        Style::default().fg(Color::DarkGray),
    ));

    let prompts = [(
        ShellInput::Enter,
        tr!("ui.debrief.prompt.back").to_string(),
        Style::default().fg(Color::LightGreen),
    )];
    frame.render_widget(Block::default().borders(Borders::ALL), area);
    let (body, footer) = body_and_footer(area, prompts.len() as u16 + 1);
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), body);
    render_footer(frame, &mut layout, footer, Alignment::Center, &prompts);
    layout
}

pub fn draw_campaign_over(frame: &mut Frame, campaign: &CampaignState) -> ScreenLayout {
    let mut layout = ScreenLayout::default();
    let area = centered(frame.area(), 64, 20);
    let completed = campaign
        .history
        .iter()
        .filter(|r| {
            matches!(
                r.result,
                murmur_campaign::ContractResult::Completed
                    | murmur_campaign::ContractResult::CompletedUnclean
            )
        })
        .count();
    let earned: i64 = campaign.history.iter().map(|r| r.payout).sum();
    let mut lines = vec![
        Line::styled(
            tr!("ui.campaign_over.title"),
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::from(murmur_core::loc::fmt(
            "ui.campaign_over.tally",
            &[
                ("taken", &campaign.history.len().to_string()),
                ("killed", &completed.to_string()),
            ],
        )),
        Line::from(trf!("ui.campaign_over.earned", earned = earned)),
        Line::raw(""),
    ];
    for record in campaign.history.iter().rev().take(6) {
        lines.push(Line::styled(
            murmur_core::loc::fmt(
                "ui.campaign_over.record",
                &[
                    ("venue", &record.venue),
                    ("district", &record.district),
                    ("result", record.result.describe()),
                ],
            ),
            Style::default().fg(Color::Gray),
        ));
    }
    let prompts = [
        (
            ShellInput::Enter,
            tr!("ui.campaign_over.prompt.again").to_string(),
            Style::default().fg(Color::LightGreen),
        ),
        (
            ShellInput::Char('q'),
            tr!("ui.campaign_over.prompt.quit").to_string(),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    frame.render_widget(Block::default().borders(Borders::ALL), area);
    let (body, footer) = body_and_footer(area, prompts.len() as u16 + 1);
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), body);
    render_footer(frame, &mut layout, footer, Alignment::Center, &prompts);
    layout
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_text_packs_words_to_width() {
        assert_eq!(wrap_text("one two three", 7), vec!["one two", "three"]);
        assert_eq!(
            wrap_text("word", 2),
            vec!["word"],
            "a word longer than the width still lands on its own line"
        );
        assert_eq!(wrap_text("", 10), vec![""], "empty text is one empty line");
        assert_eq!(
            wrap_text("a b", 0),
            vec!["a b"],
            "zero width means no wrapping at all"
        );
    }

    #[test]
    fn body_and_footer_split_a_bordered_panel() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 10,
        };
        let (body, footer) = body_and_footer(area, 3);
        assert_eq!((body.x, body.y, body.width, body.height), (1, 1, 18, 5));
        assert_eq!(
            (footer.x, footer.y, footer.width, footer.height),
            (1, 6, 18, 3)
        );
        // The footer never overflows a short panel's interior.
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 4,
        };
        let (body, footer) = body_and_footer(tiny, 5);
        assert_eq!(body.height, 0);
        assert_eq!(footer.height, 2);
    }

    #[test]
    fn centered_clamps_to_the_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 10,
        };
        let rect = centered(area, 10, 4);
        assert_eq!((rect.x, rect.y, rect.width, rect.height), (10, 3, 10, 4));
        let clamped = centered(area, 99, 99);
        assert_eq!(
            (clamped.width, clamped.height),
            (30, 10),
            "an oversized request fills the area instead of overflowing it"
        );
    }

    #[test]
    fn keymap_summary_lines_respect_the_width() {
        for line in keymap_summary(30) {
            assert!(
                line.chars().count() <= 30 || !line.contains("   "),
                "packed line '{line}' exceeds the width with room to split"
            );
        }
        assert!(!keymap_summary(30).is_empty());
    }
}

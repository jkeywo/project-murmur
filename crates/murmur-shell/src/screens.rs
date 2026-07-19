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

use crate::keymap;
use crate::{ScreenLayout, ShellInput};

const TITLE: &str = "P R O J E C T   M U R M U R";

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
            "Start over?",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw("This discards your saved campaign — the money,"),
        Line::raw("the kit, and everyone you have already dealt with."),
        Line::raw("There is only one save, and no way back."),
    ];
    let prompts = vec![
        (
            ShellInput::Char('y'),
            "y: yes, start over".to_string(),
            Style::default().fg(Color::Red),
        ),
        (
            ShellInput::Esc,
            "any other key: keep my campaign".to_string(),
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
        .map(|a| format!("{} {}", a.key, a.label))
        .collect();
    cells.extend(
        keymap::CONTROLS
            .iter()
            .map(|(key, short, _)| format!("{key} {short}")),
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
            TITLE,
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw("A social-stealth contract campaign. Take a job, slip in,"),
        Line::raw("eliminate the target, and walk out unremarked."),
        Line::raw(""),
        Line::raw("Blend in: your clothes decide where you belong."),
        Line::raw("Guards notice trespass, weapons, bodies, and worse."),
        Line::raw("Contracts carry one hard condition; break it, no pay."),
        Line::raw("Arrest costs your kit and a fine. Death ends everything."),
        Line::raw(""),
        Line::styled("keys", Style::default().add_modifier(Modifier::UNDERLINED)),
    ];
    // Built from the keymap table rather than written out again, so this
    // list cannot drift from what the keys actually do. Press ? in a
    // mission for the same bindings with full descriptions.
    lines.extend(keymap_summary(60).into_iter().map(Line::raw));
    lines.extend([
        Line::raw(""),
        Line::raw("Press ? during a mission for the full list."),
        Line::raw(""),
        Line::raw("The mouse works too: hover anything to inspect it,"),
        Line::raw("click a seen tile to walk there, and click any prompt"),
        Line::raw("or action instead of pressing its key."),
    ]);
    let green = Style::default().fg(Color::LightGreen);
    let mut prompts = vec![(
        ShellInput::Enter,
        if has_save {
            "Enter: continue your campaign".to_string()
        } else {
            "Enter: begin your campaign".to_string()
        },
        green,
    )];
    if has_save {
        prompts.push((ShellInput::Char('n'), "n: start over".to_string(), green));
    }
    prompts.push((
        ShellInput::Char('q'),
        "q: quit".to_string(),
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
        "THE SYNDICATE DESK",
        Style::default()
            .fg(Color::LightYellow)
            .add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(format!(
        "cash {}   contracts done {}",
        campaign.cash,
        campaign.history.len()
    )));
    lines.push(Line::from(
        campaign
            .district_heat
            .iter()
            .map(|(d, h)| format!("{d}: heat {h}"))
            .collect::<Vec<_>>()
            .join("   "),
    ));
    lines.push(Line::raw(""));

    if let Some(pending) = accepting {
        // Loadout selection.
        lines.push(Line::styled(
            format!(
                "LOADOUT for the {} job in {} (pick up to three)",
                pending.offer.venue, pending.offer.district
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
            (ShellInput::Enter, "Enter: take the job"),
            (ShellInput::Esc, "Esc: back to the board"),
        ] {
            let row = area.y + 1 + lines.len() as u16;
            layout.push(row, inner_x, area.x + area.width - 2, input);
            lines.push(Line::styled(text.to_string(), green));
        }
    } else {
        // The contract board.
        lines.push(Line::styled(
            "CONTRACTS ON OFFER",
            Style::default().add_modifier(Modifier::UNDERLINED),
        ));
        for (index, offer) in campaign.offers(data).iter().enumerate() {
            let key = (b'1' + index as u8) as char;
            push_key_row(
                &mut lines,
                &mut layout,
                key,
                format!(
                    "{} in {} - pays {}  (heat {})",
                    offer.venue, offer.district, offer.payout, offer.heat
                ),
                Style::default().add_modifier(Modifier::BOLD),
            );
            lines.push(Line::styled(
                format!("    the target {}", offer.hook),
                Style::default().fg(Color::Gray),
            ));
            // The board stays compact with the chip; the full condition
            // is spelled out on the briefing before you commit.
            lines.push(Line::styled(
                format!(
                    "    condition: {}",
                    offer.constraint.short(data, &offer.venue)
                ),
                Style::default().fg(Color::LightCyan),
            ));
        }
        push_key_row(
            &mut lines,
            &mut layout,
            'r',
            "let this board pass".to_string(),
            Style::default().fg(Color::DarkGray),
        );
        lines.push(Line::raw(""));

        // The stash and the shop.
        lines.push(Line::styled(
            "YOUR STASH",
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
            "nothing but your hands".to_string()
        } else {
            stash.join(", ")
        }));
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "THE FENCE (click or key to buy)",
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
                    format!("{name} - owned"),
                    Style::default().fg(Color::DarkGray),
                )
            } else {
                (
                    format!("{name} - {} ({})", entry.price, entry.approach.name()),
                    Style::default(),
                )
            };
            push_key_row(&mut lines, &mut layout, key, text, style);
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "1/2: study a contract        Esc: back        q: quit",
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
            "MISSION BRIEFING",
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            format!("contract {seed} - {} in {}", offer.venue, offer.district),
            Style::default().fg(Color::DarkGray),
        ),
        Line::raw(""),
    ];
    push_wrapped(
        &mut lines,
        format!("Target: {}", facts.target_name),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        format!("Reason: the target {}", offer.hook),
        plain,
        width,
    );
    let locations = if facts.target_locations.is_empty() {
        "unknown".to_string()
    } else {
        facts.target_locations.join(", ")
    };
    push_wrapped(
        &mut lines,
        format!("Likely locations: {locations}"),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        format!(
            "Condition: {}",
            offer.constraint.describe(data, &offer.venue)
        ),
        Style::default().fg(Color::LightCyan),
        width,
    );
    push_wrapped(
        &mut lines,
        format!("Payout on a clean job: {}", offer.payout),
        plain,
        width,
    );
    lines.push(Line::raw(""));
    push_wrapped(
        &mut lines,
        format!(
            "Security: {} guards on shift; {} staff; about {} guests",
            facts.guard_count, facts.staff_count, facts.civilian_count
        ),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        format!("Restricted areas: {}", facts.restricted_rooms.join(", ")),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        format!(
            "Disguises seen on site: {}",
            facts.available_disguises.join(", ")
        ),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        format!(
            "Places to hide a body: {} known containers",
            facts.container_count
        ),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        format!("Extraction: {}", facts.extraction_exits.join(" or ")),
        plain,
        width,
    );
    push_wrapped(
        &mut lines,
        if facts.opportunities.is_empty() {
            "Word on the inside: nothing unusual".to_string()
        } else {
            format!("Word on the inside: {}", facts.opportunities.join("; "))
        },
        plain,
        width,
    );
    lines.push(Line::raw(""));
    push_wrapped(
        &mut lines,
        if loadout_names.is_empty() {
            "You go in empty-handed.".to_string()
        } else {
            format!("You carry: {}", loadout_names.join(", "))
        },
        plain,
        width,
    );
    lines.push(Line::raw("You enter as a guest, in civilian clothes."));

    let green = Style::default().fg(Color::LightGreen);
    let prompts = [
        (ShellInput::Enter, "Enter: go in".to_string(), green),
        (
            ShellInput::Esc,
            "Esc: let the contract pass".to_string(),
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
    headline: &str,
    summary: &ResolutionSummary,
    campaign: &CampaignState,
    turns: u32,
    seed: u64,
) -> ScreenLayout {
    let mut layout = ScreenLayout::default();
    let area = centered(frame.area(), 62, 20);
    let color = match headline {
        "CONTRACT COMPLETED" => Color::LightGreen,
        "CONTRACT BREACHED" | "CONTRACT ABANDONED" => Color::Yellow,
        _ => Color::LightRed,
    };
    let mut lines = vec![
        Line::styled(
            headline.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::from(format!("the contract {}", summary.result.describe())),
        Line::from(format!("payout: {}", summary.payout)),
    ];
    if let Some(reason) = &summary.breach_reason {
        lines.push(Line::styled(
            format!("breached: {reason}"),
            Style::default().fg(Color::LightRed),
        ));
    }
    if summary.fine > 0 {
        lines.push(Line::styled(
            format!("fine paid: {}", summary.fine),
            Style::default().fg(Color::LightRed),
        ));
    }
    if !summary.confiscated.is_empty() {
        lines.push(Line::styled(
            format!("confiscated: {}", summary.confiscated.join(", ")),
            Style::default().fg(Color::LightRed),
        ));
    }
    if summary.district_heat_change > 0 {
        lines.push(Line::styled(
            "the district runs hotter now".to_string(),
            Style::default().fg(Color::Yellow),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(format!("cash: {}", campaign.cash)));
    lines.push(Line::styled(
        format!("{turns} turns   seed {seed}"),
        Style::default().fg(Color::DarkGray),
    ));

    let prompts = [(
        ShellInput::Enter,
        "Enter: back to the desk".to_string(),
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
            "THE CAMPAIGN ENDS HERE",
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::from(format!(
            "{} contracts taken, {} targets eliminated",
            campaign.history.len(),
            completed
        )),
        Line::from(format!("{earned} earned across the run")),
        Line::raw(""),
    ];
    for record in campaign.history.iter().rev().take(6) {
        lines.push(Line::styled(
            format!(
                "{} in {}: {}",
                record.venue,
                record.district,
                record.result.describe()
            ),
            Style::default().fg(Color::Gray),
        ));
    }
    let prompts = [
        (
            ShellInput::Enter,
            "Enter: a new operative takes the desk".to_string(),
            Style::default().fg(Color::LightGreen),
        ),
        (
            ShellInput::Char('q'),
            "q: quit".to_string(),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    frame.render_widget(Block::default().borders(Borders::ALL), area);
    let (body, footer) = body_and_footer(area, prompts.len() as u16 + 1);
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), body);
    render_footer(frame, &mut layout, footer, Alignment::Center, &prompts);
    layout
}

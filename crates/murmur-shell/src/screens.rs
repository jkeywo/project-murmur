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

use crate::HubLayout;

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

pub fn draw_start(frame: &mut Frame, has_save: bool) {
    let area = centered(frame.area(), 66, 24);
    let continue_line = if has_save {
        "Enter: continue your campaign        n: start over"
    } else {
        "Enter: begin your campaign"
    };
    let lines = vec![
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
        Line::raw("arrows move    . or Space wait    c crouch    r draw/holster"),
        Line::raw("o/k open/close door   g garrote   f shoot   p pickpocket"),
        Line::raw("d change disguise   b carry/drop body   h hide body"),
        Line::raw("l pick lock   t throw noisemaker   u use machine"),
        Line::raw("; look   [ ] speed   Esc cancel   Q abandon the run"),
        Line::raw(""),
        Line::raw("The mouse works too: hover anything to inspect it —"),
        Line::raw("hovering a person shows what they can see — and click"),
        Line::raw("anything with a key in front of it."),
        Line::raw(""),
        Line::styled(continue_line, Style::default().fg(Color::LightGreen)),
        Line::styled("q: quit", Style::default().fg(Color::DarkGray)),
    ];
    let widget = Paragraph::new(lines).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(widget, area);
}

/// The campaign hub: contract board, stash, shop, and district heat.
/// Records clickable rows; clicking one equals pressing its key.
pub fn draw_hub(
    frame: &mut Frame,
    data: &GameData,
    campaign: &CampaignState,
    accepting: Option<&crate::PendingAccept>,
    message: Option<&str>,
) -> HubLayout {
    let mut layout = HubLayout::default();
    let area = centered(frame.area(), 78, 34);
    let inner_x = area.x + 1;
    let mut lines: Vec<Line> = Vec::new();
    let key_style = Style::default()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD);

    // A row with a leading key, recorded for mouse hit-testing.
    let push_key_row =
        |lines: &mut Vec<Line>, layout: &mut HubLayout, key: char, text: String, style: Style| {
            let row = area.y + 1 + lines.len() as u16;
            let width = (text.chars().count() + 2) as u16;
            layout.actions.push((row, inner_x, inner_x + width, key));
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
        lines.push(Line::styled(
            "Enter: take the job        Esc: back to the board",
            Style::default().fg(Color::LightGreen),
        ));
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
) {
    let area = centered(frame.area(), 74, 28);
    let loadout_names: Vec<String> = loadout
        .iter()
        .map(|i| {
            data.item(i)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| i.clone())
        })
        .collect();
    let mut lines = vec![
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
        Line::from(format!("Target: {}", facts.target_name)),
        Line::from(format!("Reason: the target {}", offer.hook)),
        Line::from(format!(
            "Likely locations: {}",
            facts.target_locations.join(", ")
        )),
        Line::styled(
            format!(
                "Condition: {}",
                offer.constraint.describe(data, &offer.venue)
            ),
            Style::default().fg(Color::LightCyan),
        ),
        Line::from(format!("Payout on a clean job: {}", offer.payout)),
        Line::raw(""),
        Line::from(format!(
            "Security: {} guards on shift; {} staff; about {} guests",
            facts.guard_count, facts.staff_count, facts.civilian_count
        )),
        Line::from(format!(
            "Restricted areas: {}",
            facts.restricted_rooms.join(", ")
        )),
        Line::from(format!(
            "Disguises seen on site: {}",
            facts.available_disguises.join(", ")
        )),
        Line::from(format!(
            "Places to hide a body: {} known containers",
            facts.container_count
        )),
        Line::from(format!(
            "Extraction: {}",
            facts.extraction_exits.join(" or ")
        )),
        Line::from(if facts.opportunities.is_empty() {
            "Word on the inside: nothing unusual".to_string()
        } else {
            format!("Word on the inside: {}", facts.opportunities.join("; "))
        }),
        Line::raw(""),
        Line::from(if loadout_names.is_empty() {
            "You go in empty-handed.".to_string()
        } else {
            format!("You carry: {}", loadout_names.join(", "))
        }),
        Line::raw("You enter as a guest, in civilian clothes."),
        Line::raw(""),
        Line::styled(
            "Enter: go in        Esc: let it pass",
            Style::default().fg(Color::LightGreen),
        ),
    ];
    if facts.target_locations.is_empty() {
        lines[5] = Line::from("Likely locations: unknown");
    }
    // Wrap so the full spelled-out contract condition stays readable
    // rather than truncating at the panel edge.
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    frame.render_widget(widget, area);
}

pub fn draw_debrief(
    frame: &mut Frame,
    headline: &str,
    summary: &ResolutionSummary,
    campaign: &CampaignState,
    turns: u32,
    seed: u64,
) {
    let area = centered(frame.area(), 62, 16);
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
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Enter: back to the desk",
        Style::default().fg(Color::LightGreen),
    ));
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(widget, area);
}

pub fn draw_campaign_over(frame: &mut Frame, campaign: &CampaignState) {
    let area = centered(frame.area(), 64, 18);
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
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Enter: a new operative takes the desk        q: quit",
        Style::default().fg(Color::LightGreen),
    ));
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(widget, area);
}

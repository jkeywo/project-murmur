//! Ratatui rendering for the non-gameplay interface modes. These are
//! interface modes only: they never advance simulation time.

use murmur_core::world::{MissionFacts, MissionOutcome};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

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

pub fn draw_start(frame: &mut Frame) {
    let area = centered(frame.area(), 66, 22);
    let lines = vec![
        Line::styled(
            TITLE,
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw("A social-stealth infiltration. One nightclub, one target."),
        Line::raw(""),
        Line::raw("Blend in: your clothes decide where you belong."),
        Line::raw("Guards notice trespass, weapons, bodies, and worse."),
        Line::raw("Succeed: eliminate the target, then leave by any X exit."),
        Line::raw("Fail: your death, or an arrest you cannot talk out of."),
        Line::raw(""),
        Line::styled("keys", Style::default().add_modifier(Modifier::UNDERLINED)),
        Line::raw("arrows move    . or Space wait    c crouch    r draw/holster"),
        Line::raw("o/k open/close door   g garrote   f shoot   p pickpocket"),
        Line::raw("d change disguise   b carry/drop body   h hide body"),
        Line::raw("l pick lock   t throw noisemaker   ; look   [ ] speed"),
        Line::raw("Esc cancel   Q abandon run"),
        Line::raw(""),
        Line::raw("The mouse works too: hover anything to inspect it —"),
        Line::raw("hovering a person shows what they can see — and click"),
        Line::raw("an action in the sidebar to use it."),
        Line::raw(""),
        Line::styled(
            "Enter: tonight's briefing        q: quit",
            Style::default().fg(Color::LightGreen),
        ),
    ];
    let widget = Paragraph::new(lines).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(widget, area);
}

pub fn draw_briefing(frame: &mut Frame, facts: &MissionFacts, seed: u64) {
    let area = centered(frame.area(), 72, 24);
    let mut lines = vec![
        Line::styled(
            "MISSION BRIEFING",
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            format!("contract {seed}"),
            Style::default().fg(Color::DarkGray),
        ),
        Line::raw(""),
        Line::from(format!("Target: {}", facts.target_name)),
        Line::from(format!("Reason: the target {}", facts.target_reason)),
        Line::from(format!(
            "Likely locations: {}",
            facts.target_locations.join(", ")
        )),
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
        Line::raw(""),
        Line::raw("You carry a garrote and a silenced pistol (6 rounds)."),
        Line::raw("You enter as a guest, in civilian clothes."),
        Line::raw(""),
        Line::styled(
            "Enter: go in        Esc: back",
            Style::default().fg(Color::LightGreen),
        ),
    ];
    if facts.target_locations.is_empty() {
        lines[5] = Line::from("Likely locations: unknown");
    }
    let widget = Paragraph::new(lines).alignment(Alignment::Left).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(widget, area);
}

/// The precise reason a mission ended, per the outcome rules.
pub fn outcome_summary(outcome: &MissionOutcome, target_name: &str) -> (&'static str, String) {
    match outcome {
        MissionOutcome::Extracted => (
            "MISSION ACCOMPLISHED",
            format!("{target_name} eliminated; you extracted cleanly."),
        ),
        MissionOutcome::PlayerKilled => ("MISSION FAILED", "You were killed.".to_string()),
        MissionOutcome::Arrested => (
            "MISSION FAILED",
            "Arrested and dragged away; there is no talking out of this one.".to_string(),
        ),
    }
}

pub fn draw_game_over(frame: &mut Frame, headline: &str, summary: &str, turns: u32, seed: u64) {
    let area = centered(frame.area(), 60, 11);
    let color = if headline.contains("ACCOMPLISHED") {
        Color::LightGreen
    } else {
        Color::LightRed
    };
    let widget = Paragraph::new(vec![
        Line::styled(
            headline.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(summary.to_string()),
        Line::raw(""),
        Line::styled(
            format!("{turns} turns   seed {seed}"),
            Style::default().fg(Color::DarkGray),
        ),
        Line::raw(""),
        Line::styled(
            "Enter: return to start        q: quit",
            Style::default().fg(Color::LightGreen),
        ),
    ])
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(widget, area);
}

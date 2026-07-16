//! Ratatui rendering for the non-gameplay interface modes.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
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
    let area = centered(frame.area(), 62, 20);
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
        Line::raw("Blend in. Only guards escalate; only evidence betrays you."),
        Line::raw("Succeed: eliminate the target, then leave by any exit."),
        Line::raw("Fail: your death, or an arrest you cannot talk out of."),
        Line::raw(""),
        Line::styled("Keys", Style::default().add_modifier(Modifier::UNDERLINED)),
        Line::raw("arrows move   .  wait      c crouch    o/k open/close door"),
        Line::raw("g garrote     f  shoot     r draw/holster  p pickpocket"),
        Line::raw("d disguise    B  carry/drop body   h hide body   ; look"),
        Line::raw("Space pause/resume queue   Backspace undo   Esc clear"),
        Line::raw(""),
        Line::styled(
            "Enter: begin the mission        q: quit",
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

pub fn draw_placeholder_mission(frame: &mut Frame, seed: u64) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());
    let body = Paragraph::new(vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("mission seed "),
            Span::styled(format!("{seed}"), Style::default().fg(Color::Yellow)),
        ]),
        Line::raw(""),
        Line::raw("The nightclub is still under construction."),
        Line::raw("World generation arrives in the next vertical slice."),
        Line::raw(""),
        Line::raw("q: give up"),
    ])
    .alignment(Alignment::Center);
    frame.render_widget(body, layout[0]);
}

pub fn draw_game_over(frame: &mut Frame, summary: &str) {
    let area = centered(frame.area(), 56, 9);
    let widget = Paragraph::new(vec![
        Line::styled(
            "MISSION OVER",
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(summary.to_string()),
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

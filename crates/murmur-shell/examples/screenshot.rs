//! Developer tool: render shell screens headlessly and print them.
//!
//! ```text
//! cargo run -p murmur-shell --example screenshot -- 42
//! ```

use murmur_core::data::GameData;
use murmur_shell::{Shell, ShellInput};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn print_frame(terminal: &mut Terminal<TestBackend>, shell: &mut Shell, label: &str) {
    terminal.draw(|frame| shell.draw(frame)).unwrap();
    println!("=== {label} ===");
    let buffer = terminal.backend().buffer().clone();
    for y in 0..buffer.area.height {
        let mut row = String::new();
        for x in 0..buffer.area.width {
            row.push_str(buffer[(x, y)].symbol());
        }
        println!("{}", row.trim_end());
    }
}

fn main() {
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(42);
    let data = GameData::embedded().expect("embedded data");
    let mut shell = Shell::new(data, seed);
    let backend = TestBackend::new(130, 38);
    let mut terminal = Terminal::new(backend).unwrap();

    print_frame(&mut terminal, &mut shell, "start");
    shell.handle_input(ShellInput::Enter);
    print_frame(&mut terminal, &mut shell, "briefing");
    shell.handle_input(ShellInput::Enter);

    // Walk a little and let the club live for a while.
    for input in [
        ShellInput::Up,
        ShellInput::Up,
        ShellInput::Right,
        ShellInput::Right,
        ShellInput::Char('.'),
        ShellInput::Char('.'),
    ] {
        shell.handle_input(input);
    }
    for _ in 0..60 {
        shell.tick();
    }
    print_frame(&mut terminal, &mut shell, "playing");
}

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
    let mut shell = Shell::new(
        data,
        seed,
        Box::new(murmur_campaign::MemoryStore::default()),
    );
    let backend = TestBackend::new(130, 38);
    let mut terminal = Terminal::new(backend).unwrap();

    print_frame(&mut terminal, &mut shell, "start");
    shell.handle_input(ShellInput::Enter); // hub
    print_frame(&mut terminal, &mut shell, "hub");
    shell.handle_input(ShellInput::Char('1')); // study the first contract
    shell.handle_input(ShellInput::Enter); // take the job -> briefing
    print_frame(&mut terminal, &mut shell, "briefing");
    shell.handle_input(ShellInput::Enter); // go in

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

    // Reading a slot, the key list, and the guard on abandoning a run.
    shell.handle_input(ShellInput::Char('1'));
    print_frame(&mut terminal, &mut shell, "inventory slot inspected");
    shell.handle_input(ShellInput::Char('?'));
    print_frame(&mut terminal, &mut shell, "help");
    shell.handle_input(ShellInput::Esc); // dismiss help

    // Re-reading the job, and paging the view up a storey.
    shell.handle_input(ShellInput::Char('j'));
    print_frame(&mut terminal, &mut shell, "contract recalled");
    shell.handle_input(ShellInput::Esc);
    shell.handle_input(ShellInput::Char('<'));
    print_frame(&mut terminal, &mut shell, "viewing the floor above");
    shell.handle_input(ShellInput::Char('>'));

    // The debug switches, and the map with sight turned off.
    shell.handle_input(ShellInput::Char('C'));
    print_frame(&mut terminal, &mut shell, "cheat panel");
    shell.handle_input(ShellInput::Char('1')); // reveal the map
    // The toggle is an ordinary queued command, so it needs a turn to
    // resolve before the panel can show it on.
    for _ in 0..4 {
        shell.tick();
    }
    print_frame(&mut terminal, &mut shell, "cheat panel with one on");
    shell.handle_input(ShellInput::Esc);
    for _ in 0..4 {
        shell.tick();
    }
    print_frame(&mut terminal, &mut shell, "map revealed");

    shell.handle_input(ShellInput::Char('Q'));
    print_frame(&mut terminal, &mut shell, "abandon confirm");
}

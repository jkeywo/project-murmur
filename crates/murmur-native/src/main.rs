//! Native delivery: a Bevy executable that renders Project Murmur into the
//! terminal through bevy_ratatui. All game behaviour lives in
//! `murmur-shell`/`murmur-core`; this binary only bridges Bevy's runner and
//! crossterm's key events onto the shared shell.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy_ratatui::{RatatuiContext, RatatuiPlugins, event::KeyMessage};
use murmur_core::data::GameData;
use murmur_shell::{Shell, ShellInput};
use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

#[derive(Resource)]
struct ShellResource(Shell);

fn main() {
    let data = match GameData::embedded() {
        Ok(data) => data,
        Err(err) => {
            eprintln!("invalid embedded game data: {err}");
            std::process::exit(1);
        }
    };
    let seed = seed_from_args_or_clock();

    App::new()
        .add_plugins((
            MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(
                Duration::from_millis(16),
            )),
            RatatuiPlugins::default(),
        ))
        .insert_resource(ShellResource(Shell::new(data, seed)))
        .add_systems(PreUpdate, read_input)
        .add_systems(Update, (advance, draw).chain())
        .run();
}

/// `--seed <n>` pins the first mission seed for reproduction; otherwise the
/// wall clock provides one (the seed is shown in-game for replays).
fn seed_from_args_or_clock() -> u64 {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--seed"
            && let Some(value) = args.next()
            && let Ok(seed) = value.parse()
        {
            return seed;
        }
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0xC0FFEE)
}

fn read_input(
    mut shell: ResMut<ShellResource>,
    mut keys: MessageReader<KeyMessage>,
    mut exit: MessageWriter<AppExit>,
) {
    for key in keys.read() {
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            exit.write(AppExit::Success);
            return;
        }
        if let Some(input) = translate(key.code) {
            shell.0.handle_input(input);
        }
    }
    if shell.0.quit_requested() {
        exit.write(AppExit::Success);
    }
}

fn translate(code: KeyCode) -> Option<ShellInput> {
    match code {
        KeyCode::Up => Some(ShellInput::Up),
        KeyCode::Down => Some(ShellInput::Down),
        KeyCode::Left => Some(ShellInput::Left),
        KeyCode::Right => Some(ShellInput::Right),
        KeyCode::Enter => Some(ShellInput::Enter),
        KeyCode::Esc => Some(ShellInput::Esc),
        KeyCode::Backspace => Some(ShellInput::Backspace),
        KeyCode::Char(c) => Some(ShellInput::Char(c)),
        _ => None,
    }
}

fn advance(mut shell: ResMut<ShellResource>) {
    shell.0.tick();
}

fn draw(mut context: ResMut<RatatuiContext>, shell: Res<ShellResource>) -> Result {
    context.draw(|frame| shell.0.draw(frame))?;
    Ok(())
}

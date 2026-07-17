//! Native delivery: a Bevy executable that renders Project Murmur into the
//! terminal through bevy_ratatui. All game behaviour lives in
//! `murmur-shell`/`murmur-core`; this binary only bridges Bevy's runner and
//! crossterm's key events onto the shared shell.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy_ratatui::{
    RatatuiContext, RatatuiPlugins,
    event::{KeyMessage, MouseMessage},
};
use murmur_core::data::GameData;
use murmur_shell::{Shell, ShellInput};
use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};

#[derive(Resource)]
struct ShellResource(Shell);

/// The single-slot campaign save, in the platform data directory
/// (%APPDATA%\murmur on Windows, ~/.local/share/murmur elsewhere),
/// falling back to the working directory.
struct FileStore {
    path: std::path::PathBuf,
}

impl FileStore {
    fn new() -> Self {
        let base = std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from))
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
            })
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let dir = base.join("murmur");
        let _ = std::fs::create_dir_all(&dir);
        Self {
            path: dir.join("campaign.json"),
        }
    }
}

impl murmur_campaign::CampaignStore for FileStore {
    fn load(&self) -> Option<String> {
        std::fs::read_to_string(&self.path).ok()
    }

    fn save(&mut self, document: &str) {
        let _ = std::fs::write(&self.path, document);
    }

    fn clear(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

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
            RatatuiPlugins {
                enable_mouse_capture: true,
                ..Default::default()
            },
        ))
        .insert_resource(ShellResource(Shell::new(
            data,
            seed,
            Box::new(FileStore::new()),
        )))
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
    mut mice: MessageReader<MouseMessage>,
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
    for mouse in mice.read() {
        let input = match mouse.kind {
            MouseEventKind::Moved => Some(ShellInput::MouseMove {
                column: mouse.column,
                row: mouse.row,
            }),
            MouseEventKind::Down(MouseButton::Left) => Some(ShellInput::MouseClick {
                column: mouse.column,
                row: mouse.row,
            }),
            _ => None,
        };
        if let Some(input) = input {
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

fn draw(mut context: ResMut<RatatuiContext>, mut shell: ResMut<ShellResource>) -> Result {
    context.draw(|frame| shell.0.draw(frame))?;
    Ok(())
}

//! The backend-neutral game shell.
//!
//! This crate is the shared "presentation contract" between the native
//! terminal build (Bevy + bevy_ratatui) and the WebAssembly build
//! (Ratzilla): one screen state machine, one input vocabulary, one set of
//! ratatui widgets, and one cooperative batching policy. The platform
//! binaries only translate their backend's key events into [`ShellInput`],
//! call [`Shell::tick`] once per frame, and hand [`Shell::draw`] a ratatui
//! frame. Nothing in here may influence simulation results — rendering
//! cadence and input transport stay outside the deterministic core.

mod screens;

use murmur_core::data::GameData;
use murmur_core::rng::split_mix_64;
use ratatui::Frame;

/// Backend-neutral input events. Platform binaries map their own key event
/// types (crossterm, browser) onto this vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellInput {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Backspace,
    Char(char),
}

/// Which interface mode the shell is in. Start, briefing, and game-over are
/// interface modes only: they never advance simulation time.
#[derive(Clone, Debug)]
pub enum Screen {
    Start,
    Playing,
    GameOver { summary: String },
}

/// The shared game shell driven by both delivery targets.
pub struct Shell {
    data: GameData,
    mission_seed: u64,
    screen: Screen,
    quit_requested: bool,
}

impl Shell {
    /// Creates a shell on the start screen. `initial_seed` seeds the first
    /// mission; successive missions derive their seeds from it so a session
    /// stays reproducible from one number.
    pub fn new(data: GameData, initial_seed: u64) -> Self {
        Self {
            data,
            mission_seed: initial_seed,
            screen: Screen::Start,
            quit_requested: false,
        }
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    pub fn data(&self) -> &GameData {
        &self.data
    }

    pub fn mission_seed(&self) -> u64 {
        self.mission_seed
    }

    /// True once the player asked to leave the program (native builds exit;
    /// the web build ignores it).
    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// Handles one input event in the current screen.
    pub fn handle_input(&mut self, input: ShellInput) {
        match &self.screen {
            Screen::Start => match input {
                ShellInput::Enter => self.start_mission(),
                ShellInput::Char('q') => self.quit_requested = true,
                _ => {}
            },
            Screen::Playing => {
                // Gameplay input lands with the command queue in a later
                // vertical slice.
                if let ShellInput::Char('q') = input {
                    self.screen = Screen::GameOver {
                        summary: "Mission abandoned.".to_string(),
                    };
                }
            }
            Screen::GameOver { .. } => match input {
                ShellInput::Enter => {
                    self.mission_seed = split_mix_64(self.mission_seed);
                    self.screen = Screen::Start;
                }
                ShellInput::Char('q') => self.quit_requested = true,
                _ => {}
            },
        }
    }

    /// Runs one cooperative batch of simulation work. Called once per
    /// platform frame; does nothing outside active gameplay.
    pub fn tick(&mut self) {
        if let Screen::Playing = self.screen {
            // The turn driver arrives with the simulation slices.
        }
    }

    /// Renders the current screen into a ratatui frame.
    pub fn draw(&self, frame: &mut Frame) {
        match &self.screen {
            Screen::Start => screens::draw_start(frame),
            Screen::Playing => screens::draw_placeholder_mission(frame, self.mission_seed),
            Screen::GameOver { summary } => screens::draw_game_over(frame, summary),
        }
    }

    fn start_mission(&mut self) {
        // World generation arrives with the generator slice; until then the
        // playing screen is a placeholder so the loop is visible end to end.
        self.screen = Screen::Playing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> Shell {
        Shell::new(GameData::embedded().unwrap(), 1234)
    }

    #[test]
    fn starts_on_start_screen() {
        let shell = shell();
        assert!(matches!(shell.screen(), Screen::Start));
        assert!(!shell.quit_requested());
    }

    #[test]
    fn enter_starts_a_mission_and_q_quits_from_start() {
        let mut shell = shell();
        shell.handle_input(ShellInput::Enter);
        assert!(matches!(shell.screen(), Screen::Playing));

        let mut quitting = self::tests::shell();
        quitting.handle_input(ShellInput::Char('q'));
        assert!(quitting.quit_requested());
    }

    #[test]
    fn game_over_returns_to_start_with_a_fresh_seed() {
        let mut shell = shell();
        let first_seed = shell.mission_seed();
        shell.handle_input(ShellInput::Enter);
        shell.handle_input(ShellInput::Char('q'));
        assert!(matches!(shell.screen(), Screen::GameOver { .. }));
        shell.handle_input(ShellInput::Enter);
        assert!(matches!(shell.screen(), Screen::Start));
        assert_ne!(shell.mission_seed(), first_seed);
    }
}

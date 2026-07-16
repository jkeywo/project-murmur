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

pub mod mission;
pub mod queue;
mod render;
mod screens;

use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::rng::split_mix_64;
use murmur_core::turn::TurnDriver;
use ratatui::Frame;

use mission::Mission;

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

/// Which interface mode the shell is in. Start, briefing, and game-over
/// are interface modes only: they never advance simulation time.
pub enum Screen {
    Start,
    Briefing(Box<Mission>),
    Playing(Box<Mission>),
    GameOver {
        headline: &'static str,
        summary: String,
        turns: u32,
        seed: u64,
    },
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
    /// mission; successive missions derive their seeds from it so a
    /// session stays reproducible from one number.
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

    /// True once the player asked to leave the program (native builds
    /// exit; the web build ignores it).
    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// Handles one input event in the current screen.
    pub fn handle_input(&mut self, input: ShellInput) {
        match &mut self.screen {
            Screen::Start => match input {
                ShellInput::Enter => self.begin_briefing(),
                ShellInput::Char('q') => self.quit_requested = true,
                _ => {}
            },
            Screen::Briefing(_) => match input {
                ShellInput::Enter => {
                    let Screen::Briefing(mission) =
                        std::mem::replace(&mut self.screen, Screen::Start)
                    else {
                        unreachable!()
                    };
                    self.screen = Screen::Playing(mission);
                }
                ShellInput::Esc => self.screen = Screen::Start,
                _ => {}
            },
            Screen::Playing(mission) => {
                if input == ShellInput::Char('Q') {
                    let world = mission.world();
                    self.screen = Screen::GameOver {
                        headline: "MISSION ABANDONED",
                        summary: "You walked away from the contract.".to_string(),
                        turns: world.turn,
                        seed: world.seed,
                    };
                    self.mission_seed = split_mix_64(self.mission_seed);
                    return;
                }
                mission.handle_input(&self.data, input);
            }
            Screen::GameOver { .. } => match input {
                ShellInput::Enter => self.screen = Screen::Start,
                ShellInput::Char('q') => self.quit_requested = true,
                _ => {}
            },
        }
    }

    /// Runs one cooperative batch of simulation work. Called once per
    /// platform frame; does nothing outside active gameplay.
    pub fn tick(&mut self) {
        if let Screen::Playing(mission) = &mut self.screen {
            mission.tick(&self.data);
            if let Some(outcome) = mission.world().outcome.clone() {
                let target_name = mission.world().actor(mission.world().target).name.clone();
                let (headline, summary) = screens::outcome_summary(&outcome, &target_name);
                self.screen = Screen::GameOver {
                    headline,
                    summary,
                    turns: mission.world().turn,
                    seed: mission.world().seed,
                };
                self.mission_seed = split_mix_64(self.mission_seed);
            }
        }
    }

    /// Renders the current screen into a ratatui frame.
    pub fn draw(&self, frame: &mut Frame) {
        match &self.screen {
            Screen::Start => screens::draw_start(frame),
            Screen::Briefing(mission) => {
                screens::draw_briefing(frame, &mission.world().facts, mission.world().seed)
            }
            Screen::Playing(mission) => render::draw_mission(frame, &self.data, mission),
            Screen::GameOver {
                headline,
                summary,
                turns,
                seed,
            } => screens::draw_game_over(frame, headline, summary, *turns, *seed),
        }
    }

    fn begin_briefing(&mut self) {
        match generate(&self.data, self.mission_seed) {
            Ok(world) => {
                let driver = TurnDriver::new(world, &self.data);
                let mission = Mission::new(driver, &self.data);
                self.screen = Screen::Briefing(Box::new(mission));
            }
            Err(err) => {
                // Extraordinarily unlikely (generation retries internally);
                // derive a fresh seed rather than crashing the shell.
                let _ = err;
                self.mission_seed = split_mix_64(self.mission_seed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use murmur_core::actions::Command;

    fn shell() -> Shell {
        Shell::new(GameData::embedded().unwrap(), 1234)
    }

    fn start_mission(shell: &mut Shell) {
        shell.handle_input(ShellInput::Enter); // briefing
        assert!(matches!(shell.screen(), Screen::Briefing(_)));
        shell.handle_input(ShellInput::Enter); // play
        assert!(matches!(shell.screen(), Screen::Playing(_)));
    }

    fn mission(shell: &Shell) -> &Mission {
        match shell.screen() {
            Screen::Playing(mission) => mission,
            _ => panic!("not playing"),
        }
    }

    #[test]
    fn full_screen_flow_start_briefing_playing() {
        let mut shell = shell();
        assert!(matches!(shell.screen(), Screen::Start));
        start_mission(&mut shell);
        assert_eq!(mission(&shell).world().turn, 0);
    }

    #[test]
    fn queue_capacity_is_visible_and_overflow_rejects() {
        let mut shell = shell();
        start_mission(&mut shell);
        // Pause so nothing consumes, then stuff the queue past capacity.
        shell.handle_input(ShellInput::Char(' '));
        for _ in 0..40 {
            shell.handle_input(ShellInput::Char('.'));
        }
        let mission = mission(&shell);
        assert_eq!(mission.queue.len(), 32, "exactly 32 commands fit");
        assert!(
            mission
                .log
                .iter()
                .any(|line| line.contains("queue full (32/32)")),
            "overflow must be visible"
        );
    }

    #[test]
    fn escape_clears_and_backspace_removes_newest() {
        let mut shell = shell();
        start_mission(&mut shell);
        shell.handle_input(ShellInput::Char(' ')); // pause
        shell.handle_input(ShellInput::Char('.'));
        shell.handle_input(ShellInput::Up);
        assert_eq!(mission(&shell).queue.len(), 2);
        shell.handle_input(ShellInput::Backspace);
        assert_eq!(mission(&shell).queue.len(), 1);
        assert_eq!(mission(&shell).queue.head(), Some(&Command::Wait));
        shell.handle_input(ShellInput::Esc);
        assert!(mission(&shell).queue.is_empty());
    }

    #[test]
    fn paused_queue_consumes_nothing_until_resumed() {
        let mut shell = shell();
        start_mission(&mut shell);
        shell.handle_input(ShellInput::Char(' ')); // pause
        shell.handle_input(ShellInput::Char('.'));
        for _ in 0..10 {
            shell.tick();
        }
        assert_eq!(mission(&shell).world().turn, 0);
        assert_eq!(mission(&shell).queue.len(), 1);
        shell.handle_input(ShellInput::Char(' ')); // resume
        for _ in 0..10 {
            shell.tick();
        }
        assert_eq!(mission(&shell).queue.len(), 0);
        assert_eq!(mission(&shell).world().turn, 1);
    }

    #[test]
    fn look_mode_pauses_and_stays_paused_after_exit() {
        let mut shell = shell();
        start_mission(&mut shell);
        shell.handle_input(ShellInput::Char(';'));
        assert!(mission(&shell).queue.is_paused());
        assert!(matches!(mission(&shell).mode, mission::InputMode::Look(_)));
        // Cursor moves without enqueueing player movement.
        shell.handle_input(ShellInput::Up);
        assert!(mission(&shell).queue.is_empty());
        shell.handle_input(ShellInput::Esc);
        assert!(matches!(mission(&shell).mode, mission::InputMode::Normal));
        assert!(
            mission(&shell).queue.is_paused(),
            "leaving look mode keeps the queue paused until explicit resume"
        );
    }

    #[test]
    fn rejected_command_cancels_the_queued_remainder() {
        let mut shell = shell();
        start_mission(&mut shell);
        // Find a direction blocked by a wall.
        let world = mission(&shell).world();
        let from = world.player_actor().pos;
        let blocked = murmur_core::geom::Dir4::ALL.into_iter().find(|d| {
            matches!(
                world.map.tile(from.step(*d)),
                murmur_core::map::TileKind::Wall | murmur_core::map::TileKind::Void
            )
        });
        let Some(blocked) = blocked else { return };
        let input = match blocked {
            murmur_core::geom::Dir4::North => ShellInput::Up,
            murmur_core::geom::Dir4::South => ShellInput::Down,
            murmur_core::geom::Dir4::East => ShellInput::Right,
            murmur_core::geom::Dir4::West => ShellInput::Left,
        };
        shell.handle_input(ShellInput::Char(' ')); // pause to build a queue
        shell.handle_input(input); // will be rejected
        shell.handle_input(ShellInput::Char('.'));
        shell.handle_input(ShellInput::Char('.'));
        assert_eq!(mission(&shell).queue.len(), 3);
        shell.handle_input(ShellInput::Char(' ')); // resume
        for _ in 0..5 {
            shell.tick();
        }
        let mission = mission(&shell);
        assert_eq!(
            mission.world().turn,
            0,
            "rejection consumes no turn and cancels the remainder"
        );
        assert!(mission.queue.is_empty());
        assert!(
            mission
                .log
                .iter()
                .any(|line| line.contains("rejected") && line.contains("cancelled")),
            "the cancellation reason must be reported"
        );
    }

    #[test]
    fn game_over_screen_returns_to_start_with_fresh_seed() {
        let mut shell = shell();
        let first_seed = shell.mission_seed();
        start_mission(&mut shell);
        shell.handle_input(ShellInput::Char('Q'));
        assert!(matches!(shell.screen(), Screen::GameOver { .. }));
        assert_ne!(shell.mission_seed(), first_seed);
        shell.handle_input(ShellInput::Enter);
        assert!(matches!(shell.screen(), Screen::Start));
    }
}

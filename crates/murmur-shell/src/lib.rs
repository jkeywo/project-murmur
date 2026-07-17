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
//!
//! The campaign drives the flow: start → hub (contract board, shop,
//! loadout) → briefing → mission → debrief → hub, until the operative
//! dies and the campaign ends at a tally. The campaign autosaves through
//! the injected [`CampaignStore`] after every hub-level change.

pub mod mission;
pub mod queue;
mod render;
mod screens;

use murmur_campaign::{
    CampaignState, CampaignStore, ContractOffer, MissionResolution, ResolutionSummary,
};
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::rng::split_mix_64;
use murmur_core::turn::TurnDriver;
use ratatui::Frame;

use mission::Mission;

/// Backend-neutral input events. Platform binaries map their own key and
/// mouse event types (crossterm, browser) onto this vocabulary. Mouse
/// coordinates are terminal cells; the shell resolves them against the
/// layout of the last drawn frame.
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
    MouseMove { column: u16, row: u16 },
    MouseClick { column: u16, row: u16 },
}

/// Clickable rows recorded by the hub renderer: (row, x0, x1, key).
/// Clicking one is exactly pressing its key.
#[derive(Clone, Debug, Default)]
pub struct HubLayout {
    pub actions: Vec<(u16, u16, u16, char)>,
}

impl HubLayout {
    fn key_at(&self, column: u16, row: u16) -> Option<char> {
        self.actions
            .iter()
            .find(|(r, x0, x1, _)| *r == row && column >= *x0 && column <= *x1)
            .map(|(_, _, _, key)| *key)
    }
}

/// Loadout selection while accepting a contract.
#[derive(Clone, Debug)]
pub struct PendingAccept {
    pub offer: ContractOffer,
    /// Chosen item spec ids (at most three).
    pub chosen: Vec<String>,
}

/// Which interface mode the shell is in. Everything except `Playing` is
/// an interface mode only: it never advances simulation time.
pub enum Screen {
    Start,
    Hub {
        /// Set while picking a loadout for an accepted offer.
        accepting: Option<PendingAccept>,
        /// One-line feedback ("not enough cash").
        message: Option<String>,
    },
    Briefing {
        mission: Box<Mission>,
        offer: ContractOffer,
        loadout: Vec<String>,
    },
    Playing {
        mission: Box<Mission>,
        offer: ContractOffer,
        loadout: Vec<String>,
    },
    Debrief {
        headline: &'static str,
        summary: ResolutionSummary,
        turns: u32,
        seed: u64,
    },
    CampaignOver,
}

/// The shared game shell driven by both delivery targets.
pub struct Shell {
    data: GameData,
    campaign: CampaignState,
    store: Box<dyn CampaignStore>,
    screen: Screen,
    hub_layout: HubLayout,
    quit_requested: bool,
}

impl Shell {
    /// Creates a shell on the start screen. A saved campaign in the
    /// store resumes; otherwise `initial_seed` starts a fresh one.
    pub fn new(data: GameData, initial_seed: u64, store: Box<dyn CampaignStore>) -> Self {
        let campaign = store
            .load()
            .and_then(|doc| CampaignState::from_save(&doc))
            .filter(|c| !c.over)
            .unwrap_or_else(|| CampaignState::new(initial_seed, &data));
        Self {
            data,
            campaign,
            store,
            screen: Screen::Start,
            hub_layout: HubLayout::default(),
            quit_requested: false,
        }
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    pub fn data(&self) -> &GameData {
        &self.data
    }

    pub fn campaign(&self) -> &CampaignState {
        &self.campaign
    }

    /// True once the player asked to leave the program (native builds
    /// exit; the web build ignores it).
    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    fn autosave(&mut self) {
        self.store.save(&self.campaign.to_save());
    }

    /// Handles one input event in the current screen.
    pub fn handle_input(&mut self, input: ShellInput) {
        match &mut self.screen {
            Screen::Start => match input {
                ShellInput::Enter => {
                    self.screen = Screen::Hub {
                        accepting: None,
                        message: None,
                    };
                }
                ShellInput::Char('n') => {
                    // Abandon any saved campaign and start over.
                    let seed = split_mix_64(self.campaign.seed);
                    self.campaign = CampaignState::new(seed, &self.data);
                    self.store.clear();
                    self.autosave();
                    self.screen = Screen::Hub {
                        accepting: None,
                        message: Some("a fresh start".to_string()),
                    };
                }
                ShellInput::Char('q') => self.quit_requested = true,
                _ => {}
            },
            Screen::Hub { .. } => self.handle_hub_input(input),
            Screen::Briefing { .. } => match input {
                ShellInput::Enter => {
                    let Screen::Briefing {
                        mission,
                        offer,
                        loadout,
                    } = std::mem::replace(&mut self.screen, Screen::Start)
                    else {
                        unreachable!()
                    };
                    self.screen = Screen::Playing {
                        mission,
                        offer,
                        loadout,
                    };
                }
                ShellInput::Esc => {
                    // Walking away at the door: the contract slips by
                    // without penalty or record.
                    self.screen = Screen::Hub {
                        accepting: None,
                        message: Some("you let the contract pass".to_string()),
                    };
                }
                _ => {}
            },
            Screen::Playing { mission, .. } => {
                if input == ShellInput::Char('Q') {
                    let _ = mission;
                    self.finish_mission(None);
                    return;
                }
                let Screen::Playing { mission, .. } = &mut self.screen else {
                    unreachable!()
                };
                mission.handle_input(&self.data, input);
            }
            Screen::Debrief { .. } => {
                if input == ShellInput::Enter {
                    if self.campaign.over {
                        self.screen = Screen::CampaignOver;
                    } else {
                        self.screen = Screen::Hub {
                            accepting: None,
                            message: None,
                        };
                    }
                }
            }
            Screen::CampaignOver => match input {
                ShellInput::Enter => {
                    let seed = split_mix_64(self.campaign.seed);
                    self.campaign = CampaignState::new(seed, &self.data);
                    self.store.clear();
                    self.autosave();
                    self.screen = Screen::Start;
                }
                ShellInput::Char('q') => self.quit_requested = true,
                _ => {}
            },
        }
    }

    fn handle_hub_input(&mut self, input: ShellInput) {
        // Clicks route through the recorded hub layout: click == key.
        if let ShellInput::MouseClick { column, row } = input {
            if let Some(key) = self.hub_layout.key_at(column, row) {
                self.handle_hub_input(ShellInput::Char(key));
            }
            return;
        }
        let Screen::Hub { accepting, message } = &mut self.screen else {
            return;
        };
        *message = None;

        if let Some(pending) = accepting {
            // Loadout selection for the accepted offer.
            match input {
                ShellInput::Char(c @ '1'..='9') => {
                    let index = usize::from(c as u8 - b'1');
                    if let Some(item) = self.campaign.owned_equipment.get(index).cloned() {
                        if let Some(at) = pending.chosen.iter().position(|i| *i == item) {
                            pending.chosen.remove(at);
                        } else if pending.chosen.len() < murmur_core::contract::LOADOUT_SLOTS {
                            pending.chosen.push(item);
                        }
                    }
                }
                ShellInput::Enter => {
                    let pending = pending.clone();
                    self.launch_contract(pending);
                }
                ShellInput::Esc => *accepting = None,
                _ => {}
            }
            return;
        }

        match input {
            ShellInput::Char(c @ 'A'..='Z') if c != 'Q' => {}
            ShellInput::Char(c @ '1'..='9') => {
                let index = usize::from(c as u8 - b'1');
                let offers = self.campaign.offers(&self.data);
                if let Some(offer) = offers.get(index).cloned() {
                    // Default loadout: the first owned items, weapons first.
                    let mut chosen: Vec<String> = Vec::new();
                    let mut owned = self.campaign.owned_equipment.clone();
                    owned.sort_by_key(|i| {
                        std::cmp::Reverse(self.data.item(i).map(|s| s.weapon).unwrap_or(false))
                    });
                    for item in owned {
                        if chosen.len() < murmur_core::contract::LOADOUT_SLOTS {
                            chosen.push(item);
                        }
                    }
                    let Screen::Hub { accepting, .. } = &mut self.screen else {
                        unreachable!()
                    };
                    *accepting = Some(PendingAccept { offer, chosen });
                }
            }
            ShellInput::Char(c @ 'a'..='f') => {
                let index = usize::from(c as u8 - b'a');
                if let Some(entry) = self.data.equipment.get(index).cloned() {
                    let outcome = self.campaign.buy(&self.data, &entry.item);
                    let note = match outcome {
                        Ok(()) => {
                            self.autosave();
                            format!("bought: {}", entry.item)
                        }
                        Err(why) => why,
                    };
                    let Screen::Hub { message, .. } = &mut self.screen else {
                        unreachable!()
                    };
                    *message = Some(note);
                }
            }
            ShellInput::Char('r') => {
                self.campaign.refresh_offers();
                self.autosave();
                let Screen::Hub { message, .. } = &mut self.screen else {
                    unreachable!()
                };
                *message = Some("the board turns over".to_string());
            }
            ShellInput::Char('q') => self.quit_requested = true,
            ShellInput::Esc => self.screen = Screen::Start,
            _ => {}
        }
    }

    /// Accepts the offer, generates the mission, and opens the briefing.
    fn launch_contract(&mut self, pending: PendingAccept) {
        let config = match self.campaign.accept(&pending.offer, pending.chosen.clone()) {
            Ok(config) => config,
            Err(why) => {
                self.screen = Screen::Hub {
                    accepting: None,
                    message: Some(why),
                };
                return;
            }
        };
        self.autosave();
        match generate(&self.data, &config) {
            Ok(world) => {
                let driver = TurnDriver::new(world, &self.data);
                let mission = Mission::new(driver, &self.data);
                self.screen = Screen::Briefing {
                    mission: Box::new(mission),
                    offer: pending.offer,
                    loadout: pending.chosen,
                };
            }
            Err(_) => {
                // Extraordinarily unlikely (generation retries
                // internally): the contract falls through.
                self.screen = Screen::Hub {
                    accepting: None,
                    message: Some("the contract fell through; the board moves on".to_string()),
                };
            }
        }
    }

    /// Resolves the finished (or abandoned) mission into the campaign
    /// and shows the debrief. `outcome` is `None` for an abandoned run.
    fn finish_mission(&mut self, forced_outcome: Option<()>) {
        let _ = forced_outcome;
        let Screen::Playing {
            mission,
            offer,
            loadout,
        } = std::mem::replace(&mut self.screen, Screen::Start)
        else {
            return;
        };
        let world = mission.world();
        let resolution = MissionResolution {
            outcome: world.outcome.clone(),
            constraint_breached: world.constraint_breach.is_some(),
            mission_heat: world.mission_heat,
            loadout,
        };
        let headline = match &resolution.outcome {
            Some(murmur_core::world::MissionOutcome::Extracted)
                if !resolution.constraint_breached =>
            {
                "CONTRACT COMPLETED"
            }
            Some(murmur_core::world::MissionOutcome::Extracted) => "CONTRACT BREACHED",
            Some(murmur_core::world::MissionOutcome::Arrested) => "ARRESTED",
            Some(murmur_core::world::MissionOutcome::PlayerKilled) => "KILLED IN ACTION",
            None => "CONTRACT ABANDONED",
        };
        let summary = self.campaign.resolve(&self.data, &offer, &resolution);
        self.autosave();
        self.screen = Screen::Debrief {
            headline,
            summary,
            turns: world.turn,
            seed: world.seed,
        };
    }

    /// Runs one cooperative batch of simulation work. Called once per
    /// platform frame; does nothing outside active gameplay.
    pub fn tick(&mut self) {
        if let Screen::Playing { mission, .. } = &mut self.screen {
            mission.tick(&self.data);
            if mission.world().outcome.is_some() {
                self.finish_mission(None);
            }
        }
    }

    /// Renders the current screen into a ratatui frame. Takes `&mut self`
    /// because the mission and hub cache layouts for mouse hit-testing.
    pub fn draw(&mut self, frame: &mut Frame) {
        let data = &self.data;
        match &mut self.screen {
            Screen::Start => {
                screens::draw_start(frame, self.store.load().is_some());
            }
            Screen::Hub { accepting, message } => {
                self.hub_layout = screens::draw_hub(
                    frame,
                    data,
                    &self.campaign,
                    accepting.as_ref(),
                    message.as_deref(),
                );
            }
            Screen::Briefing {
                mission,
                offer,
                loadout,
            } => screens::draw_briefing(
                frame,
                data,
                &mission.world().facts,
                offer,
                loadout,
                mission.world().seed,
            ),
            Screen::Playing { mission, .. } => mission.draw(frame, data),
            Screen::Debrief {
                headline,
                summary,
                turns,
                seed,
            } => screens::draw_debrief(frame, headline, summary, &self.campaign, *turns, *seed),
            Screen::CampaignOver => screens::draw_campaign_over(frame, &self.campaign),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use murmur_campaign::MemoryStore;
    use murmur_core::actions::Command;

    fn shell() -> Shell {
        Shell::new(
            GameData::embedded().unwrap(),
            1234,
            Box::new(MemoryStore::default()),
        )
    }

    fn start_mission(shell: &mut Shell) {
        shell.handle_input(ShellInput::Enter); // start -> hub
        assert!(matches!(shell.screen(), Screen::Hub { .. }));
        shell.handle_input(ShellInput::Char('1')); // pick first offer
        assert!(matches!(
            shell.screen(),
            Screen::Hub {
                accepting: Some(_),
                ..
            }
        ));
        shell.handle_input(ShellInput::Enter); // confirm loadout -> briefing
        assert!(matches!(shell.screen(), Screen::Briefing { .. }));
        shell.handle_input(ShellInput::Enter); // go in
        assert!(matches!(shell.screen(), Screen::Playing { .. }));
    }

    fn mission(shell: &Shell) -> &Mission {
        match shell.screen() {
            Screen::Playing { mission, .. } => mission,
            _ => panic!("not playing"),
        }
    }

    #[test]
    fn full_flow_start_hub_briefing_playing() {
        let mut shell = shell();
        assert!(matches!(shell.screen(), Screen::Start));
        start_mission(&mut shell);
        assert_eq!(mission(&shell).world().turn, 0);
        // The accepted contract's constraint rides on the world.
        assert!(mission(&shell).world().constraint.is_some());
    }

    #[test]
    fn abandoning_resolves_into_a_debrief_and_back_to_the_hub() {
        let mut shell = shell();
        start_mission(&mut shell);
        shell.handle_input(ShellInput::Char('Q'));
        assert!(matches!(shell.screen(), Screen::Debrief { .. }));
        assert_eq!(shell.campaign().history.len(), 1);
        shell.handle_input(ShellInput::Enter);
        assert!(matches!(shell.screen(), Screen::Hub { .. }));
    }

    #[test]
    fn buying_from_the_shop_autosaves() {
        let mut shell = shell();
        shell.handle_input(ShellInput::Enter); // hub
        shell.campaign.cash = 10_000;
        // 'a' is the first catalogue entry (lockpicks).
        shell.handle_input(ShellInput::Char('a'));
        assert!(
            shell
                .campaign()
                .owned_equipment
                .iter()
                .any(|i| i == "lockpicks")
        );
        let saved = shell.store.load().expect("autosaved");
        let restored = CampaignState::from_save(&saved).unwrap();
        assert_eq!(restored.owned_equipment, shell.campaign().owned_equipment);
    }

    #[test]
    fn a_saved_campaign_resumes_across_shells() {
        let mut store = MemoryStore::default();
        {
            let data = GameData::embedded().unwrap();
            let mut state = CampaignState::new(77, &data);
            state.cash = 999;
            CampaignStore::save(&mut store, &state.to_save());
        }
        let shell = Shell::new(GameData::embedded().unwrap(), 1, Box::new(store));
        assert_eq!(shell.campaign().cash, 999, "the save wins over the seed");
    }

    #[test]
    fn queue_overflow_rejects_without_disturbing_queued_input() {
        let mut shell = shell();
        start_mission(&mut shell);
        for _ in 0..40 {
            shell.handle_input(ShellInput::Char('.'));
        }
        let mission = mission(&shell);
        assert_eq!(mission.queue.len(), 32, "exactly 32 commands fit");
        assert!(
            mission
                .log
                .iter()
                .any(|line| line.contains("can't plan any further")),
            "overflow is reported without queue jargon"
        );
    }

    #[test]
    fn space_waits_and_escape_clears_and_backspace_removes_newest() {
        let mut shell = shell();
        start_mission(&mut shell);
        shell.handle_input(ShellInput::Char(' '));
        assert_eq!(
            mission(&shell).queue.head(),
            Some(&Command::Wait),
            "space is wait, not a pause toggle"
        );
        shell.handle_input(ShellInput::Up);
        assert_eq!(mission(&shell).queue.len(), 2);
        shell.handle_input(ShellInput::Backspace);
        assert_eq!(mission(&shell).queue.len(), 1);
        assert_eq!(mission(&shell).queue.head(), Some(&Command::Wait));
        shell.handle_input(ShellInput::Esc);
        assert!(mission(&shell).queue.is_empty());
    }

    /// Round One bug: leaving look mode used to keep consumption paused,
    /// leaving the player unable to move. Look mode pauses internally and
    /// exit must resume.
    #[test]
    fn look_mode_pauses_internally_and_exit_resumes() {
        let mut shell = shell();
        start_mission(&mut shell);
        shell.handle_input(ShellInput::Char(';'));
        assert!(mission(&shell).queue.is_paused());
        assert!(matches!(mission(&shell).mode, mission::InputMode::Look(_)));
        shell.handle_input(ShellInput::Up);
        assert!(mission(&shell).queue.is_empty());
        for _ in 0..10 {
            shell.tick();
        }
        assert_eq!(mission(&shell).world().turn, 0);
        shell.handle_input(ShellInput::Esc);
        assert!(matches!(mission(&shell).mode, mission::InputMode::Normal));
        assert!(
            !mission(&shell).queue.is_paused(),
            "leaving look mode must never strand the player in a paused state"
        );
        shell.handle_input(ShellInput::Char('.'));
        for _ in 0..10 {
            shell.tick();
        }
        assert_eq!(mission(&shell).world().turn, 1, "play resumes immediately");
    }

    #[test]
    fn clicking_an_action_matches_pressing_its_key_and_hover_inspects() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut shell = shell();
        start_mission(&mut shell);
        let mut terminal = Terminal::new(TestBackend::new(110, 40)).unwrap();
        terminal.draw(|frame| shell.draw(frame)).unwrap();

        let (row, x0, _, key) = *mission(&shell)
            .ui
            .actions
            .iter()
            .find(|(_, _, _, key)| *key == '.')
            .expect("wait is in the palette");
        assert_eq!(key, '.');
        shell.handle_input(ShellInput::MouseClick { column: x0, row });
        assert_eq!(mission(&shell).queue.head(), Some(&Command::Wait));

        let ui = mission(&shell).ui.clone();
        let origin = ui.origin.unwrap();
        let player_pos = mission(&shell).world().player_actor().pos;
        let column = ui.map_x + u16::try_from(player_pos.x - origin.x).unwrap();
        let row = ui.map_y + u16::try_from(player_pos.y - origin.y).unwrap();
        shell.handle_input(ShellInput::MouseMove { column, row });
        assert_eq!(mission(&shell).hover, Some(player_pos));
        assert!(!mission(&shell).queue.is_paused(), "hover never pauses");
    }

    #[test]
    fn clicking_a_hub_row_matches_its_key() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut shell = shell();
        shell.handle_input(ShellInput::Enter); // hub
        let mut terminal = Terminal::new(TestBackend::new(110, 40)).unwrap();
        terminal.draw(|frame| shell.draw(frame)).unwrap();
        let (row, x0, _, _) = *shell
            .hub_layout
            .actions
            .iter()
            .find(|(_, _, _, key)| *key == '1')
            .expect("the first offer is clickable");
        shell.handle_input(ShellInput::MouseClick { column: x0, row });
        assert!(matches!(
            shell.screen(),
            Screen::Hub {
                accepting: Some(_),
                ..
            }
        ));
    }
}

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

pub mod keymap;
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
use ratatui::layout::Rect;

use mission::Mission;
use murmur_core::tr;

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

/// Clickable rows recorded by an interface screen. Clicking a row is
/// exactly the input it carries, so every prompt on every screen — keys,
/// Enter, Esc — is reachable with the mouse.
#[derive(Clone, Debug, Default)]
pub struct ScreenLayout {
    pub actions: Vec<(u16, u16, u16, ShellInput)>,
}

impl ScreenLayout {
    /// Records a clickable span on `row` from `x0` to `x1` inclusive.
    pub fn push(&mut self, row: u16, x0: u16, x1: u16, input: ShellInput) {
        self.actions.push((row, x0, x1, input));
    }

    /// Records a whole-width row: forgiving targets for centred prompts.
    pub fn push_row(&mut self, area: Rect, row: u16, input: ShellInput) {
        self.push(row, area.x, area.x + area.width.saturating_sub(1), input);
    }

    fn input_at(&self, column: u16, row: u16) -> Option<ShellInput> {
        self.actions
            .iter()
            .find(|(r, x0, x1, _)| *r == row && column >= *x0 && column <= *x1)
            .map(|(_, _, _, input)| *input)
    }
}

/// Loadout selection while accepting a contract.
#[derive(Clone, Debug)]
pub struct PendingAccept {
    pub offer: ContractOffer,
    /// Chosen item spec ids (at most three).
    pub chosen: Vec<String>,
}

/// The debrief's headline, and with it the tone the panel is drawn in.
///
/// A typed outcome rather than the headline string: `draw_debrief` used to
/// choose its colour by comparing the text against literals, so translating
/// the headline would have silently turned every debrief red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebriefHeadline {
    Completed,
    Breached,
    TargetEscaped,
    Arrested,
    Killed,
    Abandoned,
}

impl DebriefHeadline {
    pub fn text(self) -> &'static str {
        match self {
            DebriefHeadline::Completed => tr!("ui.debrief.headline.completed"),
            DebriefHeadline::Breached => tr!("ui.debrief.headline.breached"),
            DebriefHeadline::TargetEscaped => tr!("ui.debrief.headline.target_escaped"),
            DebriefHeadline::Arrested => tr!("ui.debrief.headline.arrested"),
            DebriefHeadline::Killed => tr!("ui.debrief.headline.killed"),
            DebriefHeadline::Abandoned => tr!("ui.debrief.headline.abandoned"),
        }
    }

    /// Whether the run ended well, tolerably, or badly.
    pub fn tone(self) -> Tone {
        match self {
            DebriefHeadline::Completed => Tone::Good,
            DebriefHeadline::Breached | DebriefHeadline::Abandoned => Tone::Mixed,
            DebriefHeadline::TargetEscaped
            | DebriefHeadline::Arrested
            | DebriefHeadline::Killed => Tone::Bad,
        }
    }
}

/// How a debrief outcome should read at a glance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Mixed,
    Bad,
}

/// Which interface mode the shell is in. Everything except `Playing` is
/// an interface mode only: it never advances simulation time.
pub enum Screen {
    Start,
    /// Guarding the one irreversible choice on the start screen: there is
    /// a single save slot, so starting over destroys the only campaign.
    ConfirmNewCampaign,
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
        headline: DebriefHeadline,
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
    screen_layout: ScreenLayout,
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
            screen_layout: ScreenLayout::default(),
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

    /// Discards any saved campaign and opens a fresh one at the hub.
    fn start_new_campaign(&mut self) {
        let seed = split_mix_64(self.campaign.seed);
        self.campaign = CampaignState::new(seed, &self.data);
        self.store.clear();
        self.autosave();
        self.screen = Screen::Hub {
            accepting: None,
            message: Some(tr!("ui.hub.msg.fresh_start").to_string()),
        };
    }

    /// Handles one input event in the current screen.
    pub fn handle_input(&mut self, input: ShellInput) {
        // Outside the mission every screen is a list of prompts: a click
        // resolves to the input of the row it lands on, so the mouse
        // drives the campaign exactly like the keyboard. The mission
        // handles its own clicks (map and action palette).
        let input = match (&self.screen, input) {
            (Screen::Playing { .. }, _) => input,
            (_, ShellInput::MouseClick { column, row }) => {
                match self.screen_layout.input_at(column, row) {
                    Some(resolved) => resolved,
                    None => return,
                }
            }
            _ => input,
        };
        match &mut self.screen {
            Screen::Start => match input {
                ShellInput::Enter => {
                    self.screen = Screen::Hub {
                        accepting: None,
                        message: None,
                    };
                }
                ShellInput::Char('n') => {
                    // There is one save slot and no undo, so starting over
                    // is unrecoverable: ask before discarding a campaign.
                    // With nothing saved there is nothing to lose, so the
                    // prompt only appears when it would actually protect
                    // something.
                    if self.store.load().is_some() {
                        self.screen = Screen::ConfirmNewCampaign;
                    } else {
                        self.start_new_campaign();
                    }
                }
                ShellInput::Char('q') => self.quit_requested = true,
                _ => {}
            },
            Screen::ConfirmNewCampaign => match input {
                ShellInput::Char('y') | ShellInput::Enter => self.start_new_campaign(),
                _ => self.screen = Screen::Start,
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
                        message: Some(tr!("ui.hub.msg.contract_passed").to_string()),
                    };
                }
                _ => {}
            },
            Screen::Playing { mission, .. } => {
                // Q asks first: under permadeath a mistyped key should not
                // be able to end a run. The mission owns the prompt; the
                // shell carries out the answer.
                mission.handle_input(&self.data, input);
                if mission.take_confirmed() == Some(mission::ConfirmAction::AbandonRun) {
                    self.finish_mission();
                }
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
        // Clicks were already resolved to their row's input upstream.
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
                            murmur_core::trf!("ui.hub.msg.bought", item = entry.item)
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
                *message = Some(tr!("ui.hub.msg.board_refreshed").to_string());
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
                    message: Some(tr!("ui.hub.msg.generation_failed").to_string()),
                };
            }
        }
    }

    /// Resolves the finished (or abandoned) mission into the campaign
    /// and shows the debrief.
    fn finish_mission(&mut self) {
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
            breach_reason: world.constraint_breach.clone(),
            mission_heat: world.mission_heat,
            loadout,
        };
        let headline = match &resolution.outcome {
            Some(murmur_core::world::MissionOutcome::Extracted)
                if resolution.breach_reason.is_none() =>
            {
                DebriefHeadline::Completed
            }
            Some(murmur_core::world::MissionOutcome::Extracted) => DebriefHeadline::Breached,
            Some(murmur_core::world::MissionOutcome::TargetEscaped) => {
                DebriefHeadline::TargetEscaped
            }
            Some(murmur_core::world::MissionOutcome::Arrested) => DebriefHeadline::Arrested,
            Some(murmur_core::world::MissionOutcome::PlayerKilled) => DebriefHeadline::Killed,
            None => DebriefHeadline::Abandoned,
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
                self.finish_mission();
            }
        }
    }

    /// Renders the current screen into a ratatui frame. Takes `&mut self`
    /// because the mission and hub cache layouts for mouse hit-testing.
    pub fn draw(&mut self, frame: &mut Frame) {
        let data = &self.data;
        match &mut self.screen {
            Screen::Start => {
                self.screen_layout = screens::draw_start(frame, self.store.load().is_some());
            }
            Screen::ConfirmNewCampaign => {
                self.screen_layout = screens::draw_confirm_new_campaign(frame);
            }
            Screen::Hub { accepting, message } => {
                self.screen_layout = screens::draw_hub(
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
            } => {
                self.screen_layout = screens::draw_briefing(
                    frame,
                    data,
                    &mission.world().facts,
                    offer,
                    loadout,
                    mission.world().seed,
                );
            }
            Screen::Playing { mission, .. } => mission.draw(frame, data),
            Screen::Debrief {
                headline,
                summary,
                turns,
                seed,
            } => {
                self.screen_layout =
                    screens::draw_debrief(frame, *headline, summary, &self.campaign, *turns, *seed);
            }
            Screen::CampaignOver => {
                self.screen_layout = screens::draw_campaign_over(frame, &self.campaign);
            }
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
    fn targeted_actions_with_no_target_are_unavailable_and_report_why() {
        let mut shell = shell();
        start_mission(&mut shell);
        let data = shell.data().clone();
        // The default loadout carries no lockpicks, so pick-lock can
        // never have a valid target; wait always can.
        assert!(!mission(&shell).action_available(&data, 'l'));
        assert!(mission(&shell).action_available(&data, '.'));
        // Pressing the unavailable key reports why and stays in normal
        // mode rather than entering a dead targeting prompt.
        let before = mission(&shell).log.len();
        shell.handle_input(ShellInput::Char('l'));
        assert!(matches!(mission(&shell).mode, mission::InputMode::Normal));
        let log = &mission(&shell).log;
        assert!(log.len() > before);
        assert!(log.last().unwrap().text.contains("lockpicks"));
    }

    #[test]
    fn abandoning_resolves_into_a_debrief_and_back_to_the_hub() {
        let mut shell = shell();
        start_mission(&mut shell);
        // Q asks before it ends the run.
        shell.handle_input(ShellInput::Char('Q'));
        assert!(matches!(shell.screen(), Screen::Playing { .. }));
        shell.handle_input(ShellInput::Char('y'));
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
                .any(|line| line.text.contains("can't plan any further")),
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

    /// Help pauses to be read and resumes on exit, exactly as look does.
    /// A mode that pauses without resuming is indistinguishable from a
    /// hang, because the queue has no visible state.
    #[test]
    fn help_pauses_internally_and_any_key_resumes() {
        let mut shell = shell();
        start_mission(&mut shell);
        shell.handle_input(ShellInput::Char('?'));
        assert!(matches!(mission(&shell).mode, mission::InputMode::Help));
        assert!(mission(&shell).queue.is_paused());
        for _ in 0..10 {
            shell.tick();
        }
        assert_eq!(mission(&shell).world().turn, 0, "help holds the run still");
        // Any key at all closes it.
        shell.handle_input(ShellInput::Char('z'));
        assert!(matches!(mission(&shell).mode, mission::InputMode::Normal));
        assert!(
            !mission(&shell).queue.is_paused(),
            "leaving help must never strand the player in a paused state"
        );
        shell.handle_input(ShellInput::Char('.'));
        for _ in 0..10 {
            shell.tick();
        }
        assert_eq!(mission(&shell).world().turn, 1, "play resumes immediately");
    }

    /// Abandoning a run is unrecoverable under permadeath, so Q asks. Both
    /// answers must leave the prompt and resume.
    #[test]
    fn abandoning_asks_first_and_declining_resumes() {
        let mut shell = shell();
        start_mission(&mut shell);
        shell.handle_input(ShellInput::Char('Q'));
        assert!(
            matches!(mission(&shell).mode, mission::InputMode::Confirm { .. }),
            "Q must not end the run on its own"
        );
        assert!(mission(&shell).queue.is_paused());
        // Anything that is not a yes declines.
        shell.handle_input(ShellInput::Char('n'));
        assert!(matches!(shell.screen(), Screen::Playing { .. }));
        assert!(matches!(mission(&shell).mode, mission::InputMode::Normal));
        assert!(
            !mission(&shell).queue.is_paused(),
            "declining must never strand the player in a paused state"
        );
        shell.handle_input(ShellInput::Char('.'));
        for _ in 0..10 {
            shell.tick();
        }
        assert_eq!(mission(&shell).world().turn, 1, "play resumes immediately");
    }

    /// Starting over destroys the only save, so it asks — but only when
    /// there is actually a campaign to lose.
    #[test]
    fn starting_over_asks_before_discarding_a_save() {
        let mut shell = shell();
        // A fresh shell has saved nothing yet: nothing to protect.
        shell.handle_input(ShellInput::Char('n'));
        assert!(matches!(shell.screen(), Screen::Hub { .. }));
        // Now a campaign exists, so the same key asks first.
        shell.handle_input(ShellInput::Esc);
        let seed_before = shell.campaign().seed;
        shell.screen = Screen::Start;
        shell.handle_input(ShellInput::Char('n'));
        assert!(matches!(shell.screen(), Screen::ConfirmNewCampaign));
        shell.handle_input(ShellInput::Char('x')); // anything but yes
        assert!(matches!(shell.screen(), Screen::Start));
        assert_eq!(
            shell.campaign().seed,
            seed_before,
            "declining must leave the campaign untouched"
        );
        // Confirming goes through.
        shell.handle_input(ShellInput::Char('n'));
        shell.handle_input(ShellInput::Char('y'));
        assert!(matches!(shell.screen(), Screen::Hub { .. }));
        assert_ne!(shell.campaign().seed, seed_before);
    }

    /// Every key in the palette must have a dispatch arm. The table and
    /// the match are separate, so this is what stops them drifting: each
    /// key has to visibly do something from a clean normal-mode state.
    #[test]
    fn keymap_matches_dispatch() {
        for entry in keymap::ACTIONS {
            let mut shell = shell();
            start_mission(&mut shell);
            let before_log = mission(&shell).log.len();
            let before_queue = mission(&shell).queue.len();
            shell.handle_input(ShellInput::Char(entry.key));
            let after = mission(&shell);
            let acted = !matches!(after.mode, mission::InputMode::Normal)
                || after.queue.len() != before_queue
                || after.log.len() != before_log;
            assert!(
                acted,
                "key {:?} ({}) is in the keymap but handle_normal ignores it",
                entry.key,
                entry.label()
            );
        }
    }

    /// Identical consecutive messages collapse instead of scrolling an
    /// eight-row panel clean.
    #[test]
    fn repeated_log_lines_collapse_into_a_count() {
        let mut shell = shell();
        start_mission(&mut shell);
        let data = shell.data().clone();
        // Pick-lock is unavailable with the default loadout, so pressing
        // it repeatedly produces the same refusal every time.
        assert!(!mission(&shell).action_available(&data, 'l'));
        for _ in 0..3 {
            shell.handle_input(ShellInput::Char('l'));
        }
        let log = &mission(&shell).log;
        let last = log.last().unwrap();
        assert_eq!(last.count, 3, "three identical refusals are one entry");
        assert_eq!(last.kind, mission::LogKind::Notice);
        // `contains` rather than `ends_with`: the repeat marker is itself a
        // catalogue string, so what trails the count is up to the text.
        assert!(last.display().contains("(x3)"));
        // A different message still starts a new entry.
        let entries = log.len();
        shell.handle_input(ShellInput::Char('g'));
        assert!(mission(&shell).log.len() > entries);
    }

    /// Reading an inventory slot is inspection: it costs no time and
    /// produces no action.
    #[test]
    fn inspecting_an_inventory_slot_costs_nothing() {
        let mut shell = shell();
        start_mission(&mut shell);
        let data = shell.data().clone();
        shell.handle_input(ShellInput::Char('1'));
        assert!(matches!(mission(&shell).mode, mission::InputMode::Normal));
        assert!(mission(&shell).queue.is_empty());
        let text = mission(&shell).inspected_slot_text(&data);
        assert!(text.is_some_and(|t| t.contains("slot 1:")));
        for _ in 0..10 {
            shell.tick();
        }
        assert_eq!(mission(&shell).world().turn, 0, "reading a slot is free");
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
            .screen_layout
            .actions
            .iter()
            .find(|(_, _, _, input)| *input == ShellInput::Char('1'))
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

    /// Every interface screen's prompts are clickable, not just the hub's
    /// keyed rows: Enter and Esc included.
    #[test]
    fn prompts_on_every_campaign_screen_are_clickable() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(110, 44)).unwrap();
        let mut shell = shell();

        // Start screen: clicking "Enter: ..." opens the hub.
        terminal.draw(|frame| shell.draw(frame)).unwrap();
        let (row, x0, _, _) = *shell
            .screen_layout
            .actions
            .iter()
            .find(|(_, _, _, input)| *input == ShellInput::Enter)
            .expect("the start screen offers a clickable Enter prompt");
        shell.handle_input(ShellInput::MouseClick { column: x0, row });
        assert!(matches!(shell.screen(), Screen::Hub { .. }));

        // Briefing: clicking "Esc: let the contract pass" returns to the
        // hub without taking the job.
        shell.handle_input(ShellInput::Char('1')); // pick the first offer
        shell.handle_input(ShellInput::Enter); // take it with no loadout
        assert!(matches!(shell.screen(), Screen::Briefing { .. }));
        terminal.draw(|frame| shell.draw(frame)).unwrap();
        let (row, x0, _, _) = *shell
            .screen_layout
            .actions
            .iter()
            .find(|(_, _, _, input)| *input == ShellInput::Esc)
            .expect("the briefing offers a clickable Esc prompt");
        shell.handle_input(ShellInput::MouseClick { column: x0, row });
        assert!(matches!(shell.screen(), Screen::Hub { .. }));
    }

    /// Clicking a tile you have already seen walks one step towards it.
    #[test]
    fn clicking_a_seen_tile_steps_towards_it() {
        use murmur_core::actions::Command;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut shell = shell();
        start_mission(&mut shell);
        let mut terminal = Terminal::new(TestBackend::new(110, 44)).unwrap();
        terminal.draw(|frame| shell.draw(frame)).unwrap();

        // Find a seen floor tile a few steps off, and the screen cell it
        // was drawn in.
        let ui = mission(&shell).ui.clone();
        let origin = ui.origin.expect("the map was drawn");
        let start = mission(&shell).world().player_actor().pos;
        let goal = (2..6)
            .find_map(|d| {
                let candidate = murmur_core::geom::Pos::new(start.floor, start.x + d, start.y);
                mission(&shell).is_explored(candidate).then_some(candidate)
            })
            .expect("some explored tile lies east of the spawn");
        let column = ui.map_x + u16::try_from(goal.x - origin.x).unwrap();
        let row = ui.map_y + u16::try_from(goal.y - origin.y).unwrap();

        shell.handle_input(ShellInput::MouseClick { column, row });
        assert_eq!(
            mission(&shell).queue.head(),
            Some(&Command::Move(murmur_core::geom::Dir4::East)),
            "the click queues a single step along the path"
        );

        // An unseen tile is not a destination.
        let unseen = murmur_core::geom::Pos::new(start.floor, 0, 0);
        assert!(!mission(&shell).is_explored(unseen));
    }
}

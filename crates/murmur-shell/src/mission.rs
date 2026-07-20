//! The in-mission controller: owns the turn driver and the command queue,
//! translates key and mouse input into queued commands, and paces
//! cooperative simulation batches. Rendering cadence never changes
//! simulation results; it only decides how many due turns run per
//! presentation frame.
//!
//! The command queue itself is deliberately not player-facing: it is an
//! architectural mechanism (intentions executed one per turn), surfaced
//! only through its effects. Look mode pauses consumption internally and
//! resumes it on exit, so the player is never left in an invisible paused
//! state.

use murmur_core::actions::{ActionResult, Command};
use murmur_core::data::GameData;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::map::TileKind;
use murmur_core::path::first_step_towards;
use murmur_core::turn::{TurnDriver, TurnReport};
use murmur_core::world::{ActorId, FurnitureKind, Hands, World};

use murmur_core::{tr, trf};

use crate::ShellInput;
use crate::queue::{CommandQueue, EnqueueOutcome};

/// Actions that need a follow-up direction key (or a click on an adjacent
/// tile) to pick their target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingAction {
    Garrote,
    Pickpocket,
    Disguise,
    CarryBody,
    HideBody,
    DropBody,
    OpenDoor,
    CloseDoor,
    PickLock,
    UseMachine,
}

impl PendingAction {
    pub fn prompt(self) -> &'static str {
        match self {
            PendingAction::Garrote => tr!("mission.prompt.garrote"),
            PendingAction::Pickpocket => tr!("mission.prompt.pickpocket"),
            PendingAction::Disguise => tr!("mission.prompt.disguise"),
            PendingAction::CarryBody => tr!("mission.prompt.carry_body"),
            PendingAction::HideBody => tr!("mission.prompt.hide_body"),
            PendingAction::DropBody => tr!("mission.prompt.drop_body"),
            PendingAction::OpenDoor => tr!("mission.prompt.open_door"),
            PendingAction::CloseDoor => tr!("mission.prompt.close_door"),
            PendingAction::PickLock => tr!("mission.prompt.pick_lock"),
            PendingAction::UseMachine => tr!("mission.prompt.use_machine"),
        }
    }
}

/// The mission input mode.
///
/// Every mode that pauses queue consumption must resume it on exit. With
/// no visible queue state, a pause the player cannot see and cannot leave
/// is indistinguishable from a hang.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Pending(PendingAction),
    Look(Pos),
    /// Aiming a noisemaker throw with a free cursor.
    ThrowTarget(Pos),
    TargetSelect {
        candidates: Vec<ActorId>,
        index: usize,
    },
    /// The key list, overlaid on the map. Any key dismisses it.
    Help,
    /// A yes/no question guarding something that cannot be undone.
    Confirm {
        prompt: &'static str,
        on_yes: ConfirmAction,
    },
}

/// What a confirmed prompt actually does. Kept separate from the prompt
/// text so the shell can act on the answer without re-parsing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    /// Walk out of the mission, forfeiting the contract.
    AbandonRun,
}

/// How loudly a log line should read. Most events are routine; the ones
/// that change your situation should not look the same as footsteps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogKind {
    /// Ambient chatter and confirmations.
    #[default]
    Routine,
    /// Something worth reading: a refusal, a plan cancelled, a discovery.
    Notice,
    /// Something has gone wrong: detection, injury, a broken contract.
    Alarm,
}

/// One line in the event log. Identical consecutive messages collapse into
/// a single entry with a count rather than scrolling the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub text: String,
    pub kind: LogKind,
    pub count: u16,
}

impl LogEntry {
    /// The line as rendered, including the repeat marker.
    pub fn display(&self) -> String {
        if self.count > 1 {
            murmur_core::loc::fmt(
                "ui.mission.log.repeat",
                &[("text", &self.text), ("count", &self.count.to_string())],
            )
        } else {
            self.text.clone()
        }
    }
}

/// Presentation pacing: how many due simulation turns run per frame.
/// All settings produce identical simulation results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Slow,
    Normal,
    Fast,
    Instant,
}

impl Speed {
    pub fn label(self) -> &'static str {
        match self {
            Speed::Slow => tr!("speed.slow"),
            Speed::Normal => tr!("speed.normal"),
            Speed::Fast => tr!("speed.fast"),
            Speed::Instant => tr!("speed.instant"),
        }
    }

    fn turns_due(self, frame: u64, batch_turns: u16) -> u16 {
        match self {
            Speed::Slow => u16::from(frame.is_multiple_of(10)),
            Speed::Normal => u16::from(frame.is_multiple_of(4)),
            Speed::Fast => 1,
            Speed::Instant => batch_turns.max(1),
        }
    }

    fn slower(self) -> Speed {
        match self {
            Speed::Instant => Speed::Fast,
            Speed::Fast => Speed::Normal,
            _ => Speed::Slow,
        }
    }

    fn faster(self) -> Speed {
        match self {
            Speed::Slow => Speed::Normal,
            Speed::Normal => Speed::Fast,
            _ => Speed::Instant,
        }
    }
}

pub use crate::hitmap::UiLayout;

/// The in-mission controller. Its public face is deliberately narrow:
/// input in ([`Mission::handle_input`]), frames out ([`Mission::draw`]),
/// time via [`Mission::tick`] — plus read-only accessors naming exactly
/// what a frame or a test needs. The representation is private; the
/// renderer and the tests cannot reach past the surface.
pub struct Mission {
    driver: TurnDriver,
    queue: CommandQueue,
    mode: InputMode,
    speed: Speed,
    log: Vec<LogEntry>,
    /// The map tile under the mouse cursor, for hover inspection.
    hover: Option<Pos>,
    /// Last frame's layout, for mouse hit-testing.
    ui: UiLayout,
    /// A confirmed prompt waiting for the shell to act on it.
    confirmed: Option<ConfirmAction>,
    /// The inventory slot the player last asked about, shown in the
    /// inspection line until they look at something else.
    inspected_slot: Option<usize>,
    explored: crate::fov::Explored,
    frame: u64,
    /// Hold-to-wait: the shell submits Wait turns on the player's behalf
    /// until something worth reacting to happens or any key is pressed.
    fast_forward: bool,
}

impl Mission {
    pub fn new(driver: TurnDriver, data: &GameData) -> Self {
        let explored = crate::fov::Explored::new(&driver.world().map);
        let mut mission = Self {
            driver,
            queue: CommandQueue::new(usize::from(data.tuning.queue_capacity)),
            mode: InputMode::Normal,
            speed: Speed::Fast,
            fast_forward: false,
            log: vec![LogEntry {
                text: tr!("mission.notice.entered").to_string(),
                kind: LogKind::Routine,
                count: 1,
            }],
            hover: None,
            ui: UiLayout::default(),
            confirmed: None,
            inspected_slot: None,
            explored,
            frame: 0,
        };
        mission.update_explored(data);
        mission
    }

    pub fn world(&self) -> &World {
        self.driver.world()
    }

    pub fn mode(&self) -> &InputMode {
        &self.mode
    }

    pub fn speed(&self) -> Speed {
        self.speed
    }

    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    pub fn queue(&self) -> &CommandQueue {
        &self.queue
    }

    pub fn ui(&self) -> &UiLayout {
        &self.ui
    }

    pub fn hover(&self) -> Option<Pos> {
        self.hover
    }

    /// Renders the mission and caches the layout for mouse hit-testing.
    pub fn draw(&mut self, frame: &mut ratatui::Frame, data: &GameData) {
        self.ui = crate::render::draw_mission(frame, data, self);
    }

    pub fn is_explored(&self, pos: Pos) -> bool {
        self.explored.contains(&self.driver.world().map, pos)
    }

    /// What the last-asked-about inventory slot holds, if any.
    pub fn inspected_slot_text(&self, data: &GameData) -> Option<String> {
        let slot = self.inspected_slot?;
        crate::inspect::slot_text(self.world(), data, slot)
    }

    /// The tile currently being inspected: the look cursor when look mode
    /// is active, otherwise the hovered tile.
    pub fn inspected_tile(&self) -> Option<Pos> {
        match &self.mode {
            InputMode::Look(cursor) => Some(*cursor),
            InputMode::ThrowTarget(cursor) => Some(*cursor),
            _ => self.hover,
        }
    }

    pub fn handle_input(&mut self, data: &GameData, input: ShellInput) {
        // Any deliberate input ends a fast-forward: the player has seen
        // something and wants the turn back. Mouse movement is not
        // deliberate in that sense.
        if self.fast_forward && !matches!(input, ShellInput::MouseMove { .. }) {
            self.fast_forward = false;
        }
        match input {
            ShellInput::MouseMove { column, row } => {
                let hover = self
                    .ui
                    .tile_at(column, row)
                    .filter(|pos| self.world().map.in_bounds(*pos));
                // Pointing at the map answers a different question than
                // the slot did, so the slot line steps aside.
                if hover.is_some() {
                    self.inspected_slot = None;
                }
                self.hover = hover;
                return;
            }
            ShellInput::MouseClick { column, row } => {
                self.handle_click(data, column, row);
                return;
            }
            _ => {}
        }
        match &self.mode {
            InputMode::Normal => self.handle_normal(data, input),
            InputMode::Pending(action) => {
                let action = *action;
                self.handle_pending(data, action, input);
            }
            InputMode::Look(cursor) => {
                let cursor = *cursor;
                self.handle_look(cursor, input);
            }
            InputMode::ThrowTarget(cursor) => {
                let cursor = *cursor;
                self.handle_throw_target(cursor, input);
            }
            InputMode::TargetSelect { candidates, index } => {
                let (candidates, index) = (candidates.clone(), *index);
                self.handle_target_select(candidates, index, input);
            }
            InputMode::Help => self.handle_help(input),
            InputMode::Confirm { on_yes, .. } => {
                let on_yes = *on_yes;
                self.handle_confirm(on_yes, input);
            }
        }
    }

    /// Help is a reading mode: any key at all closes it, and closing it
    /// resumes whatever was planned.
    fn handle_help(&mut self, _input: ShellInput) {
        self.mode = InputMode::Normal;
        self.queue.resume();
    }

    /// Answering the prompt either way leaves the mode and resumes, so a
    /// confirmation can never become a state the player is stuck in.
    /// `confirmed` is read by the shell, which owns the consequences.
    fn handle_confirm(&mut self, on_yes: ConfirmAction, input: ShellInput) {
        let yes = matches!(input, ShellInput::Char('y') | ShellInput::Enter);
        self.mode = InputMode::Normal;
        self.queue.resume();
        if yes {
            self.confirmed = Some(on_yes);
        }
    }

    /// Takes the answer to a confirmed prompt, if one is waiting. The
    /// shell polls this because abandoning a run is its decision to carry
    /// out, not the mission's.
    pub fn take_confirmed(&mut self) -> Option<ConfirmAction> {
        self.confirmed.take()
    }

    /// Opens a yes/no prompt, pausing execution while it is up.
    fn ask(&mut self, prompt: &'static str, on_yes: ConfirmAction) {
        self.queue.pause();
        self.mode = InputMode::Confirm { prompt, on_yes };
    }

    /// Clicking an action in the palette is exactly pressing its key;
    /// clicking the map completes pending targeting, aims shots, or moves
    /// the look cursor.
    fn handle_click(&mut self, data: &GameData, column: u16, row: u16) {
        // Clicking a recorded row is exactly the input it carries — the
        // same resolution every interface screen uses.
        if let Some(input) = self.ui.rows.input_at(column, row) {
            self.handle_input(data, input);
            return;
        }
        let Some(pos) = self
            .ui
            .tile_at(column, row)
            .filter(|pos| self.world().map.in_bounds(*pos))
        else {
            return;
        };
        match &self.mode {
            InputMode::Pending(action) => {
                let action = *action;
                let player_pos = self.world().player_actor().pos;
                if pos == player_pos || pos.is_adjacent(player_pos) {
                    self.mode = InputMode::Normal;
                    match self.command_for_target(action, pos) {
                        Some(command) => self.enqueue(command),
                        None => self.push_log_kind(
                            tr!("mission.notice.nothing_there").to_string(),
                            LogKind::Notice,
                        ),
                    }
                }
            }
            InputMode::TargetSelect { candidates, .. } => {
                let clicked = candidates
                    .iter()
                    .copied()
                    .find(|id| self.world().actor(*id).pos == pos);
                if let Some(target) = clicked {
                    self.mode = InputMode::Normal;
                    self.enqueue(Command::Shoot(target));
                }
            }
            InputMode::Look(_) => {
                self.mode = InputMode::Look(pos);
            }
            // The overlay covers the map, so a click on it is a click on
            // the overlay: dismiss, exactly as any key would.
            InputMode::Help => self.handle_help(ShellInput::Esc),
            // A click is plainly not a "yes", so it answers no. Both
            // answers leave the mode, so the prompt is never a trap.
            InputMode::Confirm { on_yes, .. } => {
                let on_yes = *on_yes;
                self.handle_confirm(on_yes, ShellInput::Esc);
            }
            InputMode::ThrowTarget(_) => {
                self.mode = InputMode::Normal;
                self.enqueue(Command::ThrowNoisemaker(pos));
            }
            // Click-to-move: one step along the shortest path to any tile
            // you have already seen. Clicking again walks the next step,
            // so movement stays one intention per turn like the keys.
            InputMode::Normal => {
                if !self.is_explored(pos) {
                    return;
                }
                let world = self.world();
                if world.player_actor().pos == pos {
                    return;
                }
                match first_step_towards(world, data, world.player, pos) {
                    Some(dir) => self.enqueue(Command::Move(dir)),
                    None => self
                        .push_log_kind(tr!("mission.notice.no_path").to_string(), LogKind::Notice),
                }
            }
        }
    }

    /// Whether a palette action currently has a valid potential target
    /// or context. Non-targeted actions (move, wait, crouch, look, speed)
    /// are always available; targeted ones grey out and, if pressed,
    /// report why rather than entering a dead targeting mode.
    pub fn action_available(&self, data: &GameData, key: char) -> bool {
        crate::availability::action_block(self.world(), data, key).is_none()
    }

    fn handle_normal(&mut self, data: &GameData, input: ShellInput) {
        // Targeted actions with no valid target report why instead of
        // entering a dead targeting mode.
        if let ShellInput::Char(key) = input
            && let Some(reason) = crate::availability::action_block(self.world(), data, key)
        {
            self.push_log_kind(
                trf!("mission.notice.blocked", reason = reason),
                LogKind::Notice,
            );
            return;
        }
        match input {
            ShellInput::Up => self.enqueue(Command::Move(Dir4::North)),
            ShellInput::Down => self.enqueue(Command::Move(Dir4::South)),
            ShellInput::Left => self.enqueue(Command::Move(Dir4::West)),
            ShellInput::Right => self.enqueue(Command::Move(Dir4::East)),
            ShellInput::Char('.') | ShellInput::Char(' ') => self.enqueue(Command::Wait),
            ShellInput::Char('z') => {
                self.fast_forward = true;
                self.push_log_kind(
                    tr!("mission.notice.fast_forward").to_string(),
                    LogKind::Notice,
                );
            }
            ShellInput::Char('c') => self.enqueue(Command::ToggleCrouch),
            ShellInput::Char('r') => self.enqueue(Command::DrawOrHolster),
            ShellInput::Char('g') => self.mode = InputMode::Pending(PendingAction::Garrote),
            ShellInput::Char('p') => self.mode = InputMode::Pending(PendingAction::Pickpocket),
            ShellInput::Char('d') => self.mode = InputMode::Pending(PendingAction::Disguise),
            ShellInput::Char('h') => self.mode = InputMode::Pending(PendingAction::HideBody),
            ShellInput::Char('o') => self.mode = InputMode::Pending(PendingAction::OpenDoor),
            ShellInput::Char('k') => self.mode = InputMode::Pending(PendingAction::CloseDoor),
            ShellInput::Char('l') => self.mode = InputMode::Pending(PendingAction::PickLock),
            ShellInput::Char('u') => self.mode = InputMode::Pending(PendingAction::UseMachine),
            ShellInput::Char('t') => {
                self.mode = InputMode::ThrowTarget(self.world().player_actor().pos);
            }
            ShellInput::Char('b') => {
                if matches!(self.world().player_actor().hands, Hands::CarryingBody(_)) {
                    self.mode = InputMode::Pending(PendingAction::DropBody);
                } else {
                    self.mode = InputMode::Pending(PendingAction::CarryBody);
                }
            }
            ShellInput::Char('f') => {
                let candidates = crate::fov::visible_actors(self.world(), data);
                if candidates.is_empty() {
                    self.push_log_kind(tr!("mission.block.no_target").to_string(), LogKind::Notice);
                } else {
                    self.mode = InputMode::TargetSelect {
                        candidates,
                        index: 0,
                    };
                }
            }
            ShellInput::Char(';') => {
                // Look mode pauses execution internally; leaving it
                // resumes, so the pause is never a stuck state.
                self.queue.pause();
                self.inspected_slot = None;
                self.mode = InputMode::Look(self.world().player_actor().pos);
            }
            ShellInput::Char('?') => {
                // Same contract as look: pause to read, resume on exit.
                self.queue.pause();
                self.mode = InputMode::Help;
            }
            ShellInput::Char('Q') => {
                self.ask(tr!("mission.confirm.abandon"), ConfirmAction::AbandonRun);
            }
            // Reading a slot costs no time and produces no action: it is
            // inspection, like look. Carrying an item is what enables its
            // verb, so there is nothing to "use" here.
            ShellInput::Char(slot @ '1'..='6') => {
                self.inspected_slot = Some(usize::from(slot as u8 - b'1'));
            }
            ShellInput::Char('[') => {
                self.speed = self.speed.slower();
                self.push_log(trf!("mission.notice.speed", speed = self.speed.label()));
            }
            ShellInput::Char(']') => {
                self.speed = self.speed.faster();
                self.push_log(trf!("mission.notice.speed", speed = self.speed.label()));
            }
            ShellInput::Backspace => {
                self.queue.remove_newest();
            }
            ShellInput::Esc => {
                let cancelled = self.queue.clear();
                if cancelled > 0 {
                    self.push_log_kind(tr!("mission.notice.stopped").to_string(), LogKind::Notice);
                }
            }
            _ => {}
        }
    }

    fn handle_pending(&mut self, data: &GameData, action: PendingAction, input: ShellInput) {
        let dir = match input {
            ShellInput::Up => Some(Dir4::North),
            ShellInput::Down => Some(Dir4::South),
            ShellInput::Left => Some(Dir4::West),
            ShellInput::Right => Some(Dir4::East),
            // 'b' twice targets the player's own tile.
            ShellInput::Char('b')
                if action == PendingAction::CarryBody || action == PendingAction::DropBody =>
            {
                None
            }
            ShellInput::Esc => {
                self.mode = InputMode::Normal;
                return;
            }
            _ => {
                self.mode = InputMode::Normal;
                return;
            }
        };
        let _ = data;
        self.mode = InputMode::Normal;
        let player_pos = self.world().player_actor().pos;

        // DropBody needs the raw option rather than a resolved target pos.
        if action == PendingAction::DropBody {
            self.enqueue(Command::DropBody(dir));
            return;
        }

        let target_pos = match dir {
            Some(dir) => player_pos.step(dir),
            None => player_pos,
        };
        match self.command_for_target(action, target_pos) {
            Some(command) => self.enqueue(command),
            None => self.push_log_kind(
                tr!("mission.notice.nothing_there").to_string(),
                LogKind::Notice,
            ),
        }
    }

    /// Captures the stable domain ID for an action target at enqueue
    /// time; validation happens only when the command executes.
    fn command_for_target(&self, action: PendingAction, pos: Pos) -> Option<Command> {
        let world = self.world();
        match action {
            PendingAction::Garrote => world.standing_actor_at(pos).map(|a| Command::Garrote(a.id)),
            PendingAction::Pickpocket => world
                .standing_actor_at(pos)
                .map(|a| Command::Pickpocket(a.id))
                .or_else(|| world.body_at(pos).map(|a| Command::Pickpocket(a.id))),
            PendingAction::Disguise => world
                .body_at(pos)
                .map(|a| Command::TakeDisguiseFromBody(a.id))
                .or_else(|| {
                    world.furniture_at(pos).and_then(|f| {
                        (f.kind == FurnitureKind::Wardrobe)
                            .then_some(Command::TakeDisguiseFromWardrobe(f.id))
                    })
                }),
            PendingAction::CarryBody => world.body_at(pos).map(|a| Command::CarryBody(a.id)),
            PendingAction::HideBody => world.furniture_at(pos).and_then(|f| {
                (f.kind == FurnitureKind::Container).then_some(Command::HideBody(f.id))
            }),
            PendingAction::OpenDoor => match world.map.tile(pos) {
                TileKind::Door(id) => Some(Command::OpenDoor(id)),
                _ => None,
            },
            PendingAction::CloseDoor => match world.map.tile(pos) {
                TileKind::Door(id) => Some(Command::CloseDoor(id)),
                _ => None,
            },
            PendingAction::PickLock => match world.map.tile(pos) {
                TileKind::Door(id) => Some(Command::PickLock(id)),
                _ => None,
            },
            PendingAction::UseMachine => world.furniture_at(pos).and_then(|f| {
                (f.kind == FurnitureKind::Machine).then_some(Command::Interact(f.id))
            }),
            PendingAction::DropBody => {
                // Handled directly in handle_pending; unreachable here.
                None
            }
        }
    }

    fn handle_look(&mut self, cursor: Pos, input: ShellInput) {
        match input {
            ShellInput::Up | ShellInput::Down | ShellInput::Left | ShellInput::Right => {
                let dir = match input {
                    ShellInput::Up => Dir4::North,
                    ShellInput::Down => Dir4::South,
                    ShellInput::Left => Dir4::West,
                    _ => Dir4::East,
                };
                let next = cursor.step(dir);
                if self.world().map.in_bounds(next) {
                    self.mode = InputMode::Look(next);
                }
            }
            ShellInput::Char(';') | ShellInput::Esc => {
                // Exiting look mode resumes execution immediately.
                self.mode = InputMode::Normal;
                self.queue.resume();
            }
            _ => {}
        }
    }

    fn handle_throw_target(&mut self, cursor: Pos, input: ShellInput) {
        match input {
            ShellInput::Up | ShellInput::Down | ShellInput::Left | ShellInput::Right => {
                let dir = match input {
                    ShellInput::Up => Dir4::North,
                    ShellInput::Down => Dir4::South,
                    ShellInput::Left => Dir4::West,
                    _ => Dir4::East,
                };
                let next = cursor.step(dir);
                if self.world().map.in_bounds(next) {
                    self.mode = InputMode::ThrowTarget(next);
                }
            }
            ShellInput::Enter => {
                self.mode = InputMode::Normal;
                self.enqueue(Command::ThrowNoisemaker(cursor));
            }
            ShellInput::Char('t') | ShellInput::Esc => self.mode = InputMode::Normal,
            _ => {}
        }
    }

    fn handle_target_select(&mut self, candidates: Vec<ActorId>, index: usize, input: ShellInput) {
        match input {
            ShellInput::Up | ShellInput::Left => {
                let index = (index + candidates.len() - 1) % candidates.len();
                self.mode = InputMode::TargetSelect { candidates, index };
            }
            ShellInput::Down | ShellInput::Right | ShellInput::Char('f') => {
                let index = (index + 1) % candidates.len();
                self.mode = InputMode::TargetSelect { candidates, index };
            }
            ShellInput::Enter => {
                let target = candidates[index];
                self.mode = InputMode::Normal;
                self.enqueue(Command::Shoot(target));
            }
            ShellInput::Esc => self.mode = InputMode::Normal,
            _ => {}
        }
    }

    fn enqueue(&mut self, command: Command) {
        match self.queue.push(command) {
            EnqueueOutcome::Accepted => {}
            EnqueueOutcome::RejectedFull => {
                self.push_log_kind(
                    tr!("mission.notice.queue_full").to_string(),
                    LogKind::Notice,
                );
            }
        }
    }

    /// One presentation frame: run every due simulation turn for the
    /// current speed, then refresh exploration memory.
    pub fn tick(&mut self, data: &GameData) {
        self.frame += 1;
        self.fast_forward_turns(data);
        let turns = self.speed.turns_due(self.frame, data.tuning.batch_turns);
        for _ in 0..turns {
            if !self.step_one(data) {
                break;
            }
        }
        self.update_explored(data);
    }

    /// Runs at most one simulation turn. Returns false when no turn was
    /// due (idle, look mode, or mission over).
    /// Runs held-down waiting: submit Wait turns until something worth
    /// reacting to happens. The commands go through the ordinary driver,
    /// so they land in `accepted_commands` and replay is untouched — this
    /// is purely presentation deciding how fast to press the wait key.
    fn fast_forward_turns(&mut self, data: &GameData) {
        if !self.fast_forward {
            return;
        }
        let target_state = |driver: &TurnDriver| {
            let world = driver.world();
            let target = world.actor(world.target);
            (
                target.alive(),
                target
                    .ai
                    .as_ref()
                    .and_then(|ai| ai.schedule.as_ref())
                    .and_then(|s| s.current())
                    .map(|b| b.protection),
            )
        };
        // A bounded burst per frame keeps the interface responsive while
        // still skipping dead time quickly.
        for _ in 0..data.tuning.batch_turns.max(1) {
            if self.driver.mission_over() || self.queue.is_paused() || !self.queue.is_empty() {
                self.fast_forward = false;
                return;
            }
            if self.driver.player_busy() {
                let report = self.driver.continue_busy(data);
                self.absorb(report);
                continue;
            }
            let hp_before = self.world().player_actor().hp;
            let before = target_state(&self.driver);
            let log_before = self.log.len();
            match self.driver.submit(data, &Command::Wait) {
                Ok(report) => self.absorb(report),
                Err(_) => {
                    self.fast_forward = false;
                    return;
                }
            }
            // Interrupt on anything the player would want to react to: a
            // message, the mission ending, taking damage, or the target's
            // protection changing — the last is the window opening.
            if self.log.len() != log_before
                || self.driver.mission_over()
                || self.world().player_actor().hp != hp_before
                || target_state(&self.driver) != before
            {
                self.fast_forward = false;
                return;
            }
        }
    }

    fn step_one(&mut self, data: &GameData) -> bool {
        if self.driver.mission_over() {
            return false;
        }
        if self.driver.player_busy() {
            let report = self.driver.continue_busy(data);
            self.absorb(report);
            return true;
        }
        if self.queue.is_paused() {
            return false;
        }
        let Some(command) = self.queue.head().copied() else {
            return false;
        };
        match self.driver.submit(data, &command) {
            Ok(report) => {
                self.queue.pop_head();
                self.absorb(report);
                true
            }
            Err(reason) => {
                // Pre-turn rejection: pure, no time passed. Remove the
                // offender and quietly drop whatever was planned after it.
                self.queue.pop_head();
                self.queue.clear();
                self.push_log_kind(
                    trf!("mission.notice.rejected", reason = reason.message()),
                    LogKind::Notice,
                );
                false
            }
        }
    }

    fn absorb(&mut self, report: TurnReport) {
        // A breach is the one event message that changes the run's payout,
        // so it reads as an alarm. The state check is the real guard; the
        // prefix only picks the breach line out of that turn's messages,
        // and matches how `breach_constraint` formats it.
        for message in &report.events.messages {
            self.push_log_kind(message.clone(), LogKind::Routine);
        }
        // A breach is the one event that changes the run's payout, so it
        // reads as an alarm. It arrives in its own field rather than being
        // picked out of the message list by prefix, which stopped working
        // the moment the words became translatable.
        if let Some(breach) = &report.events.breach {
            self.push_log_kind(breach.clone(), LogKind::Alarm);
        }
        // Perception messages are alarms, screams, and their propagation:
        // by definition the lines that say your situation just changed.
        for message in &report.perception {
            self.push_log_kind(message.clone(), LogKind::Alarm);
        }
        if let Some(ActionResult::Failed(_)) = &report.events.player_result {
            // In-turn failure: the turn passed; the rest of the plan is
            // abandoned (the failure itself was already logged).
            self.queue.clear();
        }
    }

    fn push_log(&mut self, message: String) {
        self.push_log_kind(message, LogKind::Routine);
    }

    /// Appends a log line, or bumps the repeat count when it is identical
    /// to the line already at the tail. A patrol passing five times reads
    /// as one line with a count instead of five lines that push everything
    /// else off an eight-row panel.
    fn push_log_kind(&mut self, message: String, kind: LogKind) {
        if let Some(last) = self.log.last_mut()
            && last.text == message
            && last.kind == kind
        {
            last.count = last.count.saturating_add(1);
            return;
        }
        self.log.push(LogEntry {
            text: message,
            kind,
            count: 1,
        });
        if self.log.len() > 200 {
            let excess = self.log.len() - 200;
            self.log.drain(0..excess);
        }
    }

    fn update_explored(&mut self, data: &GameData) {
        self.explored.extend_visible(self.driver.world(), data);
    }

    /// A one-line description of an inspected tile, honest about what the
    /// player can currently see.
    pub fn describe(&self, data: &GameData, pos: Pos, visible: bool) -> String {
        crate::inspect::describe(self.world(), data, pos, visible, self.is_explored(pos))
    }
}

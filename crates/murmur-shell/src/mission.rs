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

use std::collections::HashSet;

use murmur_core::actions::{ActionResult, Command};
use murmur_core::data::GameData;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::map::{TileKind, line_of_sight, tiles_visible_from};
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

/// Where the last frame put things, for mouse hit-testing. Rebuilt on
/// every draw.
#[derive(Clone, Debug, Default)]
pub struct UiLayout {
    /// Interior of the map viewport in terminal cells (borders excluded).
    pub map_x: u16,
    pub map_y: u16,
    pub map_w: u16,
    pub map_h: u16,
    /// The map tile rendered at the viewport's top-left interior cell.
    pub origin: Option<Pos>,
    /// Clickable action items: (row, first column, last column, key).
    pub actions: Vec<(u16, u16, u16, char)>,
}

impl UiLayout {
    /// The map tile under a terminal cell, if any.
    pub fn tile_at(&self, column: u16, row: u16) -> Option<Pos> {
        let origin = self.origin?;
        if column < self.map_x
            || row < self.map_y
            || column >= self.map_x + self.map_w
            || row >= self.map_y + self.map_h
        {
            return None;
        }
        Some(Pos::new(
            origin.floor,
            origin.x + (column - self.map_x) as i16,
            origin.y + (row - self.map_y) as i16,
        ))
    }

    /// The action key under a terminal cell, if any.
    pub fn action_at(&self, column: u16, row: u16) -> Option<char> {
        self.actions
            .iter()
            .find(|(r, x0, x1, _)| *r == row && column >= *x0 && column <= *x1)
            .map(|(_, _, _, key)| *key)
    }
}

pub struct Mission {
    pub driver: TurnDriver,
    pub queue: CommandQueue,
    pub mode: InputMode,
    pub speed: Speed,
    pub log: Vec<LogEntry>,
    /// The map tile under the mouse cursor, for hover inspection.
    pub hover: Option<Pos>,
    /// Last frame's layout, for mouse hit-testing.
    pub ui: UiLayout,
    /// A confirmed prompt waiting for the shell to act on it.
    confirmed: Option<ConfirmAction>,
    /// The inventory slot the player last asked about, shown in the
    /// inspection line until they look at something else.
    inspected_slot: Option<usize>,
    explored: Vec<Vec<bool>>,
    frame: u64,
}

impl Mission {
    pub fn new(driver: TurnDriver, data: &GameData) -> Self {
        let world = driver.world();
        let floor_len = usize::from(world.map.width()) * usize::from(world.map.height());
        let explored = (0..world.map.floor_count())
            .map(|_| vec![false; floor_len])
            .collect();
        let mut mission = Self {
            driver,
            queue: CommandQueue::new(usize::from(data.tuning.queue_capacity)),
            mode: InputMode::Normal,
            speed: Speed::Fast,
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

    /// Renders the mission and caches the layout for mouse hit-testing.
    pub fn draw(&mut self, frame: &mut ratatui::Frame, data: &GameData) {
        self.ui = crate::render::draw_mission(frame, data, self);
    }

    pub fn is_explored(&self, pos: Pos) -> bool {
        let world = self.driver.world();
        if !world.map.in_bounds(pos) {
            return false;
        }
        let index = usize::try_from(pos.y).unwrap() * usize::from(world.map.width())
            + usize::try_from(pos.x).unwrap();
        self.explored[usize::from(pos.floor)][index]
    }

    /// The player's current field of view, widened so that every
    /// sight-blocking tile bordering a visible open tile is lit too:
    /// standing against a wall shows the whole wall face, matching how a
    /// person actually reads a room.
    pub fn visible_tiles(&self, data: &GameData) -> Vec<Pos> {
        let world = self.driver.world();
        let player = world.player_actor();
        let base = tiles_visible_from(
            player.pos,
            data.tuning.player_vision_range,
            &world.map,
            world.sight_blocker(player.crouched),
        );
        let mut lit: HashSet<Pos> = base.iter().copied().collect();
        // Classify blockers without crouch effects: walls, closed doors,
        // and tall furniture, not low cover.
        let blocking = world.sight_blocker(false);
        for pos in &base {
            if blocking(*pos) {
                continue;
            }
            for dy in -1i16..=1 {
                for dx in -1i16..=1 {
                    let neighbour = Pos::new(pos.floor, pos.x + dx, pos.y + dy);
                    if world.map.in_bounds(neighbour) && blocking(neighbour) {
                        lit.insert(neighbour);
                    }
                }
            }
        }
        lit.into_iter().collect()
    }

    /// Living NPCs the player can currently see, nearest first.
    pub fn visible_actors(&self, data: &GameData) -> Vec<ActorId> {
        let world = self.driver.world();
        let player = world.player_actor();
        let mut ids: Vec<(i16, ActorId)> = world
            .actors
            .iter()
            .filter(|a| !a.is_player() && a.alive() && !a.departed && a.hidden_in.is_none())
            .filter(|a| {
                player
                    .pos
                    .chebyshev(a.pos)
                    .is_some_and(|d| d <= data.tuning.player_vision_range)
                    && line_of_sight(
                        player.pos,
                        a.pos,
                        world.sight_blocker(player.crouched || a.crouched),
                    )
            })
            .map(|a| (player.pos.chebyshev(a.pos).unwrap_or(i16::MAX), a.id))
            .collect();
        ids.sort();
        ids.into_iter().map(|(_, id)| id).collect()
    }

    /// What the last-asked-about inventory slot holds, if any.
    ///
    /// Items are passive: carrying one is what enables its verb, so the
    /// useful thing to report is which key it unlocks rather than a flavour
    /// line. The key names come from the keymap table, so this description
    /// follows any rebinding automatically.
    pub fn inspected_slot_text(&self, data: &GameData) -> Option<String> {
        let slot = self.inspected_slot?;
        let world = self.world();
        let Some(item) = world.carried_items(world.player).nth(slot) else {
            return Some(trf!("ui.mission.slot_line.empty", n = slot + 1));
        };
        let Some(spec) = data.item(&item.spec) else {
            return Some(murmur_core::loc::fmt(
                "ui.mission.slot_line.unknown",
                &[("n", &(slot + 1).to_string()), ("id", &item.spec)],
            ));
        };
        let mut notes: Vec<String> = Vec::new();
        let mut enables = |key: char| {
            if let Some(action) = crate::keymap::action(key) {
                notes.push(murmur_core::loc::fmt(
                    "ui.mission.item.enables",
                    &[("action", action.label()), ("key", &action.key.to_string())],
                ));
            }
        };
        if spec.firearm {
            enables('f');
        } else if spec.weapon {
            enables('g');
        }
        if spec.lockpick {
            enables('l');
        }
        if spec.noisemaker {
            enables('t');
        }
        if spec.invitation {
            notes.push(tr!("ui.mission.item.invitation").to_string());
        }
        if spec.staff_pass {
            notes.push(tr!("ui.mission.item.staff_pass").to_string());
        }
        if spec.master_key {
            notes.push(tr!("ui.mission.item.master_key").to_string());
        } else if spec.unlocks.is_some() {
            notes.push(tr!("ui.mission.item.one_lock").to_string());
        }
        if item.charges > 0 {
            notes.push(trf!("ui.mission.item.charges", count = item.charges));
        }
        let detail = if notes.is_empty() {
            tr!("ui.mission.item.no_use").to_string()
        } else {
            notes.join(", ")
        };
        Some(murmur_core::loc::fmt(
            "ui.mission.slot_line",
            &[
                ("n", &(slot + 1).to_string()),
                ("name", &spec.name),
                ("detail", &detail),
            ],
        ))
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
        if let Some(key) = self.ui.action_at(column, row) {
            self.handle_input(data, ShellInput::Char(key));
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
        self.action_block(data, key).is_none()
    }

    /// The reason a targeted action cannot be attempted right now, or
    /// `None` when it is available (or is a non-targeted action).
    fn action_block(&self, data: &GameData, key: char) -> Option<&'static str> {
        let world = self.world();
        let player = world.player_actor();
        let here = player.pos;
        // Own tile plus the four orthogonal neighbours — the reach of
        // every adjacency-based command.
        let near = |probe: &dyn Fn(Pos) -> bool| {
            probe(here) || Dir4::ALL.into_iter().any(|d| probe(here.step(d)))
        };
        let carries = |pred: &dyn Fn(&murmur_core::data::ItemSpec) -> bool| {
            world
                .carried_items(world.player)
                .any(|i| data.item(&i.spec).is_some_and(pred))
        };
        let living_npc = |p: Pos| {
            world
                .standing_actor_at(p)
                .is_some_and(|a| !a.is_player() && a.alive())
        };

        match key {
            'r' => (!carries(&|s| s.firearm)).then_some(tr!("mission.block.no_firearm_owned")),
            'g' => {
                if !carries(&|s| s.weapon && !s.firearm) {
                    Some(tr!("mission.block.no_garrote"))
                } else if !near(&living_npc) {
                    Some(tr!("mission.block.no_garrote_target"))
                } else {
                    None
                }
            }
            'f' => {
                if !carries(&|s| s.firearm) {
                    Some(tr!("mission.block.no_firearm"))
                } else if self.visible_actors(data).is_empty() {
                    Some(tr!("mission.block.no_target"))
                } else {
                    None
                }
            }
            'p' => {
                if world.carried_items(world.player).count()
                    >= murmur_core::actions::INVENTORY_SLOTS
                {
                    Some(tr!("mission.block.pockets_full"))
                } else if !near(&|p| {
                    world.standing_actor_at(p).is_some_and(|a| !a.is_player())
                        || world.body_at(p).is_some()
                }) {
                    Some(tr!("mission.block.no_mark"))
                } else {
                    None
                }
            }
            'd' => {
                if player.hands != Hands::Free {
                    Some(tr!("mission.block.hands_busy"))
                } else if !near(&|p| {
                    world.body_at(p).is_some()
                        || world.furniture_at(p).is_some_and(|f| {
                            f.kind == FurnitureKind::Wardrobe && f.disguise.is_some()
                        })
                }) {
                    Some(tr!("mission.block.no_clothes"))
                } else {
                    None
                }
            }
            'b' => {
                if matches!(player.hands, Hands::CarryingBody(_)) {
                    None // drop is available
                } else if player.hands != Hands::Free {
                    Some(tr!("mission.block.hands_busy"))
                } else if !near(&|p| world.body_at(p).is_some()) {
                    Some(tr!("mission.block.no_body"))
                } else {
                    None
                }
            }
            'h' => {
                if !matches!(player.hands, Hands::CarryingBody(_)) {
                    Some(tr!("mission.block.not_carrying"))
                } else if !near(&|p| {
                    world
                        .furniture_at(p)
                        .is_some_and(|f| f.kind == FurnitureKind::Container && f.body.is_none())
                }) {
                    Some(tr!("mission.block.no_container"))
                } else {
                    None
                }
            }
            'o' => (!near(
                &|p| matches!(world.map.tile(p), TileKind::Door(id) if !world.door(id).open),
            ))
            .then_some(tr!("mission.block.no_door_to_open")),
            'k' => {
                (!near(&|p| matches!(world.map.tile(p), TileKind::Door(id) if world.door(id).open)))
                    .then_some(tr!("mission.block.no_door_to_close"))
            }
            'l' => {
                if !carries(&|s| s.lockpick) {
                    Some(tr!("mission.block.no_lockpicks"))
                } else if !near(
                    &|p| matches!(world.map.tile(p), TileKind::Door(id) if world.door(id).locked_by.is_some()),
                ) {
                    Some(tr!("mission.block.no_lock"))
                } else {
                    None
                }
            }
            't' => {
                let ready = world
                    .carried_items(world.player)
                    .any(|i| data.item(&i.spec).is_some_and(|s| s.noisemaker) && i.charges > 0);
                (!ready).then_some(tr!("mission.block.no_charges"))
            }
            'u' => (!near(&|p| {
                world
                    .furniture_at(p)
                    .is_some_and(|f| f.kind == FurnitureKind::Machine && !f.used)
            }))
            .then_some(tr!("mission.block.nothing_to_use")),
            _ => None,
        }
    }

    fn handle_normal(&mut self, data: &GameData, input: ShellInput) {
        // Targeted actions with no valid target report why instead of
        // entering a dead targeting mode.
        if let ShellInput::Char(key) = input
            && let Some(reason) = self.action_block(data, key)
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
                let candidates = self.visible_actors(data);
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
        let width = usize::from(self.driver.world().map.width());
        for pos in self.visible_tiles(data) {
            let index = usize::try_from(pos.y).unwrap() * width + usize::try_from(pos.x).unwrap();
            self.explored[usize::from(pos.floor)][index] = true;
        }
    }

    /// A one-line description of an inspected tile, honest about what the
    /// player can currently see.
    pub fn describe(&self, data: &GameData, pos: Pos, visible: bool) -> String {
        let world = self.world();
        if !visible && !self.is_explored(pos) {
            return tr!("ui.mission.tile.unseen").to_string();
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(room) = world.room_at(pos) {
            let zone_label = data
                .venue(&world.venue)
                .map(|v| v.zone_label(room.zone))
                .unwrap_or_else(|| room.zone.name());
            parts.push(murmur_core::loc::fmt(
                "ui.mission.tile.room",
                &[("room", &room.name), ("zone", zone_label)],
            ));
        } else if matches!(world.map.tile(pos), TileKind::Floor | TileKind::Stairs) {
            parts.push(tr!("ui.mission.tile.corridor").to_string());
        }
        match world.map.tile(pos) {
            TileKind::Wall => parts.push(tr!("ui.mission.tile.wall").to_string()),
            TileKind::Stairs => parts.push(tr!("ui.mission.tile.stairs").to_string()),
            TileKind::Door(id) => {
                let door = world.door(id);
                let state = if door.open {
                    tr!("ui.mission.tile.door.open")
                } else {
                    tr!("ui.mission.tile.door.closed")
                };
                if door.locked_by.is_some() {
                    parts.push(trf!("ui.mission.tile.door.locked", state = state));
                } else {
                    parts.push(state.to_string());
                }
            }
            _ => {}
        }
        if world.extraction_tiles.contains(&pos) {
            parts.push(tr!("ui.mission.tile.exit").to_string());
        }
        if visible {
            if let Some(actor) = world.standing_actor_at(pos) {
                let role = actor
                    .role
                    .map(|r| r.name().to_string())
                    .unwrap_or_else(|| tr!("ui.mission.tile.you").to_string());
                let mood = actor
                    .ai
                    .as_ref()
                    .map(|ai| ai.mood.label())
                    .unwrap_or_default();
                if actor.is_target {
                    parts.push(murmur_core::loc::fmt(
                        "ui.mission.tile.target",
                        &[("name", &actor.name), ("role", &role), ("mood", mood)],
                    ));
                } else if actor.is_player() {
                    parts.push(tr!("ui.mission.tile.you").to_string());
                } else {
                    parts.push(murmur_core::loc::fmt(
                        "ui.mission.tile.actor",
                        &[("name", &actor.name), ("role", &role), ("mood", mood)],
                    ));
                }
            }
            if let Some(body) = world.body_at(pos) {
                parts.push(trf!("ui.mission.tile.body", name = body.name));
            }
            for item in world.items_at(pos) {
                if let Some(spec) = data.item(&item.spec) {
                    parts.push(spec.name.clone());
                }
            }
            if let Some(furniture) = world.furniture_at(pos) {
                let described = match furniture.kind {
                    FurnitureKind::LowCover => tr!("ui.mission.tile.low_cover").to_string(),
                    FurnitureKind::Container => {
                        if furniture.body.is_some() {
                            tr!("ui.mission.tile.container_full").to_string()
                        } else {
                            tr!("ui.mission.tile.container").to_string()
                        }
                    }
                    FurnitureKind::Wardrobe => match &furniture.disguise {
                        Some(d) => trf!(
                            "ui.mission.tile.wardrobe",
                            disguise = data.disguise(d).map(|s| s.name.as_str()).unwrap_or(d)
                        ),
                        None => tr!("ui.mission.tile.wardrobe_empty").to_string(),
                    },
                    FurnitureKind::Machine => {
                        let spec = furniture
                            .machine
                            .as_deref()
                            .and_then(|id| data.opportunity(id));
                        match spec {
                            Some(spec) if furniture.used => {
                                trf!("ui.mission.tile.machine_spent", name = spec.name)
                            }
                            Some(spec) => murmur_core::loc::fmt(
                                "ui.mission.tile.machine",
                                &[
                                    ("name", &spec.name),
                                    ("presentation", &spec.presentation),
                                    ("risk", &spec.risk),
                                ],
                            ),
                            None => tr!("ui.mission.tile.machinery").to_string(),
                        }
                    }
                };
                parts.push(described);
            }
        }
        if parts.is_empty() {
            tr!("ui.mission.tile.nothing").to_string()
        } else {
            parts.join(", ")
        }
    }
}

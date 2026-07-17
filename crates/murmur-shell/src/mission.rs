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
use murmur_core::turn::{TurnDriver, TurnReport};
use murmur_core::world::{ActorId, FurnitureKind, Hands, World};

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
}

impl PendingAction {
    pub fn prompt(self) -> &'static str {
        match self {
            PendingAction::Garrote => "garrote - which direction?",
            PendingAction::Pickpocket => "pickpocket - which direction?",
            PendingAction::Disguise => "take disguise - which direction?",
            PendingAction::CarryBody => "carry body - which direction? (b again: here)",
            PendingAction::HideBody => "hide body - which direction?",
            PendingAction::DropBody => "drop body - which direction?",
            PendingAction::OpenDoor => "open - which direction?",
            PendingAction::CloseDoor => "close - which direction?",
            PendingAction::PickLock => "pick lock - which direction?",
        }
    }
}

/// The mission input mode.
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
            Speed::Slow => "slow",
            Speed::Normal => "normal",
            Speed::Fast => "fast",
            Speed::Instant => "instant",
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
    pub log: Vec<String>,
    /// The map tile under the mouse cursor, for hover inspection.
    pub hover: Option<Pos>,
    /// Last frame's layout, for mouse hit-testing.
    pub ui: UiLayout,
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
            log: vec!["you slip in through the entrance".to_string()],
            hover: None,
            ui: UiLayout::default(),
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
                self.hover = self
                    .ui
                    .tile_at(column, row)
                    .filter(|pos| self.world().map.in_bounds(*pos));
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
        }
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
                        None => self.push_log("nothing suitable there".to_string()),
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
            InputMode::ThrowTarget(_) => {
                self.mode = InputMode::Normal;
                self.enqueue(Command::ThrowNoisemaker(pos));
            }
            InputMode::Normal => {}
        }
    }

    fn handle_normal(&mut self, data: &GameData, input: ShellInput) {
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
                    self.push_log("no target in sight".to_string());
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
                self.mode = InputMode::Look(self.world().player_actor().pos);
            }
            ShellInput::Char('[') => {
                self.speed = self.speed.slower();
                self.push_log(format!("speed: {}", self.speed.label()));
            }
            ShellInput::Char(']') => {
                self.speed = self.speed.faster();
                self.push_log(format!("speed: {}", self.speed.label()));
            }
            ShellInput::Backspace => {
                self.queue.remove_newest();
            }
            ShellInput::Esc => {
                let cancelled = self.queue.clear();
                if cancelled > 0 {
                    self.push_log("you stop what you were doing".to_string());
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
            None => self.push_log("nothing suitable there".to_string()),
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
                self.push_log("you can't plan any further ahead".to_string());
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
                self.push_log(format!("rejected: {}", reason.message()));
                false
            }
        }
    }

    fn absorb(&mut self, report: TurnReport) {
        for message in &report.events.messages {
            self.push_log(message.clone());
        }
        for message in &report.perception {
            self.push_log(message.clone());
        }
        if let Some(ActionResult::Failed(_)) = &report.events.player_result {
            // In-turn failure: the turn passed; the rest of the plan is
            // abandoned (the failure itself was already logged).
            self.queue.clear();
        }
    }

    fn push_log(&mut self, message: String) {
        self.log.push(message);
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
            return "unseen".to_string();
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(room) = world.room_at(pos) {
            let zone_label = data
                .venue(&world.venue)
                .map(|v| v.zone_label(room.zone))
                .unwrap_or_else(|| room.zone.name());
            parts.push(format!("{} [{}]", room.name, zone_label));
        } else if matches!(world.map.tile(pos), TileKind::Floor | TileKind::Stairs) {
            parts.push("corridor".to_string());
        }
        match world.map.tile(pos) {
            TileKind::Wall => parts.push("wall".to_string()),
            TileKind::Stairs => parts.push("stairs".to_string()),
            TileKind::Door(id) => {
                let door = world.door(id);
                let state = if door.open {
                    "open door"
                } else {
                    "closed door"
                };
                if door.locked_by.is_some() {
                    parts.push(format!("{state} (locked)"));
                } else {
                    parts.push(state.to_string());
                }
            }
            _ => {}
        }
        if world.extraction_tiles.contains(&pos) {
            parts.push("extraction exit".to_string());
        }
        if visible {
            if let Some(actor) = world.standing_actor_at(pos) {
                let role = actor
                    .role
                    .map(|r| r.name().to_string())
                    .unwrap_or_else(|| "you".to_string());
                let mood = actor
                    .ai
                    .as_ref()
                    .map(|ai| format!("{:?}", ai.mood).to_lowercase())
                    .unwrap_or_default();
                if actor.is_target {
                    parts.push(format!("{} - THE TARGET ({role}, {mood})", actor.name));
                } else if actor.is_player() {
                    parts.push("you".to_string());
                } else {
                    parts.push(format!("{} ({role}, {mood})", actor.name));
                }
            }
            if let Some(body) = world.body_at(pos) {
                parts.push(format!("the body of {}", body.name));
            }
            for item in world.items_at(pos) {
                if let Some(spec) = data.item(&item.spec) {
                    parts.push(spec.name.clone());
                }
            }
            if let Some(furniture) = world.furniture_at(pos) {
                let described = match furniture.kind {
                    FurnitureKind::LowCover => "low cover".to_string(),
                    FurnitureKind::Container => {
                        if furniture.body.is_some() {
                            "container (occupied)".to_string()
                        } else {
                            "container".to_string()
                        }
                    }
                    FurnitureKind::Wardrobe => match &furniture.disguise {
                        Some(d) => format!(
                            "wardrobe ({})",
                            data.disguise(d).map(|s| s.name.as_str()).unwrap_or(d)
                        ),
                        None => "wardrobe (empty)".to_string(),
                    },
                };
                parts.push(described);
            }
        }
        if parts.is_empty() {
            "nothing of note".to_string()
        } else {
            parts.join(", ")
        }
    }
}

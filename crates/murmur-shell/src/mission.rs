//! The in-mission controller: owns the turn driver and the command queue,
//! translates key input into queued commands, and paces cooperative
//! simulation batches. Rendering cadence never changes simulation results;
//! it only decides how many due turns run per presentation frame.

use murmur_core::actions::{ActionResult, Command, RejectReason};
use murmur_core::data::GameData;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::map::{TileKind, line_of_sight, tiles_visible_from};
use murmur_core::turn::{TurnDriver, TurnReport};
use murmur_core::world::{ActorId, FurnitureKind, Hands, World};

use crate::ShellInput;
use crate::queue::{CommandQueue, EnqueueOutcome};

/// Actions that need a follow-up direction key to pick their target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingAction {
    Garrote,
    Pickpocket,
    Disguise,
    CarryBody,
    HideBody,
    OpenDoor,
    CloseDoor,
}

impl PendingAction {
    pub fn prompt(self) -> &'static str {
        match self {
            PendingAction::Garrote => "garrote - which direction?",
            PendingAction::Pickpocket => "pickpocket - which direction?",
            PendingAction::Disguise => "take disguise - which direction?",
            PendingAction::CarryBody => "carry body - which direction? (b again: here)",
            PendingAction::HideBody => "hide body - which direction?",
            PendingAction::OpenDoor => "open - which direction?",
            PendingAction::CloseDoor => "close - which direction?",
        }
    }
}

/// The mission input mode. Look mode pauses queue consumption and keeps it
/// paused after exit until the player explicitly resumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Pending(PendingAction),
    Look(Pos),
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

pub struct Mission {
    pub driver: TurnDriver,
    pub queue: CommandQueue,
    pub mode: InputMode,
    pub speed: Speed,
    pub log: Vec<String>,
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
            explored,
            frame: 0,
        };
        mission.update_explored(data);
        mission
    }

    pub fn world(&self) -> &World {
        self.driver.world()
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

    /// The player's current field of view. Low cover blocks the view of a
    /// crouched player symmetrically.
    pub fn visible_tiles(&self, data: &GameData) -> Vec<Pos> {
        let world = self.driver.world();
        let player = world.player_actor();
        tiles_visible_from(
            player.pos,
            data.tuning.player_vision_range,
            &world.map,
            world.sight_blocker(player.crouched),
        )
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

    pub fn handle_input(&mut self, data: &GameData, input: ShellInput) {
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
            InputMode::TargetSelect { candidates, index } => {
                let (candidates, index) = (candidates.clone(), *index);
                self.handle_target_select(data, candidates, index, input);
            }
        }
    }

    fn handle_normal(&mut self, data: &GameData, input: ShellInput) {
        match input {
            ShellInput::Up => self.enqueue(Command::Move(Dir4::North)),
            ShellInput::Down => self.enqueue(Command::Move(Dir4::South)),
            ShellInput::Left => self.enqueue(Command::Move(Dir4::West)),
            ShellInput::Right => self.enqueue(Command::Move(Dir4::East)),
            ShellInput::Char('.') => self.enqueue(Command::Wait),
            ShellInput::Char('c') => self.enqueue(Command::ToggleCrouch),
            ShellInput::Char('r') => self.enqueue(Command::DrawOrHolster),
            ShellInput::Char('g') => self.mode = InputMode::Pending(PendingAction::Garrote),
            ShellInput::Char('p') => self.mode = InputMode::Pending(PendingAction::Pickpocket),
            ShellInput::Char('d') => self.mode = InputMode::Pending(PendingAction::Disguise),
            ShellInput::Char('h') => self.mode = InputMode::Pending(PendingAction::HideBody),
            ShellInput::Char('o') => self.mode = InputMode::Pending(PendingAction::OpenDoor),
            ShellInput::Char('k') => self.mode = InputMode::Pending(PendingAction::CloseDoor),
            ShellInput::Char('b') => {
                if matches!(self.world().player_actor().hands, Hands::CarryingBody(_)) {
                    self.enqueue(Command::DropBody);
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
                self.queue.pause();
                self.mode = InputMode::Look(self.world().player_actor().pos);
                self.push_log("look mode: queue paused".to_string());
            }
            ShellInput::Char(' ') => {
                self.queue.toggle_paused();
                let state = if self.queue.is_paused() {
                    "paused"
                } else {
                    "running"
                };
                self.push_log(format!("queue {state}"));
            }
            ShellInput::Char('[') => {
                self.speed = self.speed.slower();
                self.push_log(format!("speed: {}", self.speed.label()));
            }
            ShellInput::Char(']') => {
                self.speed = self.speed.faster();
                self.push_log(format!("speed: {}", self.speed.label()));
            }
            ShellInput::Backspace if self.queue.remove_newest().is_some() => {
                self.push_log("newest queued command removed".to_string());
            }
            ShellInput::Esc => {
                let dropped = self.queue.clear();
                if dropped > 0 {
                    self.push_log(format!("queue cleared ({dropped} commands)"));
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
            // 'b' twice targets the player's own tile (a body underfoot).
            ShellInput::Char('b') if action == PendingAction::CarryBody => None,
            ShellInput::Esc => {
                self.mode = InputMode::Normal;
                return;
            }
            _ => {
                self.mode = InputMode::Normal;
                return;
            }
        };
        self.mode = InputMode::Normal;
        let player_pos = self.world().player_actor().pos;
        let target_pos = match dir {
            Some(dir) => player_pos.step(dir),
            None => player_pos,
        };
        let command = self.command_for_target(data, action, target_pos);
        match command {
            Some(command) => self.enqueue(command),
            None => self.push_log("nothing suitable there".to_string()),
        }
    }

    /// Captures the stable domain ID for a directional action target at
    /// enqueue time; validation happens only when the command executes.
    fn command_for_target(
        &self,
        _data: &GameData,
        action: PendingAction,
        pos: Pos,
    ) -> Option<Command> {
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
                // Exiting look mode keeps the queue paused until the
                // player explicitly resumes.
                self.mode = InputMode::Normal;
                self.push_log("look mode off (queue still paused)".to_string());
            }
            ShellInput::Char(' ') => {
                self.queue.toggle_paused();
            }
            _ => {}
        }
    }

    fn handle_target_select(
        &mut self,
        data: &GameData,
        candidates: Vec<ActorId>,
        index: usize,
        input: ShellInput,
    ) {
        let _ = data;
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
                let capacity = self.queue.capacity();
                self.push_log(format!(
                    "queue full ({capacity}/{capacity}): input rejected"
                ));
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
    /// due (idle, paused, or mission over).
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
                // offender and cancel the queued remainder with the reason.
                self.queue.pop_head();
                let dropped = self.queue.clear();
                self.reject_feedback(reason, dropped);
                false
            }
        }
    }

    fn reject_feedback(&mut self, reason: RejectReason, dropped: usize) {
        if dropped > 0 {
            self.push_log(format!(
                "rejected: {} ({dropped} queued commands cancelled)",
                reason.message()
            ));
        } else {
            self.push_log(format!("rejected: {}", reason.message()));
        }
    }

    fn absorb(&mut self, report: TurnReport) {
        for message in &report.events.messages {
            self.push_log(message.clone());
        }
        for message in &report.perception {
            self.push_log(message.clone());
        }
        if let Some(ActionResult::Failed(why)) = &report.events.player_result {
            // In-turn failure: the turn passed; cancel the remainder.
            let dropped = self.queue.clear();
            if dropped > 0 {
                self.push_log(format!(
                    "action failed: {why}; {dropped} queued commands cancelled"
                ));
            }
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

    /// A one-line description of a looked-at tile, honest about what the
    /// player can currently see.
    pub fn describe(&self, data: &GameData, pos: Pos, visible: bool) -> String {
        let world = self.world();
        if !visible && !self.is_explored(pos) {
            return "unseen".to_string();
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(room) = world.room_at(pos) {
            parts.push(format!("{} [{}]", room.name, room.zone.name()));
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

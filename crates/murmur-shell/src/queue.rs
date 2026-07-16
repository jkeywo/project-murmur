//! The player command queue.
//!
//! A bounded FIFO of controller commands. Entries are intentions, never
//! prevalidated simulation actions: the head is translated against the
//! live world only when it is due to execute. Capacity is exactly the
//! authored `queue_capacity` (32), always visible; overflow rejects the
//! new command visibly without disturbing queued input. Escape clears,
//! Backspace removes the newest entry, Space pauses or resumes
//! consumption, and look mode pauses without clearing.

use murmur_core::actions::Command;
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct CommandQueue {
    entries: VecDeque<Command>,
    capacity: usize,
    paused: bool,
}

/// What happened when input tried to join the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Accepted,
    /// The queue was full; the command was rejected visibly and existing
    /// entries were left untouched.
    RejectedFull,
}

impl CommandQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
            paused: false,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn pause(&mut self) {
        self.paused = true;
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    pub fn toggle_paused(&mut self) {
        self.paused = !self.paused;
    }

    pub fn push(&mut self, command: Command) -> EnqueueOutcome {
        if self.entries.len() >= self.capacity {
            return EnqueueOutcome::RejectedFull;
        }
        self.entries.push_back(command);
        EnqueueOutcome::Accepted
    }

    /// The command due to execute next, without consuming it.
    pub fn head(&self) -> Option<&Command> {
        self.entries.front()
    }

    /// Consumes the head after the driver accepted it.
    pub fn pop_head(&mut self) -> Option<Command> {
        self.entries.pop_front()
    }

    /// Backspace: removes the newest command.
    pub fn remove_newest(&mut self) -> Option<Command> {
        self.entries.pop_back()
    }

    /// Escape, rejection, or in-turn failure: drops everything.
    pub fn clear(&mut self) -> usize {
        let dropped = self.entries.len();
        self.entries.clear();
        dropped
    }

    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use murmur_core::geom::Dir4;

    fn queue() -> CommandQueue {
        CommandQueue::new(32)
    }

    #[test]
    fn capacity_is_exactly_32_and_overflow_rejects_visibly() {
        let mut q = queue();
        for _ in 0..32 {
            assert_eq!(q.push(Command::Wait), EnqueueOutcome::Accepted);
        }
        assert_eq!(q.len(), 32);
        assert_eq!(q.push(Command::Wait), EnqueueOutcome::RejectedFull);
        assert_eq!(q.len(), 32, "rejected input must not disturb the queue");
    }

    #[test]
    fn fifo_order_is_preserved() {
        let mut q = queue();
        q.push(Command::Move(Dir4::North));
        q.push(Command::Wait);
        q.push(Command::Move(Dir4::East));
        assert_eq!(q.pop_head(), Some(Command::Move(Dir4::North)));
        assert_eq!(q.pop_head(), Some(Command::Wait));
        assert_eq!(q.pop_head(), Some(Command::Move(Dir4::East)));
        assert_eq!(q.pop_head(), None);
    }

    #[test]
    fn backspace_removes_only_the_newest() {
        let mut q = queue();
        q.push(Command::Move(Dir4::North));
        q.push(Command::Move(Dir4::East));
        assert_eq!(q.remove_newest(), Some(Command::Move(Dir4::East)));
        assert_eq!(q.len(), 1);
        assert_eq!(q.head(), Some(&Command::Move(Dir4::North)));
    }

    #[test]
    fn clear_reports_how_many_were_cancelled() {
        let mut q = queue();
        q.push(Command::Wait);
        q.push(Command::Wait);
        q.push(Command::Wait);
        assert_eq!(q.clear(), 3);
        assert!(q.is_empty());
    }

    #[test]
    fn pause_toggling() {
        let mut q = queue();
        assert!(!q.is_paused());
        q.toggle_paused();
        assert!(q.is_paused());
        q.toggle_paused();
        assert!(!q.is_paused());
    }
}

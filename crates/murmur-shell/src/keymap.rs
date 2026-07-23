//! The single source of truth for mission key bindings.
//!
//! Every surface that names a key reads it from here: the clickable action
//! palette, the in-mission help overlay, and the start screen's key list.
//! Before this module the same fifteen literals were written out in four
//! places that could drift apart silently; a drift test in `mission` now
//! keeps the table and the dispatch match honest about each other.
//!
//! The words are not here either: entries hold `data/loc/strings.csv` ids
//! and resolve through accessors, so the table stays a `const`.
//!
//! Availability is deliberately *not* in the table. Whether an action can
//! be used right now depends on live world state, so it stays in
//! `Mission::action_block`, keyed by the same char.

/// How an action reads on the help screen: verbs that do the same kind of
/// work are grouped so the list can be skimmed rather than read.
use murmur_core::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// Moving, waiting, and changing stance.
    Stance,
    /// Quiet work: disguises, pockets, locks, distractions.
    Stealth,
    /// Loud work: weapons and bodies.
    Force,
    /// Doors and machines.
    World,
}

impl Category {
    pub fn title(self) -> &'static str {
        match self {
            Category::Stance => tr!("keymap.category.stance"),
            Category::Stealth => tr!("keymap.category.stealth"),
            Category::Force => tr!("keymap.category.force"),
            Category::World => tr!("keymap.category.world"),
        }
    }

    /// Every category, in the order the help overlay lists them.
    pub const ALL: [Category; 4] = [
        Category::Stance,
        Category::Stealth,
        Category::Force,
        Category::World,
    ];
}

/// One keyed action: the palette shows [`ActionKey::label`], the help
/// overlay shows [`ActionKey::help`].
///
/// The table stores string *ids*, not words. It has to: `ACTIONS` is a
/// `const`, and a catalogue lookup is a function call. Resolving through
/// accessors keeps the table const-evaluable and means a retranslation
/// needs no code change at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionKey {
    pub key: char,
    /// Catalogue id of the short palette label; the text is kept under
    /// fourteen columns to fit the sidebar's two-per-row layout.
    pub label_id: &'static str,
    /// Catalogue id of the line explaining what the action does and needs.
    pub help_id: &'static str,
    pub category: Category,
}

impl ActionKey {
    /// Short palette label.
    pub fn label(&self) -> &'static str {
        murmur_core::loc::text(self.label_id)
    }

    /// One line explaining what the action does and what it needs.
    pub fn help(&self) -> &'static str {
        murmur_core::loc::text(self.help_id)
    }
}

/// The clickable action palette, in palette order. Every entry here must
/// have a match arm in `Mission::handle_normal`; `keymap_matches_dispatch`
/// enforces that.
pub const ACTIONS: &[ActionKey] = &[
    ActionKey {
        key: '.',
        label_id: "keymap.action.wait.label",
        help_id: "keymap.action.wait.help",
        category: Category::Stance,
    },
    ActionKey {
        key: 'c',
        label_id: "keymap.action.crouch.label",
        help_id: "keymap.action.crouch.help",
        category: Category::Stance,
    },
    ActionKey {
        key: 'r',
        label_id: "keymap.action.draw.label",
        help_id: "keymap.action.draw.help",
        category: Category::Force,
    },
    ActionKey {
        key: 'g',
        label_id: "keymap.action.garrote.label",
        help_id: "keymap.action.garrote.help",
        category: Category::Force,
    },
    ActionKey {
        key: 'f',
        label_id: "keymap.action.shoot.label",
        help_id: "keymap.action.shoot.help",
        category: Category::Force,
    },
    ActionKey {
        key: 'p',
        label_id: "keymap.action.pickpocket.label",
        help_id: "keymap.action.pickpocket.help",
        category: Category::Stealth,
    },
    ActionKey {
        key: 'd',
        label_id: "keymap.action.disguise.label",
        help_id: "keymap.action.disguise.help",
        category: Category::Stealth,
    },
    ActionKey {
        key: 'b',
        label_id: "keymap.action.carry.label",
        help_id: "keymap.action.carry.help",
        category: Category::Force,
    },
    ActionKey {
        key: 'h',
        label_id: "keymap.action.hide.label",
        help_id: "keymap.action.hide.help",
        category: Category::Force,
    },
    ActionKey {
        key: 'o',
        label_id: "keymap.action.open.label",
        help_id: "keymap.action.open.help",
        category: Category::World,
    },
    ActionKey {
        key: 'k',
        label_id: "keymap.action.close.label",
        help_id: "keymap.action.close.help",
        category: Category::World,
    },
    ActionKey {
        key: 'l',
        label_id: "keymap.action.lock.label",
        help_id: "keymap.action.lock.help",
        category: Category::Stealth,
    },
    ActionKey {
        key: 't',
        label_id: "keymap.action.noise.label",
        help_id: "keymap.action.noise.help",
        category: Category::Stealth,
    },
    ActionKey {
        key: 'u',
        label_id: "keymap.action.machine.label",
        help_id: "keymap.action.machine.help",
        category: Category::World,
    },
    ActionKey {
        key: 'e',
        label_id: "keymap.action.lead.label",
        help_id: "keymap.action.lead.help",
        category: Category::Stealth,
    },
    ActionKey {
        key: 'n',
        label_id: "keymap.action.plant.label",
        help_id: "keymap.action.plant.help",
        category: Category::Stealth,
    },
    ActionKey {
        key: ';',
        label_id: "keymap.action.look.label",
        help_id: "keymap.action.look.help",
        category: Category::Stance,
    },
];

/// Keys that are not palette actions: movement, interface, and the
/// controls the palette has no room for. Listed on the help overlay and
/// the start screen so nothing is discoverable only by accident.
///
/// Each is `(key, short label id, description id)`. The short label is
/// what the start screen's compact list shows; the description is for the
/// help overlay, which has room for a full sentence. As with [`ActionKey`],
/// the table holds catalogue ids so it can stay a `const`; read them
/// through [`control_short`] and [`control_help`].
///
/// The key names themselves are not translated: they are what is printed on
/// the keyboard, and a player looking for `Esc` needs to find `Esc`.
pub const CONTROLS: &[(&str, &str, &str)] = &[
    (
        "arrows",
        "keymap.control.move.short",
        "keymap.control.move.help",
    ),
    (
        "1-6",
        "keymap.control.slot.short",
        "keymap.control.slot.help",
    ),
    (
        "[ ]",
        "keymap.control.speed.short",
        "keymap.control.speed.help",
    ),
    (
        "z",
        "keymap.control.fast_forward.short",
        "keymap.control.fast_forward.help",
    ),
    (
        "< >",
        "keymap.control.floor.short",
        "keymap.control.floor.help",
    ),
    (
        "j",
        "keymap.control.contract.short",
        "keymap.control.contract.help",
    ),
    (
        "C",
        "keymap.control.cheats.short",
        "keymap.control.cheats.help",
    ),
    (
        "Esc",
        "keymap.control.cancel.short",
        "keymap.control.cancel.help",
    ),
    (
        "Backspace",
        "keymap.control.undo.short",
        "keymap.control.undo.help",
    ),
    ("?", "keymap.control.help.short", "keymap.control.help.help"),
    (
        "Q",
        "keymap.control.abandon.short",
        "keymap.control.abandon.help",
    ),
];

/// The compact start-screen label for a [`CONTROLS`] entry.
pub fn control_short(entry: &(&str, &str, &str)) -> &'static str {
    murmur_core::loc::text(entry.1)
}

/// The full help-overlay description for a [`CONTROLS`] entry.
pub fn control_help(entry: &(&str, &str, &str)) -> &'static str {
    murmur_core::loc::text(entry.2)
}

/// The action for a key, if the key is a palette action.
pub fn action(key: char) -> Option<&'static ActionKey> {
    ACTIONS.iter().find(|a| a.key == key)
}

/// Palette actions in a category, in palette order.
pub fn in_category(category: Category) -> impl Iterator<Item = &'static ActionKey> {
    ACTIONS.iter().filter(move |a| a.category == category)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_keys_are_unique() {
        for (index, entry) in ACTIONS.iter().enumerate() {
            assert!(
                ACTIONS[..index].iter().all(|other| other.key != entry.key),
                "duplicate action key {:?}",
                entry.key
            );
        }
    }

    #[test]
    fn every_action_belongs_to_a_listed_category() {
        for entry in ACTIONS {
            assert!(
                Category::ALL.contains(&entry.category),
                "{:?} is in a category the help overlay never renders",
                entry.key
            );
        }
    }

    #[test]
    fn palette_labels_fit_the_sidebar_column() {
        // The sidebar pads each entry to sixteen columns as "<key> <label>".
        for entry in ACTIONS {
            assert!(
                entry.label().chars().count() <= 14,
                "label {:?} overflows the palette column",
                entry.label()
            );
        }
    }
}

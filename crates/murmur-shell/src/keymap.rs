//! The single source of truth for mission key bindings.
//!
//! Every surface that names a key reads it from here: the clickable action
//! palette, the in-mission help overlay, and the start screen's key list.
//! Before this module the same fifteen literals were written out in four
//! places that could drift apart silently; a drift test in `mission` now
//! keeps the table and the dispatch match honest about each other.
//!
//! Availability is deliberately *not* in the table. Whether an action can
//! be used right now depends on live world state, so it stays in
//! `Mission::action_block`, keyed by the same char.

/// How an action reads on the help screen: verbs that do the same kind of
/// work are grouped so the list can be skimmed rather than read.
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
            Category::Stance => "stance",
            Category::Stealth => "quiet work",
            Category::Force => "force",
            Category::World => "the room",
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

/// One keyed action: the palette shows `label`, the help overlay shows
/// `help`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionKey {
    pub key: char,
    /// Short palette label; kept under fourteen columns to fit the
    /// sidebar's two-per-row layout.
    pub label: &'static str,
    /// One line explaining what the action does and what it needs.
    pub help: &'static str,
    pub category: Category,
}

/// The clickable action palette, in palette order. Every entry here must
/// have a match arm in `Mission::handle_normal`; `keymap_matches_dispatch`
/// enforces that.
pub const ACTIONS: &[ActionKey] = &[
    ActionKey {
        key: '.',
        label: "wait",
        help: "let a turn pass; Space does the same",
        category: Category::Stance,
    },
    ActionKey {
        key: 'c',
        label: "crouch",
        help: "crouch: quieter and harder to see, but slower to read the room",
        category: Category::Stance,
    },
    ActionKey {
        key: 'r',
        label: "draw/holster",
        help: "draw or put away a firearm; drawn weapons alarm anyone who sees them",
        category: Category::Force,
    },
    ActionKey {
        key: 'g',
        label: "garrote",
        help: "garrote someone from behind: silent, and lethal",
        category: Category::Force,
    },
    ActionKey {
        key: 'f',
        label: "shoot",
        help: "shoot a visible target; f cycles targets, Enter fires",
        category: Category::Force,
    },
    ActionKey {
        key: 'p',
        label: "pickpocket",
        help: "steal from the living, loot the dead",
        category: Category::Stealth,
    },
    ActionKey {
        key: 'd',
        label: "disguise",
        help: "take clothes from a body or a wardrobe; needs free hands",
        category: Category::Stealth,
    },
    ActionKey {
        key: 'b',
        label: "carry/drop",
        help: "pick up or put down a body; b again drops it on your own tile",
        category: Category::Force,
    },
    ActionKey {
        key: 'h',
        label: "hide body",
        help: "stow a carried body in a container so nobody finds it",
        category: Category::Force,
    },
    ActionKey {
        key: 'o',
        label: "open door",
        help: "open an adjacent door",
        category: Category::World,
    },
    ActionKey {
        key: 'k',
        label: "close door",
        help: "close an adjacent door behind you",
        category: Category::World,
    },
    ActionKey {
        key: 'l',
        label: "pick lock",
        help: "pick a locked door: slow, and suspicious if seen",
        category: Category::Stealth,
    },
    ActionKey {
        key: 't',
        label: "noisemaker",
        help: "throw a noisemaker at a tile to pull people towards it",
        category: Category::Stealth,
    },
    ActionKey {
        key: 'u',
        label: "use machine",
        help: "use an adjacent opportunity machine",
        category: Category::World,
    },
    ActionKey {
        key: ';',
        label: "look",
        help: "free look cursor; costs no time and pauses nothing you planned",
        category: Category::Stance,
    },
];

/// Keys that are not palette actions: movement, interface, and the
/// controls the palette has no room for. Listed on the help overlay and
/// the start screen so nothing is discoverable only by accident.
///
/// Each is `(key, short label, description)`. The short label is what the
/// start screen's compact list shows; the description is for the help
/// overlay, which has room for a full sentence.
pub const CONTROLS: &[(&str, &str, &str)] = &[
    (
        "arrows",
        "move",
        "move, or aim whatever you are currently pointing",
    ),
    ("1-6", "read a slot", "read what an inventory slot holds"),
    (
        "[ ]",
        "speed",
        "slow down or speed up the display; the run is unaffected",
    ),
    (
        "Esc",
        "cancel",
        "cancel what you are aiming, or stop what you planned",
    ),
    (
        "Backspace",
        "take back",
        "take back the last thing you planned",
    ),
    ("?", "help", "this help"),
    ("Q", "abandon run", "abandon the run (asks first)"),
];

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
                entry.label.chars().count() <= 14,
                "label {:?} overflows the palette column",
                entry.label
            );
        }
    }
}

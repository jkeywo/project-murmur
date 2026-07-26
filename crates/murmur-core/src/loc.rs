//! Localisation: every player-facing string, keyed by a stable ID.
//!
//! The source of truth is `data/loc/strings.csv`, a three-column table of
//! `id, context, text`. It is embedded at compile time alongside the RON data
//! files, for the same reason: the native and web builds must ship
//! byte-identical text, and a mission's log is part of its deterministic
//! replay.
//!
//! The mechanism is [`vellum_strings`] — the fleet's pipeline, which murmur's
//! own table and its coverage test were two of the sources for. What stays
//! here is what is murmur's: the embedded file, the pools the generator reads,
//! and the `tr!`/`trf!` macros that close over this catalogue.
//!
//! # Placeholder text
//!
//! Every string an agent wrote — which today is all of them — is wrapped in
//! `[square brackets]` in the CSV itself. The brackets are literal content,
//! not a runtime decoration, so what a translator reads in the file is
//! exactly what the game prints. When a human writes the real line, they
//! drop the brackets in the same edit. Anything bracketed on screen has not
//! been through a writer yet.
//!
//! # Interpolation
//!
//! Rust's `format!` needs a compile-time literal, so runtime strings carry
//! their own `{named}` slots filled by [`fmt`]:
//!
//! ```
//! # use murmur_core::loc;
//! let line = loc::fmt("ui.mission.here", &[("what", "a closed door")]);
//! ```
//!
//! Prefer the [`tr!`](crate::tr) and [`trf!`](crate::trf) macros at call
//! sites: they take a literal ID, which lets the coverage test scan the
//! source and prove that every ID used in code exists in the CSV, and that
//! the CSV carries no orphans.

use std::sync::OnceLock;

use vellum_strings::{Locale, Table};

const STRINGS_CSV: &str = include_str!("../../../data/loc/strings.csv");

/// Rendered in place of a string whose ID is not in the catalogue. Loud on
/// purpose, and a fixed static rather than the missing ID, so a lookup in a
/// render loop cannot leak memory one frame at a time.
pub use vellum_strings::MISSING;

/// The embedded catalogue, parsed once.
///
/// # Panics
///
/// If `strings.csv` is malformed. The file ships inside the binary, so a
/// failure here is a build that should never have been produced, and every
/// caller would otherwise have to handle an error that cannot occur in a
/// well-formed build.
pub fn catalogue() -> &'static Table {
    static CATALOGUE: OnceLock<Table> = OnceLock::new();
    CATALOGUE.get_or_init(|| match Table::parse(Locale::ENGLISH, STRINGS_CSV) {
        Ok(table) => table,
        Err(errors) => {
            let report: Vec<String> = errors.iter().map(ToString::to_string).collect();
            panic!(
                "data/loc/strings.csv is malformed:\n  {}",
                report.join("\n  ")
            );
        }
    })
}

/// The text for `id`. Borrowed from the process-wide catalogue, so this is
/// `&'static str` and drops straight into the `&'static str` fields the
/// keymap tables use.
pub fn text(id: &str) -> &'static str {
    catalogue().text(id)
}

/// Every string whose ID starts with `prefix`, in ID order.
///
/// This is how the authored pools (person names, districts, briefing reasons)
/// reach the generator: the RON files no longer carry the lists, so the
/// catalogue is the list. IDs are zero-padded (`names.first.01`) because the
/// backing map is ordered by ID and a mission's pick must not depend on how
/// many entries exist — unpadded, adding a tenth entry would reorder the
/// first nine and change every existing seed's output.
pub fn pool(prefix: &str) -> Vec<String> {
    catalogue()
        .with_prefix(prefix)
        .map(|(_, row)| row.text.clone())
        .collect()
}

/// The IDs behind [`pool`], for the coverage test — these are reached by
/// prefix rather than by a literal, so nothing can find them by scanning.
pub fn pool_ids(prefix: &str) -> Vec<String> {
    catalogue()
        .with_prefix(prefix)
        .map(|(id, _)| id.to_owned())
        .collect()
}

/// The text for `id` with each `{name}` slot replaced by its argument.
///
/// Unmatched slots are left as written rather than blanked: a visible
/// `{room}` on screen points at the bug, where an empty gap hides it.
pub fn fmt(id: &str, args: &[(&str, &str)]) -> String {
    catalogue().format(id, args)
}

/// Substitutes `{name}` slots in `template`. Split out from [`fmt`] so the
/// substitution itself is testable without a catalogue.
pub fn interpolate(template: &str, args: &[(&str, &str)]) -> String {
    vellum_strings::interpolate(template, args)
}

/// Looks up a localised string by literal ID.
///
/// The ID must be a string literal so the coverage test can find it by
/// scanning the source.
#[macro_export]
macro_rules! tr {
    ($id:literal) => {
        $crate::loc::text($id)
    };
}

/// Looks up a localised string and fills its `{named}` slots.
///
/// ```
/// # use murmur_core::trf;
/// let what = "a closed door";
/// let line = trf!("ui.mission.here", what = what);
/// ```
#[macro_export]
macro_rules! trf {
    ($id:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::loc::fmt($id, &[$((stringify!($name), &*::std::string::ToString::to_string(&$value))),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    // The parser is shared, but the behaviour murmur's strings.csv relies on
    // is asserted here too. An engine change that broke quoting or dropped
    // section markers should fail in the game that depends on it, not only in
    // the crate that made the change.

    #[test]
    fn parses_quoted_fields_and_embedded_commas() {
        let table = Table::parse(
            Locale::ENGLISH,
            "id,context,text\na.b,\"a note, with a comma\",\"say \"\"hi\"\"\"\n",
        )
        .expect("parses");
        assert_eq!(table.text("a.b"), "say \"hi\"");
        assert_eq!(
            table.row("a.b").expect("present").context,
            "a note, with a comma"
        );
    }

    /// murmur's table runs to six hundred rows; the `#` section markers and
    /// blank spacers are what keep it readable, and neither is a row.
    #[test]
    fn skips_blank_and_comment_rows() {
        let csv = "id,context,text\n\n# mission,,\na.b,note,[one]\n";
        assert_eq!(Table::parse(Locale::ENGLISH, csv).expect("parses").len(), 1);
    }

    #[test]
    fn rejects_the_authoring_mistakes_that_would_reach_the_screen() {
        for csv in [
            "id,context,text\na.b,note,[one]\na.b,note,[two]\n", // duplicate id
            "id,context,text\na.b,note,\n",                      // truncated edit
            "id,context,text\na.b,note,[found in {room]\n",      // unclosed slot
            "id,context,text\na.b,note,\"never closed",          // malformed CSV
        ] {
            assert!(Table::parse(Locale::ENGLISH, csv).is_err(), "{csv:?}");
        }
    }

    #[test]
    fn interpolates_named_slots() {
        assert_eq!(
            interpolate(
                "[{who} is in {room}]",
                &[("who", "a guard"), ("room", "VIP")]
            ),
            "[a guard is in VIP]"
        );
    }

    #[test]
    fn repeated_slots_all_fill() {
        assert_eq!(interpolate("{a}-{a}", &[("a", "x")]), "x-x");
    }

    #[test]
    fn embedded_catalogue_parses() {
        assert!(!catalogue().is_empty());
    }

    /// Every id used in code exists in the CSV, and every id in the CSV is
    /// used.
    ///
    /// This is the test that makes the whole scheme safe to refactor: a
    /// renamed id, a typo, or a string nobody prints any more is caught here
    /// rather than showing up on a briefing panel as `!!MISSING STRING!!`.
    ///
    /// The ids nothing can find by scanning — the per-spec text
    /// `GameData::resolve_text` builds from a structural id, and the ordered
    /// pools read by prefix — are handed over *expanded*, built from the
    /// loaded data rather than declared as a list of prefixes. That is what
    /// makes the check exact in both directions: a prefix allowlist covers a
    /// deleted item's abandoned row forever, and says nothing about a new
    /// item whose row was never written.
    #[test]
    fn every_id_is_both_defined_and_used() {
        use vellum_strings::{AuditInput, audit};

        let data = crate::data::GameData::embedded().expect("the embedded data set loads");
        let roots = [
            concat!(env!("CARGO_MANIFEST_DIR"), "/../murmur-core/src"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../murmur-campaign/src"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../murmur-shell/src"),
        ]
        .map(std::path::PathBuf::from);

        let report = audit(
            catalogue(),
            AuditInput::new(&roots)
                .derived(data.text_ids())
                // This module holds the scanner's own marker literals and has
                // no lookups of its own beyond the doc examples.
                .skip("loc.rs"),
        );
        assert!(report.files_scanned > 0, "found no sources to scan");
        assert!(report.ok(), "\n{report}");
    }
}

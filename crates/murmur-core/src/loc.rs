//! Localisation: every player-facing string, keyed by a stable ID.
//!
//! The source of truth is `data/loc/strings.csv`, a three-column table of
//! `id, context, english`. It is embedded at compile time alongside the RON
//! data files, for the same reason: the native and web builds must ship
//! byte-identical text, and a mission's log is part of its deterministic
//! replay.
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
//! sites: they take a literal ID, which lets `loc_ids_all_resolve` scan the
//! source and prove that every ID used in code exists in the CSV, and that
//! the CSV carries no orphans.

use std::collections::BTreeMap;
use std::sync::OnceLock;

const STRINGS_CSV: &str = include_str!("../../../data/loc/strings.csv");

/// Rendered in place of a string whose ID is not in the catalogue. Loud on
/// purpose, and a fixed static rather than the missing ID, so a lookup in a
/// render loop cannot leak memory one frame at a time. The offending ID is
/// named by `debug_assert!` and by the coverage test.
pub const MISSING: &str = "!!MISSING STRING!!";

/// One row of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Stable lookup key, dot-separated by area: `hub.stash.empty`.
    pub id: String,
    /// What the string is for and where it appears — the note a translator
    /// reads instead of guessing from the English.
    pub context: String,
    /// The English text, including any `{named}` interpolation slots.
    pub english: String,
}

/// Every string in the game, indexed by ID.
#[derive(Debug, Default)]
pub struct Catalogue {
    entries: BTreeMap<String, Entry>,
}

/// Why a CSV failed to load. Any of these is a build-breaking authoring
/// mistake, not something to recover from at runtime.
#[derive(Debug, PartialEq, Eq)]
pub enum LocError {
    MissingHeader,
    /// Row `row` (1-based, counting the header) has the wrong column count.
    BadColumnCount {
        row: usize,
        found: usize,
    },
    /// The same ID appears twice; the second one would silently win.
    DuplicateId {
        row: usize,
        id: String,
    },
    /// An empty `english` cell — almost always a truncated edit.
    EmptyText {
        row: usize,
        id: String,
    },
    /// A `{` with no closing `}`, which would print as literal noise.
    UnclosedPlaceholder {
        row: usize,
        id: String,
    },
    /// The file is not well-formed CSV: an unterminated quote, or text after a
    /// closing one. Previously these were quietly absorbed and produced
    /// plausible-looking rubbish; the shared reader refuses them instead.
    Malformed {
        line: usize,
        message: &'static str,
    },
}

impl std::fmt::Display for LocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LocError::MissingHeader => write!(f, "strings.csv has no header row"),
            LocError::BadColumnCount { row, found } => {
                write!(f, "row {row}: expected 3 columns, found {found}")
            }
            LocError::DuplicateId { row, id } => write!(f, "row {row}: duplicate id {id:?}"),
            LocError::EmptyText { row, id } => write!(f, "row {row}: {id:?} has no english text"),
            LocError::UnclosedPlaceholder { row, id } => {
                write!(f, "row {row}: {id:?} has an unclosed {{placeholder")
            }
            LocError::Malformed { line, message } => {
                write!(f, "strings.csv line {line}: {message}")
            }
        }
    }
}

impl std::error::Error for LocError {}

impl Catalogue {
    /// Parses the three-column table, rejecting the authoring mistakes that
    /// would otherwise surface as garbled text mid-mission.
    pub fn parse(source: &str) -> Result<Self, LocError> {
        let parsed = vellum_strings::parse_csv(source).map_err(|error| LocError::Malformed {
            line: error.line,
            message: error.message,
        })?;
        let mut rows = parsed.into_iter().enumerate();
        let (_, header) = rows.next().ok_or(LocError::MissingHeader)?;
        if header.len() < 3 || header[0].trim() != "id" {
            return Err(LocError::MissingHeader);
        }
        let mut entries = BTreeMap::new();
        for (index, row) in rows {
            let row_number = index + 1;
            // Blank spacer rows and `#` section markers keep the CSV
            // readable in a text editor without meaning anything.
            let first = row.first().map(|s| s.trim()).unwrap_or("");
            if first.is_empty() || first.starts_with('#') {
                continue;
            }
            if row.len() != 3 {
                return Err(LocError::BadColumnCount {
                    row: row_number,
                    found: row.len(),
                });
            }
            let entry = Entry {
                id: first.to_string(),
                context: row[1].trim().to_string(),
                english: row[2].clone(),
            };
            if entry.english.trim().is_empty() {
                return Err(LocError::EmptyText {
                    row: row_number,
                    id: entry.id,
                });
            }
            if entry.english.matches('{').count() != entry.english.matches('}').count() {
                return Err(LocError::UnclosedPlaceholder {
                    row: row_number,
                    id: entry.id,
                });
            }
            if entries.contains_key(&entry.id) {
                return Err(LocError::DuplicateId {
                    row: row_number,
                    id: entry.id,
                });
            }
            entries.insert(entry.id.clone(), entry);
        }
        Ok(Catalogue { entries })
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.get(id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Every string whose ID starts with `prefix`, in ID order.
    ///
    /// This is how the authored pools (person names, districts, briefing
    /// reasons) reach the generator: the RON files no longer carry the
    /// lists, so the catalogue is the list. IDs are zero-padded
    /// (`names.first.01`) because the backing map is ordered by ID and a
    /// mission's pick must not depend on how many entries exist —
    /// unpadded, adding a tenth entry would reorder the first nine and
    /// change every existing seed's output.
    pub fn with_prefix(&self, prefix: &str) -> Vec<&str> {
        self.entries
            .range(prefix.to_string()..)
            .take_while(|(id, _)| id.starts_with(prefix))
            .map(|(_, entry)| entry.english.as_str())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The embedded catalogue, parsed once.
///
/// # Panics
///
/// If `strings.csv` is malformed. The file ships inside the binary, so a
/// failure here is a build that should never have been produced, and every
/// caller would otherwise have to handle an error that cannot occur in a
/// well-formed build.
pub fn catalogue() -> &'static Catalogue {
    static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();
    CATALOGUE.get_or_init(|| match Catalogue::parse(STRINGS_CSV) {
        Ok(catalogue) => catalogue,
        Err(error) => panic!("data/loc/strings.csv is malformed: {error}"),
    })
}

/// The text for `id`. Borrowed from the process-wide catalogue, so this is
/// `&'static str` and drops straight into the `&'static str` fields the
/// keymap tables use.
pub fn text(id: &str) -> &'static str {
    match catalogue().get(id) {
        Some(entry) => entry.english.as_str(),
        None => {
            debug_assert!(false, "no localised string for id {id:?}");
            MISSING
        }
    }
}

/// The text for `id` with each `{name}` slot replaced by its argument.
///
/// Unmatched slots are left as written rather than blanked: a visible
/// `{room}` on screen points at the bug, where an empty gap hides it.
pub fn fmt(id: &str, args: &[(&str, &str)]) -> String {
    interpolate(text(id), args)
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

    // The reader itself is shared, but the behaviour murmur's strings.csv
    // relies on is asserted here too. An engine change that broke quoting
    // should fail in the game that depends on it, not only in the crate that
    // made it.
    fn parse_csv(source: &str) -> Vec<Vec<String>> {
        vellum_strings::parse_csv(source).expect("well-formed CSV")
    }

    #[test]
    fn parses_quoted_fields_and_embedded_commas() {
        let rows =
            parse_csv("id,context,english\na.b,\"a note, with a comma\",\"say \"\"hi\"\"\"\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec!["a.b", "a note, with a comma", "say \"hi\""]);
    }

    #[test]
    fn trailing_newline_does_not_make_a_phantom_row() {
        assert_eq!(parse_csv("id,context,english\n").len(), 1);
    }

    /// The shared reader is stricter than the one it replaced: malformed CSV
    /// is refused rather than absorbed into plausible-looking rubbish.
    #[test]
    fn malformed_csv_is_rejected_rather_than_guessed_at() {
        assert!(matches!(
            Catalogue::parse("id,context,english\na.b,note,\"never closed"),
            Err(LocError::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let csv = "id,context,english\na.b,note,[one]\na.b,note,[two]\n";
        assert!(matches!(
            Catalogue::parse(csv),
            Err(LocError::DuplicateId { .. })
        ));
    }

    #[test]
    fn rejects_empty_text() {
        let csv = "id,context,english\na.b,note,\n";
        assert!(matches!(
            Catalogue::parse(csv),
            Err(LocError::EmptyText { .. })
        ));
    }

    #[test]
    fn rejects_unclosed_placeholder() {
        let csv = "id,context,english\na.b,note,[found in {room]\n";
        assert!(matches!(
            Catalogue::parse(csv),
            Err(LocError::UnclosedPlaceholder { .. })
        ));
    }

    #[test]
    fn skips_blank_and_comment_rows() {
        let csv = "id,context,english\n\n# mission,,\na.b,note,[one]\n";
        let catalogue = Catalogue::parse(csv).expect("parses");
        assert_eq!(catalogue.len(), 1);
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
    /// renamed id, a typo, or a string nobody prints any more is caught
    /// here rather than showing up on a briefing panel as
    /// `!!MISSING STRING!!`. It works because `tr!`/`trf!` take literal
    /// ids, so the ids are all findable by scanning the sources.
    #[test]
    fn every_id_is_both_defined_and_used() {
        use std::collections::BTreeSet;

        // Ids reached by a derived lookup rather than a literal — the
        // per-spec text `GameData::resolve_text` builds from a structural
        // id, and the ordered pools read by prefix. They cannot be found by
        // scanning, so they are named here.
        const DERIVED_PREFIXES: &[&str] = &[
            "venue.",
            "disguise.",
            "item.",
            "opportunity.",
            "room.",
            "names.first.",
            "names.last.",
            "briefing.reason.",
            "campaign.district.",
        ];

        let mut looked_up: BTreeSet<String> = BTreeSet::new();
        let roots = [
            concat!(env!("CARGO_MANIFEST_DIR"), "/../murmur-core/src"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../murmur-campaign/src"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../murmur-shell/src"),
        ];
        let mut sources = Vec::new();
        for root in roots {
            collect_rust_files(std::path::Path::new(root), &mut sources);
        }
        assert!(!sources.is_empty(), "found no sources to scan");
        let mut corpus = String::new();
        for path in &sources {
            // This module holds the scanner's own marker literals, and has
            // no lookups of its own beyond the doc examples.
            if path.ends_with("loc.rs") {
                continue;
            }
            let text = std::fs::read_to_string(path).expect("readable source");
            // The id argument of tr!(…), trf!(…) and loc::fmt(…). Newlines
            // are allowed between the paren and the literal: rustfmt breaks
            // the longer calls across lines.
            for (marker, offset) in [("tr!(", 4), ("trf!(", 5), ("fmt(", 4)] {
                let mut scanned = 0usize;
                while let Some(at) = text[scanned..].find(marker) {
                    let start = scanned + at;
                    scanned = start + offset;
                    // `tr!(` is a suffix of `include_str!(`, so a marker
                    // that continues an identifier is not a lookup.
                    if text[..start]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_')
                    {
                        continue;
                    }
                    let after = text[scanned..].trim_start();
                    if let Some(body) = after.strip_prefix('"')
                        && let Some(end) = body.find('"')
                    {
                        looked_up.insert(body[..end].to_string());
                    }
                }
            }
            corpus.push_str(&text);
        }

        let defined: BTreeSet<&str> = catalogue().ids().collect();

        // Direction one: a typo or a rename in a `tr!` never silently
        // becomes `!!MISSING STRING!!` on screen.
        let missing: Vec<&String> = looked_up
            .iter()
            .filter(|id| !defined.contains(id.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "ids used in code but absent from data/loc/strings.csv: {missing:#?}"
        );

        // Direction two: no dead rows. This one searches for the id as a
        // literal anywhere in the sources rather than reusing the lookups
        // above, because some ids are held in `const` tables (the keymap)
        // instead of being passed to a macro.
        let orphans: Vec<&&str> = defined
            .iter()
            .filter(|id| !DERIVED_PREFIXES.iter().any(|p| id.starts_with(p)))
            .filter(|id| !corpus.contains(&format!("\"{id}\"")))
            .collect();
        assert!(
            orphans.is_empty(),
            "ids in data/loc/strings.csv that nothing prints: {orphans:#?}"
        );
    }

    fn collect_rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
}

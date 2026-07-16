# Project Murmur PASM specification

This directory is the source of truth for Project Murmur's system
boundaries and gameplay design.

- `core/foundation.yaml` — the MVP design entities, now carrying
  implementation mappings (paths, symbols, tests) and lifecycle statuses
  that reflect the shipped code.
- `core/decisions.yaml` — decision records made during implementation:
  technology selections, interpretation calls, and rules discovered
  through playtesting, each with rationale and references.

Add or update the model for a system before (or alongside) its
implementation, and keep `uv run pasm validate` green — CI runs it on
every push.

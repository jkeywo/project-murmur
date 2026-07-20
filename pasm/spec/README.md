# Project Murmur PASM specification

This directory is the source of truth for Project Murmur's system
boundaries and gameplay design.

- `core/foundation.yaml` — the MVP design entities, now carrying
  implementation mappings (paths, symbols, tests) and lifecycle statuses
  that reflect the shipped code.
- `core/decisions.yaml` — decision records made during implementation:
  technology selections, interpretation calls, and rules discovered
  through playtesting, each with rationale and references.
- `milestones/milestone-2.yaml` — Milestone 2: Contract Sandbox. Shipped.
- `milestones/milestone-3.yaml` — Milestone 3: Deep Venues, Protected
  Targets, Interlocking Routines. Shipped.
- `milestones/milestone-4.yaml` — Milestone 4: The Job Is Not Always A
  Kill. Proposed; objective variety, which the roadmap gates city-scale
  work behind.
- `milestones/milestone-5.yaml` — Milestone 5: Persistence. Proposed;
  venue memory, survivors and nemeses, faction state, safehouses.
- `milestones/milestone-6.yaml` — Milestone 6: The City. Proposed; map
  scales, police response, street life, safehouses as places.
- `roadmap/long-term-wishlist.yaml` — explicitly deferred, non-committed
  campaign and city-scale wishlist features.

Add or update the model for a system before (or alongside) its
implementation, and keep `uv run pasm validate` green — CI runs it on
every push.

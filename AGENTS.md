@RTK.md

# Project Murmur — Agent Guide

You are writing **Project Murmur**, a **turn-based ASCII social-stealth roguelike** set in a procedurally generated two-storey nightclub. The player blends in using a disguise system (clothes determine belonging), assassinates a target (garrote or shoot), and escapes through an extraction exit before guards close in.

## Tech Stack

| Layer | Technology |
|---|---|
| Simulation core | Rust, `murmur-core` — deterministic, discrete-turn, no engine/platform deps |
| Terminal UI | ratatui via `murmur-shell` (shared by native and web targets) |
| Native binary | Bevy + `bevy_ratatui` via `murmur-native` |
| Web build | WASM via `murmur-web`, deployed to GitHub Pages |
| Game data | RON files under `data/`, embedded at compile time |
| Architecture model | PASM (Python) — YAML spec under `pasm/spec/` |
| CI | GitHub Actions — tests, clippy, PASM validation, deploy on green default branch |

### Workspace crates

- `murmur-core` — world, generator with reachability proof, actions, AI, perception, turn driver, replay
- `murmur-shell` — backend-neutral controller + ratatui screens, command queue, input modes
- `murmur-native` — Bevy executable rendering into a real terminal
- `murmur-web` — same shell compiled to WASM, rendered by Ratzilla

## PASM — Keep It Up to Date

PASM (Project Architecture & System Model) is the executable architecture model. **Every feature or structural change must be reflected in `pasm/spec/` before or alongside the implementation.** This is not optional — PASM validation gates CI.

### Rules

1. **Model first, then build** — add entity/component definitions to `pasm/spec/` before writing Rust code for a new system.
2. **Record decisions** — when you make a design choice during implementation, log it in `pasm/spec/core/decisions.yaml`.
3. **Run validation** — after any model change, run `uv run pasm validate` and fix errors before committing.
4. **Keep evidence current** — run `uv run pasm scan --json` periodically to observe implementation coverage; close gaps by updating either the model or the code.
5. **Never leave dead spec** — when you remove or refactor a system, update or archive its PASM declarations.

### Quick commands

```powershell
uv sync --group dev                    # install PASM tooling
uv run pasm validate                   # schema, references, mapping checks
uv run pasm scan --json                # observe implementation evidence
uv run pasm query entity <id>          # explore model entities
```

## Development Commands

```powershell
cargo test --workspace --exclude murmur-web    # all tests
cargo clippy --workspace --exclude murmur-web --all-targets -- -D warnings
cargo fmt --all
cargo run --release -p murmur-native           # run the game
cargo run -p murmur-core --example dump_map -- 42   # print a generated club
cargo run -p murmur-shell --example screenshot -- 42  # headless render
```

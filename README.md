# Project Murmur

A turn-based ASCII social-stealth roguelike. One generated two-storey
nightclub, one target, one way out. Blend into the crowd — your clothes
decide where you belong — then garrote or shoot the target and walk out
through an extraction exit before the guards close in.

**Play in the browser:** <https://jkeywo.github.io/project-murmur/>

**Play natively** (any terminal, Windows/macOS/Linux):

```powershell
cargo run --release -p murmur-native            # random mission
cargo run --release -p murmur-native -- --seed 42
```

## How it plays

Commands queue up (32 slots, always visible) and execute one per turn
against an authoritative, deterministic simulation. A command that cannot
be submitted is rejected without time passing and cancels the queued
remainder; a valid action can still fail during simultaneous resolution,
and that turn stands. NPCs run generated routines on a relaxed cadence,
see through facing cones, grow suspicious of trespass, crouching, drawn
weapons, and bodies, and propagate alerts by line of sight only.

Every mission derives from a seed: layout, population, schedules, items,
and tie-breaker randomness. Replaying the accepted commands against the
seed reproduces the identical result, turn by turn.

## Workspace

| Crate | Role |
| --- | --- |
| `murmur-core` | Deterministic simulation: world, generator with reachability proof, actions, AI, perception, turn driver, replay. No engine or platform dependencies. |
| `murmur-shell` | Backend-neutral controller and ratatui presentation shared by both delivery targets: screens, command queue, input modes, rendering. |
| `murmur-native` | Bevy executable rendering into a real terminal via `bevy_ratatui`. |
| `murmur-web` | The same shell compiled to WebAssembly and rendered by Ratzilla; deployed to GitHub Pages by CI. |

Gameplay data — the disguise permission matrix, rooms, roles, items,
names, and every tunable number — is authored RON under [`data/`](data),
embedded at compile time so both targets ship identical data.

## Development

```powershell
cargo test --workspace --exclude murmur-web    # unit, scenario, and playthrough tests
cargo clippy --workspace --exclude murmur-web --all-targets -- -D warnings
cargo clippy -p murmur-web --target wasm32-unknown-unknown -- -D warnings
cargo fmt --all
cargo run -p murmur-core --example dump_map -- 42       # print a generated club
cargo run -p murmur-shell --example screenshot -- 42    # render screens headlessly
```

CI runs the same checks plus the PASM model validation on every push; a
green default branch builds the Trunk site and deploys it to GitHub
Pages. Failed checks never deploy.

## Architecture model

PASM (Project Architecture & System Model) keeps the system boundaries,
design rules, decision records, and implementation evidence explicit as
the project grows. The authored model lives in `pasm/spec/`; design
decisions made during implementation are recorded in
`pasm/spec/core/decisions.yaml`.

```powershell
uv sync --group dev
uv run pasm validate          # schema, references, mapping checks
uv run pasm scan --json       # observe implementation evidence in the repo
uv run pasm query entity turn-driver
```

The design foundation itself is documented in
[docs/architecture/foundation.md](docs/architecture/foundation.md).

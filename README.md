# Project Murmur

A turn-based ASCII social-stealth contract campaign. Take a job from the
syndicate desk, buy your kit, slip into a generated venue — a heaving
nightclub, a bonded warehouse, a layered hotel, an embassy villa, a
working port — and eliminate a *protected* target under the contract's
one hard condition. The mark walks a daily cycle with a bodyguard
detail: surrounded in public, alone only during private beats behind
locked doors. Shoot into the escort and a bodyguard takes the round;
wait for the window, engineer it with the staff paging desk, or rig an
accident that does not care who is standing guard. Then walk out
unremarked. Arrest costs your carried kit and a fine; death ends the
campaign.

**Play in the browser:** <https://jkeywo.github.io/project-murmur/>

**Play natively** (any terminal, Windows/macOS/Linux):

```powershell
cargo run --release -p murmur-native            # resumes your campaign
cargo run --release -p murmur-native -- --seed 42
```

## How it plays

Commands execute one per turn against an authoritative, deterministic
simulation (input runs ahead through an internal 32-slot queue). A command
that cannot be submitted is rejected without time passing and cancels the
queued remainder; a valid action can still fail during simultaneous
resolution, and that turn stands. The mouse works everywhere: hover
inspects any tile (hovering a person shows exactly what they can see),
and clicking a sidebar action equals pressing its key. `j` re-reads the
contract, `<` and `>` page the map through the building's storeys, and
`C` opens the developer switches (reveal the map, blind the NPCs,
invulnerability, endless ammo) — those are commands like any other, so
they cost a turn, mark the run, and replay exactly. Hold `z` to wait
until something changes — a message, damage, or the target's protection
flipping, which the sidebar tracks as standing intel. NPCs run generated
routines on a relaxed cadence, see through facing cones, grow suspicious
of trespass, crouching, drawn weapons, and bodies, and propagate alerts
by line of sight only. The target's detail trails it through the crowd,
posts up outside rooms it takes private calls in, and comes looking if
the call runs long.

Every mission derives from its contract's seed: layout, population,
schedules, items, opportunity machines, and tie-breaker randomness.
Replaying the accepted commands against the config reproduces the
identical result, turn by turn. Before a mission ships, the generation
planner certifies it is completable three ways — social stealth,
physical stealth, and violence — plus one route that satisfies the
contract's condition with your actual loadout.

## The campaign

Contracts on the board name a venue, a district, a payout, and exactly
one condition (no gunfire, no collateral, no bodies found, kill in
private, or a dictated exit); breaking it forfeits the pay. Cash buys
equipment — lockpicks, noisemakers, a forged staff pass, a counterfeit
invitation, a silenced pistol, a garrote — owned until an arrest
confiscates what you carried. Crimes that witnesses observe heat up the
mission (wary guards, backup at the door) and hot missions raise the
district's persistent heat: more guards next time, cooling only while
you work elsewhere. The campaign autosaves between contracts (a file
natively, localStorage in the browser).

## Workspace

| Crate | Role |
| --- | --- |
| `murmur-core` | Deterministic simulation: world, venue graph grammar, route planner, actions, AI, perception, opportunities, heat, turn driver, replay. No engine or platform dependencies. |
| `murmur-campaign` | The layer above missions: contract board, economy, district heat ledger, resolution rules, and the versioned single-slot save. |
| `murmur-shell` | Backend-neutral controller and ratatui presentation shared by both delivery targets: hub and mission screens, command queue, input modes, rendering. |
| `murmur-native` | Bevy executable rendering into a real terminal via `bevy_ratatui`. |
| `murmur-web` | The same shell compiled to WebAssembly and rendered by Ratzilla; deployed to GitHub Pages by CI. |

Gameplay data — venues, rooms, the disguise permission matrix, items,
the equipment catalogue, the five opportunity machines, campaign
economy, and every tunable number — is authored RON under
[`data/`](data), embedded at compile time so both targets ship
identical data. A new venue is one data entry; no venue-specific code
exists.

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

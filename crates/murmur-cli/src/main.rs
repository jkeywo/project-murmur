//! Headless developer CLI.
//!
//! Three jobs, all of them things the game itself cannot do while a person is
//! watching it: inspect one generated mission, drive one mission to an
//! outcome, and stress a whole corpus of seeds.
//!
//! The corpus command is the one that earns its keep in CI. Generation already
//! refuses to hand back a mission it cannot prove completable — the planner
//! certifies three route classes before `generate` returns — but a proof and a
//! playable mission are different claims, and only the second one is what a
//! player buys. `corpus` checks both: every seed must generate, and every
//! generated mission must fall to the autoplayer. A seed that certifies and
//! then cannot be finished is exactly the bug this exists to catch.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use murmur_core::autoplay::autoplay;
use murmur_core::contract::MissionConfig;
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::replay::world_fingerprint;
use murmur_core::turn::TurnDriver;

#[derive(Parser)]
#[command(name = "murmur", about = "Headless tools for Project Murmur.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate one mission and describe what came out.
    Generate {
        #[arg(long, default_value_t = 0)]
        seed: u64,
        #[arg(long, default_value = "nightclub")]
        venue: String,
    },
    /// Drive one mission to an outcome with the autoplayer.
    Play {
        #[arg(long, default_value_t = 0)]
        seed: u64,
        #[arg(long, default_value = "nightclub")]
        venue: String,
        /// Print every command the bot played.
        #[arg(long)]
        commands: bool,
    },
    /// Generate and play a corpus of seeds across every venue.
    Corpus {
        #[arg(long, default_value_t = 64)]
        count: u64,
        /// Restrict to one venue instead of sweeping all of them.
        #[arg(long)]
        venue: Option<String>,
        /// Fail if the corpus takes longer than this many seconds.
        #[arg(long)]
        budget_seconds: Option<u64>,
        /// Fail if fewer than this many per-thousand missions are won.
        ///
        /// The default is a regression gate, not a target. The bot measures
        /// around 593 permille over 960 missions since the getaway fix (a
        /// clean kill is worth nothing if the bot loiters over the body
        /// until the detail arrives — up from 536); 500 leaves room for the
        /// variance between one seed range and another (a 320-seed sample
        /// runs twenty-odd permille off the full figure) while still
        /// catching a venue that has stopped being finishable. Raise it as
        /// the bot improves — never lower it to make a red run green.
        #[arg(long, default_value_t = 500)]
        min_win_permille: u32,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let data = match GameData::embedded() {
        Ok(data) => data,
        Err(error) => {
            eprintln!("content failed to load: {} ({})", error.message, error.file);
            return ExitCode::FAILURE;
        }
    };

    match cli.command {
        Command::Generate { seed, venue } => generate_one(&data, seed, &venue),
        Command::Play {
            seed,
            venue,
            commands,
        } => play_one(&data, seed, &venue, commands),
        Command::Corpus {
            count,
            venue,
            budget_seconds,
            min_win_permille,
        } => corpus(
            &data,
            count,
            venue.as_deref(),
            budget_seconds,
            min_win_permille,
        ),
    }
}

fn generate_one(data: &GameData, seed: u64, venue: &str) -> ExitCode {
    let config = MissionConfig::new(seed, venue);
    let world = match generate(data, &config) {
        Ok(world) => world,
        Err(error) => {
            eprintln!("seed {seed} at {venue}: {error:?}");
            return ExitCode::FAILURE;
        }
    };
    println!("seed: {seed}");
    println!("venue: {venue}");
    println!("target: {}", world.facts.target_name);
    println!("actors: {}", world.actors.len());
    println!("rooms: {}", world.rooms.len());
    println!("floors: {}", world.map.floor_count());
    println!("extraction tiles: {}", world.extraction_tiles.len());
    println!("player wears: {}", world.player_actor().worn_disguise);
    let target_pos = world.actor(world.target).pos;
    match world.room_at(target_pos) {
        Some(room) => println!(
            "target room: {} (zone {:?}), legal for player: {}",
            room.template,
            room.zone,
            murmur_core::access::verdict_for_pos(&world, data, world.player, target_pos)
                .is_allowed()
        ),
        None => println!("target room: circulation space"),
    }
    for furniture in &world.furniture {
        if let Some(disguise) = &furniture.disguise {
            let zones = data
                .disguise(disguise)
                .map(|spec| format!("{:?}", spec.zones))
                .unwrap_or_default();
            println!("wardrobe: {disguise} grants {zones}");
        }
    }
    for proof in &world.routes.proofs {
        println!(
            "route {}: kill in {}, exit via {} ({} steps)",
            proof.class.name(),
            proof.kill_room,
            proof.exit_room,
            proof.steps.len()
        );
    }
    println!("fingerprint: {:016x}", world_fingerprint(&world));
    ExitCode::SUCCESS
}

fn play_one(data: &GameData, seed: u64, venue: &str, show_commands: bool) -> ExitCode {
    let config = MissionConfig::new(seed, venue);
    let world = match generate(data, &config) {
        Ok(world) => world,
        Err(error) => {
            eprintln!("seed {seed} at {venue}: {error:?}");
            return ExitCode::FAILURE;
        }
    };
    let mut driver = TurnDriver::new(world, data);
    let report = autoplay(data, &mut driver);

    println!("seed: {seed}");
    println!("venue: {venue}");
    println!("outcome: {:?}", report.outcome);
    println!("turns: {}", report.turns);
    println!("commands: {}", report.commands.len());
    println!("stalled: {}", report.stalled);
    println!("heat: {}", driver.world().mission_heat);
    println!("fingerprint: {:016x}", world_fingerprint(driver.world()));
    if show_commands {
        for (index, command) in report.commands.iter().enumerate() {
            println!("  {index:4}: {command:?}");
        }
    }
    if report.won() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn corpus(
    data: &GameData,
    count: u64,
    venue: Option<&str>,
    budget_seconds: Option<u64>,
    min_win_permille: u32,
) -> ExitCode {
    let venues: Vec<String> = match venue {
        Some(one) => vec![one.to_owned()],
        None => data.venues.iter().map(|spec| spec.id.clone()).collect(),
    };

    let started = Instant::now();
    let mut generation_failures = Vec::new();
    let mut losses: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut played = 0u32;
    let mut won = 0u32;
    let mut tally: BTreeMap<String, u32> = BTreeMap::new();

    for venue in &venues {
        for seed in 0..count {
            let config = MissionConfig::new(seed, venue.clone());
            let world = match generate(data, &config) {
                Ok(world) => world,
                Err(error) => {
                    generation_failures.push(format!("{venue} seed {seed}: {error:?}"));
                    continue;
                }
            };
            let mut driver = TurnDriver::new(world, data);
            let report = autoplay(data, &mut driver);
            played += 1;
            let label = match (&report.outcome, report.stalled) {
                (Some(outcome), _) => format!("{outcome:?}"),
                (None, true) => "stalled".to_owned(),
                (None, false) => "unfinished".to_owned(),
            };
            *tally.entry(label).or_insert(0) += 1;
            if report.won() {
                won += 1;
            } else {
                losses.entry(venue.clone()).or_default().push(format!(
                    "seed {seed}: {:?} after {} turns{}",
                    report.outcome,
                    report.turns,
                    if report.stalled { " (stalled)" } else { "" }
                ));
            }
        }
    }

    let elapsed = started.elapsed();
    let win_permille = (won * 1000).checked_div(played).unwrap_or(0);
    println!(
        "corpus: {played} missions across {} venues in {:.1}s, {} generation failures, \
         {won} won ({win_permille} permille)",
        venues.len(),
        elapsed.as_secs_f64(),
        generation_failures.len(),
    );
    for (outcome, count) in &tally {
        let permille = (count * 1000).checked_div(played).unwrap_or(0);
        println!("  {outcome}: {count} ({permille} permille)");
    }
    for failure in &generation_failures {
        println!("  generate: {failure}");
    }
    for (venue, seeds) in &losses {
        println!("  {venue}: {} unwon", seeds.len());
        for line in seeds.iter().take(10) {
            println!("    {line}");
        }
        if seeds.len() > 10 {
            println!("    ... and {} more", seeds.len() - 10);
        }
    }

    let mut failed = false;
    if !generation_failures.is_empty() {
        eprintln!(
            "{} corpus seeds failed to generate",
            generation_failures.len()
        );
        failed = true;
    }
    if win_permille < min_win_permille {
        eprintln!(
            "corpus won {win_permille} permille against a floor of {min_win_permille}: \
             a mission the planner certified is not finishable in play"
        );
        failed = true;
    }
    if let Some(budget) = budget_seconds
        && elapsed.as_secs() > budget
    {
        eprintln!(
            "corpus took {:.1}s, over the {budget}s budget",
            elapsed.as_secs_f64()
        );
        failed = true;
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

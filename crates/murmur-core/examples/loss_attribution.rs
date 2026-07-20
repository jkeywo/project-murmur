//! Developer tool: where do the autoplayer's losses come from?
//!
//! Plays a corpus and, for every mission that is not won, records what the
//! bot was doing in the turns before it lost — was it firing on an escorted
//! target, caught trespassing, chasing across the venue, or run out of
//! clock. Categorises before any fix, so the fixes are aimed rather than
//! guessed at.
//!
//! ```text
//! cargo run --release -p murmur-core --example loss_attribution
//! ```

use std::collections::BTreeMap;

use murmur_core::actions::Command;
use murmur_core::autoplay::autoplay;
use murmur_core::contract::MissionConfig;
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::turn::TurnDriver;
use murmur_core::world::MissionOutcome;

const SEEDS: u64 = 64;

fn main() {
    let data = GameData::embedded().unwrap();
    let venues = [
        "nightclub",
        "warehouse",
        "grand-hotel",
        "embassy-villa",
        "port-authority",
    ];

    let mut outcomes: BTreeMap<&str, u32> = BTreeMap::new();
    // For deaths and arrests: what the last meaningful command was.
    let mut loss_kind: BTreeMap<&str, u32> = BTreeMap::new();
    let mut total = 0u32;

    for venue in venues {
        for seed in 0..SEEDS {
            let world = generate(&data, &MissionConfig::new(seed, venue)).unwrap();
            let target = world.target;
            let mut driver = TurnDriver::new(world, &data);
            let report = autoplay(&data, &mut driver);
            total += 1;

            let label = match &report.outcome {
                Some(MissionOutcome::Extracted) => "extracted",
                Some(MissionOutcome::PlayerKilled) => "killed",
                Some(MissionOutcome::Arrested) => "arrested",
                Some(MissionOutcome::TargetEscaped) => "target-escaped",
                None => "stalled",
            };
            *outcomes.entry(label).or_default() += 1;

            let won = report.outcome == Some(MissionOutcome::Extracted);
            if won {
                continue;
            }

            // Attribute the loss. Look at the tail of the command log and
            // the final world.
            let world = driver.world();
            let target_dead = !world.actor(target).alive();
            let tail: Vec<&Command> = report.commands.iter().rev().take(12).collect();
            let fired = tail
                .iter()
                .any(|c| matches!(c, Command::Shoot(_) | Command::DrawOrHolster));
            let garrotted = tail.iter().any(|c| matches!(c, Command::Garrote(_)));

            // For a lost getaway, split by how the kill was made and how the
            // getaway ended: a loud kill and a silent kill are different
            // problems, and dying is a different problem from arrest.
            let killed_by_shot = report
                .commands
                .iter()
                .any(|c| matches!(c, Command::Shoot(_)));
            let kind = if label == "stalled" {
                "stall"
            } else if target_dead {
                match (killed_by_shot, label) {
                    (true, "killed") => "getaway: shot-kill, then died",
                    (true, _) => "getaway: shot-kill, then arrested",
                    (false, "killed") => "getaway: quiet-kill, then died",
                    (false, _) => "getaway: quiet-kill, then arrested",
                }
            } else if fired {
                // Went loud on a live target: the escalation.
                "shot-a-live-target"
            } else if garrotted {
                // Tried the wire and it went wrong — seen, or not behind.
                "garrote-gone-wrong"
            } else {
                // Died or arrested with no attack in the tail: caught while
                // moving — trespass, or a disguise that did not hold.
                "caught-moving"
            };
            *loss_kind.entry(kind).or_default() += 1;
        }
    }

    let permille = |n: u32| n * 1000 / total;
    println!(
        "loss attribution, {total} missions across {} venues",
        venues.len()
    );
    println!("  outcomes:");
    for (label, n) in &outcomes {
        println!("    {label:<16} {n:>4}  ({} permille)", permille(*n));
    }
    println!("  losses by what the bot was doing:");
    for (kind, n) in &loss_kind {
        println!("    {kind:<20} {n:>4}  ({} permille)", permille(*n));
    }
}

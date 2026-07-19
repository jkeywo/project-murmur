//! Developer tool: how long does a world take to generate and simulate?
use murmur_core::actions::Command;
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::turn::TurnDriver;
use std::time::Instant;

fn main() {
    let data = GameData::embedded().unwrap();
    for venue in [
        "nightclub",
        "warehouse",
        "grand-hotel",
        "embassy-villa",
        "port-authority",
    ] {
        let t0 = Instant::now();
        let mut worlds = Vec::new();
        for seed in 0..40u64 {
            worlds.push(
                generate(
                    &data,
                    &murmur_core::contract::MissionConfig::new(seed, venue),
                )
                .unwrap(),
            );
        }
        let gen_time = t0.elapsed();
        let world = worlds.pop().unwrap();
        let mut driver = TurnDriver::new(world, &data);
        let t1 = Instant::now();
        for _ in 0..600 {
            if driver.mission_over() {
                break;
            }
            if driver.player_busy() {
                driver.continue_busy(&data);
                continue;
            }
            let _ = driver.submit(&data, &Command::Wait);
        }
        let sim = t1.elapsed();
        println!(
            "{venue}: 40 worlds in {:?} ({:?}/world) | 600 turns in {:?} ({:?}/turn)",
            gen_time,
            gen_time / 40,
            sim,
            sim / 600
        );
    }
}

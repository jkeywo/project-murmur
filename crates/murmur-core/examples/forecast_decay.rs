//! Developer tool: how long does a forecast stay true?
//!
//! Takes a forecast at a point mid-mission, then lets the real bot play on
//! and compares. The forecast assumes the player stands still, so every
//! error is the player's own influence on the venue — which is exactly the
//! error a planner would have to tolerate, since a planner cannot know
//! what it will decide to do before it decides it.
//!
//! ```text
//! cargo run --release -p murmur-core --example forecast_decay
//! ```

use murmur_core::actions::Command;
use murmur_core::autoplay::forecast::Forecast;
use murmur_core::contract::MissionConfig;
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::turn::TurnDriver;
use murmur_core::world::ActorId;

/// Offsets reported, in turns.
const PROBES: [u32; 6] = [1, 5, 10, 20, 40, 80];
const HORIZON: u32 = 80;
/// Turn at which the forecast is taken: far enough in that the venue is
/// live and the bot has committed to something.
const FORECAST_AT: u32 = 60;

fn main() {
    let data = GameData::embedded().unwrap();
    // hits[i] / seen[i] = share of actors still where the forecast said,
    // at PROBES[i] turns out.
    let mut hits = [0u32; PROBES.len()];
    let mut seen = [0u32; PROBES.len()];
    // How far wrong, in tiles, when wrong.
    let mut drift = [0u64; PROBES.len()];
    // Within-tolerance: a route planner asks "will this corridor be
    // watched", not "is guard seven on tile twelve, four". Exact-match
    // understates how useful a forecast is for pricing a route.
    let mut near = [0u32; PROBES.len()];
    const TOLERANCE: i16 = 3;

    for venue in [
        "nightclub",
        "warehouse",
        "grand-hotel",
        "embassy-villa",
        "port-authority",
    ] {
        for seed in 0..24u64 {
            let world = generate(&data, &MissionConfig::new(seed, venue)).unwrap();
            let mut driver = TurnDriver::new(world, &data);

            // Let the venue get going, with the player idle.
            for _ in 0..FORECAST_AT {
                if driver.mission_over() {
                    break;
                }
                if driver.player_busy() {
                    driver.continue_busy(&data);
                } else if driver.submit(&data, &Command::Wait).is_err() {
                    break;
                }
            }
            if driver.mission_over() {
                continue;
            }

            let forecast = Forecast::read(driver.world(), &data, HORIZON);
            let tracked: Vec<ActorId> = driver
                .world()
                .actors
                .iter()
                .filter(|a| !a.is_player() && a.alive() && !a.departed)
                .map(|a| a.id)
                .collect();

            // Now let the real bot play, which is what makes this a test of
            // the forecast rather than of determinism.
            let mut player = murmur_core::autoplay::Autoplayer::new();
            let mut elapsed = 0u32;
            let mut probe = 0usize;
            while probe < PROBES.len() {
                if driver.mission_over() {
                    break;
                }
                let before = driver.world().turn;
                player.step(&data, &mut driver);
                elapsed += driver.world().turn.saturating_sub(before);
                while probe < PROBES.len() && elapsed >= PROBES[probe] {
                    let offset = PROBES[probe];
                    for id in &tracked {
                        let Some(predicted) = forecast.position(*id, offset) else {
                            continue;
                        };
                        let actual = driver.world().actor(*id).pos;
                        seen[probe] += 1;
                        let apart = actual.chebyshev(predicted);
                        if apart.is_some_and(|d| d <= TOLERANCE) {
                            near[probe] += 1;
                        }
                        if actual == predicted {
                            hits[probe] += 1;
                        } else {
                            drift[probe] +=
                                u64::from(actual.chebyshev(predicted).unwrap_or(99).unsigned_abs());
                        }
                    }
                    probe += 1;
                }
            }
        }
    }

    println!("forecast accuracy, 120 missions across 5 venues");
    println!("  turns  exact  within 3  mean drift when wrong");
    for (i, offset) in PROBES.iter().enumerate() {
        if seen[i] == 0 {
            continue;
        }
        let share = f64::from(hits[i]) * 100.0 / f64::from(seen[i]);
        let misses = seen[i] - hits[i];
        let mean = if misses == 0 {
            0.0
        } else {
            drift[i] as f64 / f64::from(misses)
        };
        let close = f64::from(near[i]) * 100.0 / f64::from(seen[i]);
        println!("  {offset:>5}  {share:>5.1}%  {close:>7.1}%  {mean:>5.1} tiles");
    }
}

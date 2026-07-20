//! Probe: are extraction tiles ever adjacent to (or on) a doorway?
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::geom::Dir4;
use murmur_core::map::TileKind;

fn main() {
    let data = GameData::embedded().unwrap();
    let mut adjacent = 0;
    let mut on_door = 0;
    let mut total = 0;
    for venue in [
        "nightclub",
        "warehouse",
        "grand-hotel",
        "embassy-villa",
        "port-authority",
    ] {
        for seed in 0..40u64 {
            let w = generate(
                &data,
                &murmur_core::contract::MissionConfig::new(seed, venue),
            )
            .unwrap();
            for tile in &w.extraction_tiles {
                total += 1;
                if matches!(w.map.tile(*tile), TileKind::Door(_)) {
                    on_door += 1;
                    println!("{venue} seed {seed}: extraction ON a door at {tile:?}");
                }
                for d in Dir4::ALL {
                    if matches!(w.map.tile(tile.step(d)), TileKind::Door(_)) {
                        adjacent += 1;
                        if adjacent <= 6 {
                            println!(
                                "{venue} seed {seed}: extraction {tile:?} sits against door {:?}",
                                tile.step(d)
                            );
                        }
                        break;
                    }
                }
            }
        }
    }
    println!("total {total}, on a door {on_door}, against a door {adjacent}");
}

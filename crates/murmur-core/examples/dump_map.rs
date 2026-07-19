//! Developer tool: print a generated nightclub as ASCII.
//!
//! ```text
//! cargo run -p murmur-core --example dump_map -- 42
//! ```

use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::geom::Pos;
use murmur_core::map::TileKind;
use murmur_core::world::FurnitureKind;

fn main() {
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(42);
    let data = GameData::embedded().expect("embedded data");
    let world = generate(
        &data,
        &murmur_core::contract::MissionConfig::new(seed, "nightclub"),
    )
    .expect("generation");

    println!("seed {seed}");
    println!(
        "target: {} ({}), reason: {}",
        world.facts.target_name,
        world.facts.target_locations.join(", "),
        world.facts.target_reason
    );
    println!(
        "guards {} staff {} civilians {} containers {}",
        world.facts.guard_count,
        world.facts.staff_count,
        world.facts.civilian_count,
        world.facts.container_count
    );
    println!("proof: {:?}", world.proof);

    for floor in 0..world.map.floor_count() {
        println!("--- floor {floor} ---");
        for y in 0..world.map.height() as i16 {
            let mut row = String::new();
            for x in 0..world.map.width() as i16 {
                let pos = Pos::new(floor, x, y);
                let ch = if let Some(actor) = world.standing_actor_at(pos) {
                    if actor.is_player() {
                        '@'
                    } else if actor.is_target {
                        'T'
                    } else {
                        actor
                            .role
                            .and_then(|r| data.role_spec(r))
                            .map(|s| s.glyph)
                            .unwrap_or('?')
                    }
                } else if let Some(f) = world.furniture_at(pos) {
                    match f.kind {
                        FurnitureKind::LowCover => '=',
                        FurnitureKind::Container => 'O',
                        FurnitureKind::Wardrobe => 'W',
                        FurnitureKind::Machine => '&',
                    }
                } else if world.extraction_tiles.contains(&pos) {
                    'X'
                } else {
                    match world.map.tile(pos) {
                        TileKind::Void => ' ',
                        TileKind::Wall => '#',
                        TileKind::Floor => '.',
                        TileKind::Door(id) => {
                            if world.door(id).locked_by.is_some() {
                                '*'
                            } else {
                                '+'
                            }
                        }
                        TileKind::Stairs(_) => '<',
                    }
                };
                row.push(ch);
            }
            println!("{row}");
        }
        for room in world.rooms.iter().filter(|r| r.floor == floor) {
            println!(
                "  {} [{}] {}x{} at ({},{})",
                room.name,
                room.zone.name(),
                room.bounds.w,
                room.bounds.h,
                room.bounds.x,
                room.bounds.y
            );
        }
    }
}

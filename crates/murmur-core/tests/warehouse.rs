//! The second venue: the bonded warehouse must run on exactly the same
//! contract, planner, grammar, opportunity, heat, and campaign systems
//! as the nightclub — venue-driven architecture, not nightclub-specific.

use murmur_core::contract::{Constraint, MissionConfig};
use murmur_core::data::{GameData, Role, Zone};
use murmur_core::generator::generate;
use murmur_core::planner::RouteClass;

fn data() -> GameData {
    GameData::embedded().unwrap()
}

#[test]
fn warehouses_generate_with_all_route_proofs_across_seeds() {
    let data = data();
    for seed in 0..20u64 {
        let world = generate(&data, &MissionConfig::new(seed, "warehouse"))
            .unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        // The same certification bar as the nightclub.
        for class in [
            RouteClass::Social,
            RouteClass::Physical,
            RouteClass::Violence,
        ] {
            assert!(world.routes.class(class).is_some(), "seed {seed}");
        }
        assert!(world.routes.loadout_proof.is_some());
        // Its own gradient: every tier is present somewhere.
        for zone in [Zone::Public, Zone::Staff, Zone::Secure, Zone::Personal] {
            assert!(
                world.rooms.iter().any(|r| r.zone == zone),
                "seed {seed}: no {zone:?} room"
            );
        }
        // The venue rides on the world for presentation flavour.
        assert_eq!(world.venue, "warehouse");
    }
}

#[test]
fn the_warehouse_crowd_contrasts_with_the_nightclub() {
    let data = data();
    let club = generate(&data, &MissionConfig::new(5, "nightclub")).unwrap();
    let warehouse = generate(&data, &MissionConfig::new(5, "warehouse")).unwrap();
    let civilians = |world: &murmur_core::world::World| {
        world
            .actors
            .iter()
            .filter(|a| a.role == Some(Role::Civilian))
            .count()
    };
    assert!(
        civilians(&warehouse) < civilians(&club),
        "the warehouse floor is sparse: {} vs {}",
        civilians(&warehouse),
        civilians(&club)
    );
}

#[test]
fn warehouse_contracts_support_every_constraint() {
    let data = data();
    let constraints = [
        Constraint::NoFirearms,
        Constraint::NoCivilianCasualties,
        Constraint::NoBodiesFound,
        Constraint::PrivateKill,
        Constraint::SpecificExit {
            room_template: "dock".to_string(),
        },
    ];
    for constraint in constraints {
        let mut proven = false;
        for seed in 0..8u64 {
            let config = MissionConfig::new(seed, "warehouse").with_constraint(constraint.clone());
            if let Ok(world) = generate(&data, &config) {
                assert!(world.routes.constraint_proof.is_some());
                proven = true;
                break;
            }
        }
        assert!(proven, "{constraint:?} never certified in the warehouse");
    }
}

#[test]
fn machines_place_in_the_warehouse_too() {
    let data = data();
    let mut any = 0usize;
    for seed in 0..8u64 {
        let world = generate(&data, &MissionConfig::new(seed, "warehouse")).unwrap();
        any += world
            .furniture
            .iter()
            .filter(|f| f.machine.is_some())
            .count();
    }
    assert!(any > 0, "opportunity machines are venue-agnostic");
}

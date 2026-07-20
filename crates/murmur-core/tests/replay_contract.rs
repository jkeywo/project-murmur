//! Murmur against the shared replay contract.
//!
//! The suites next door assert this game's rules. These assert the properties
//! every replay-as-save game needs and that are easy to lose in a refactor
//! without any rule visibly breaking — checked by `vellum-replay` so that both
//! games are held to the same statement of them.
//!
//! The one worth having is [`rejection_is_pure`]: it compares the mission
//! *fingerprint* either side of a refused command, so a rejection that quietly
//! consumed a tie-breaker draw would fail here even though nothing observable
//! moved. Murmur documents that rejection is free; this is what checks it.

mod common;

use murmur_core::actions::Command;
use murmur_core::contract::MissionConfig;
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::geom::Dir4;
use murmur_core::replay::MissionSim;
use vellum_replay::contract;

fn data() -> GameData {
    GameData::embedded().expect("embedded content")
}

/// A short script of commands that apply from a mission's opening position.
/// Waiting always applies, so the script is about exercising the driver rather
/// than about clever play; the golden suite covers real missions.
fn script() -> Vec<Command> {
    vec![
        Command::Wait,
        Command::ToggleCrouch,
        Command::Wait,
        Command::ToggleCrouch,
    ]
}

#[test]
fn murmur_keeps_the_replay_contract() {
    let data = data();
    let config = MissionConfig::new(5, "nightclub");
    let new = || {
        let world = generate(&data, &config).expect("mission generates");
        MissionSim::new(&data, world)
    };

    // Garroting an actor the player is nowhere near is refused for a reason
    // that cannot change with the seed: you must be directly behind them.
    let rejected = Command::Garrote(murmur_core::world::ActorId(1));

    contract::check_all(new, &script(), &rejected);
}

/// The same, on a venue with storeys and locked doors, so the contract is not
/// only checked against the simplest layout.
#[test]
fn the_contract_holds_on_a_multi_storey_venue() {
    let data = data();
    let config = MissionConfig::new(2, "grand-hotel");
    let new = || {
        let world = generate(&data, &config).expect("mission generates");
        MissionSim::new(&data, world)
    };
    // Moving is legal somewhere, so the refusal used here is one that holds
    // regardless of where the player starts.
    let rejected = Command::HideBody(murmur_core::world::FurnitureId(0));
    contract::check_all(new, &script(), &rejected);
}

/// Stepping into a wall is the everyday refusal, and it must be free too.
#[test]
fn walking_into_a_wall_costs_nothing() {
    let data = data();
    let config = MissionConfig::new(5, "nightclub");
    let new = || {
        let world = generate(&data, &config).expect("mission generates");
        MissionSim::new(&data, world)
    };
    // At least one of the four directions is blocked from any starting tile
    // in a built venue; whichever it is, the check demands a genuine refusal.
    let world = generate(&data, &config).expect("mission generates");
    let start = world.player_actor().pos;
    let blocked = Dir4::ALL
        .into_iter()
        .find(|dir| world.blocks_move(start.step(*dir)))
        .expect("the player starts somewhere with at least one wall beside them");

    contract::rejection_is_pure(new, &[], &Command::Move(blocked));
}

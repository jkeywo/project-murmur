//! Missions, pasteable.
//!
//! A mission was always reproducible from its [`MissionRecord`] — that is what
//! the replay contract means. What it was not, until the shared engine layer
//! brought the format over from the other game, was *portable*: there was no
//! way to hand someone a mission except by sending them a file.

use murmur_core::actions::Command;
use murmur_core::autoplay::autoplay;
use murmur_core::contract::MissionConfig;
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::geom::Dir4;
use murmur_core::replay::{MissionRecord, replay, world_fingerprint};
use murmur_core::turn::TurnDriver;

#[test]
fn a_real_mission_survives_the_round_trip() {
    let data = GameData::embedded().expect("embedded content");
    let config = MissionConfig::new(3, "nightclub");
    let world = generate(&data, &config).expect("mission generates");
    let mut driver = TurnDriver::new(world, &data);
    let report = autoplay(&data, &mut driver);
    let played = world_fingerprint(driver.world());

    let record = MissionRecord {
        config,
        commands: report.commands,
    };
    let code = record.share_code();
    assert!(code.starts_with("MUR1-"), "unexpected code: {code}");

    let back = MissionRecord::from_share_code(&code).expect("the code decodes");
    assert_eq!(back, record, "the code did not carry the whole record");

    // And the decoded record must still reproduce the run, not merely compare
    // equal — the point of a share code is the mission on the other end of it.
    let replayed = replay(&data, &back).expect("the decoded mission replays");
    assert_eq!(
        world_fingerprint(&replayed),
        played,
        "the shared mission played out differently"
    );
}

#[test]
fn a_mistyped_code_is_refused_rather_than_misread() {
    let record = MissionRecord {
        config: MissionConfig::new(11, "grand-hotel"),
        commands: vec![Command::Move(Dir4::North), Command::Wait],
    };
    let code = record.share_code();

    let mut broken: Vec<char> = code.chars().collect();
    let index = code.len() - 3;
    broken[index] = if broken[index] == 'A' { 'B' } else { 'A' };
    let broken: String = broken.into_iter().collect();
    assert_ne!(broken, code, "the test must actually change the code");

    assert!(
        MissionRecord::from_share_code(&broken).is_err(),
        "a corrupted code was accepted, so the checksum is not doing its job"
    );
}

#[test]
fn another_games_code_is_not_a_murmur_mission() {
    // rogue-hunter's codes carry an RH1- prefix. Feeding one here must fail
    // cleanly rather than decode into a plausible-looking mission.
    assert!(MissionRecord::from_share_code("RH1-AAAAAAAA").is_err());
}

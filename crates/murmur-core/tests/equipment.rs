//! Equipment scenarios: the loadout gates capability, and every
//! catalogue item creates a concrete one — lockpicks defeat locks,
//! noisemakers lure investigators, the forged pass and counterfeit
//! invitation legitimise access, and the weapons gate their kills.

use murmur_core::access::{AccessVerdict, verdict_for_pos};
use murmur_core::actions::{Command, RejectReason};
use murmur_core::contract::MissionConfig;
use murmur_core::data::{GameData, Role, Zone};
use murmur_core::generator::generate;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::map::TileKind;
use murmur_core::turn::TurnDriver;
use murmur_core::world::{ActorId, Mood, World};

mod common;
use common::{quiet_all_npcs, setup_config};

fn setup_loadout(seed: u64, loadout: &[&str]) -> (GameData, TurnDriver) {
    setup_config(
        MissionConfig::new(seed, "nightclub")
            .with_loadout(loadout.iter().map(|s| s.to_string()).collect()),
    )
}

fn place(world: &mut World, actor: ActorId, pos: Pos) {
    common::place(world, actor, pos, None);
}

#[test]
fn oversized_and_unknown_loadouts_are_rejected() {
    let data = GameData::embedded().unwrap();
    let too_many = MissionConfig::new(1, "nightclub").with_loadout(vec![
        "garrote".into(),
        "lockpicks".into(),
        "noisemaker".into(),
        "forged-pass".into(),
    ]);
    assert!(generate(&data, &too_many).is_err());
    let unknown = MissionConfig::new(1, "nightclub").with_loadout(vec!["rocket-launcher".into()]);
    assert!(generate(&data, &unknown).is_err());
}

#[test]
fn equipment_only_enters_missions_through_the_loadout() {
    let data = GameData::embedded().unwrap();
    let world = generate(&data, &MissionConfig::new(5, "nightclub")).unwrap();
    // Default loadout: garrote and pistol on the player, and no
    // purchasable item generated anywhere else in the venue.
    for item in &world.items {
        let spec = data.item(&item.spec).unwrap();
        if spec.purchasable {
            assert_eq!(
                item.location,
                murmur_core::world::ItemLocation::CarriedBy(world.player),
                "purchasable '{}' generated outside the loadout",
                item.spec
            );
        }
    }
}

#[test]
fn the_garrote_is_required_equipment_for_garrotting() {
    let (data, mut driver) = setup_loadout(7, &["silenced-pistol"]);
    quiet_all_npcs(driver.world_mut());
    let target = driver.world().target;
    assert!(matches!(
        driver.submit(&data, &Command::Garrote(target)),
        Err(RejectReason::NoGarrote)
    ));
}

#[test]
fn lockpicks_open_locked_doors_without_the_key() {
    let (data, mut driver) = setup_loadout(9, &["lockpicks", "garrote"]);
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;

    // Find a locked door and a walkable tile beside it.
    let world = driver.world();
    let (door_id, stand) = world
        .map
        .floor_positions(0)
        .chain(world.map.floor_positions(1))
        .find_map(|pos| {
            let TileKind::Door(id) = world.map.tile(pos) else {
                return None;
            };
            world.door(id).locked_by.as_ref()?;
            Dir4::ALL
                .into_iter()
                .map(|d| pos.step(d))
                .find(|p| {
                    matches!(world.map.tile(*p), TileKind::Floor)
                        && world.standing_actor_at(*p).is_none()
                        && world.furniture_at(*p).is_none()
                })
                .map(|stand| (id, stand))
        })
        .expect("a locked door with a free adjacent tile");

    place(driver.world_mut(), player, stand);
    driver.submit(&data, &Command::PickLock(door_id)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    let door = driver.world().door(door_id);
    assert!(door.locked_by.is_none(), "the lock is defeated for good");
    assert!(door.open, "the picked door swings open");

    // Without lockpicks the same command is rejected outright.
    let (data2, mut driver2) = setup_loadout(9, &["garrote"]);
    quiet_all_npcs(driver2.world_mut());
    let player2 = driver2.world().player;
    place(driver2.world_mut(), player2, stand);
    assert!(matches!(
        driver2.submit(&data2, &Command::PickLock(door_id)),
        Err(RejectReason::NoLockpicks)
    ));
}

#[test]
fn a_thrown_noisemaker_draws_a_guard_to_the_spot() {
    let (data, mut driver) = setup_loadout(11, &["noisemaker", "garrote"]);
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;
    let guard = driver
        .world()
        .actors
        .iter()
        .find(|a| a.role == Some(Role::Guard))
        .map(|a| a.id)
        .expect("a guard exists");

    // Stage the guard close enough to hear but far enough not to watch.
    let spot = driver.world().actor(player).pos;
    let near = Pos::new(spot.floor, spot.x + 4, spot.y);
    place(driver.world_mut(), guard, near);

    driver
        .submit(&data, &Command::ThrowNoisemaker(spot))
        .unwrap();
    let world = driver.world();
    let charge = world
        .carried_items(player)
        .find(|i| i.spec == "noisemaker")
        .unwrap();
    assert_eq!(charge.charges, 1, "one charge spent");
    let guard_ai = world.actor(guard).ai.as_ref().unwrap();
    assert_eq!(guard_ai.mood, Mood::Investigating);
    assert_eq!(guard_ai.focus, Some(spot));

    // Charges run out.
    driver
        .submit(&data, &Command::ThrowNoisemaker(spot))
        .unwrap();
    assert!(matches!(
        driver.submit(&data, &Command::ThrowNoisemaker(spot)),
        Err(RejectReason::NoNoisemaker)
    ));
}

#[test]
fn the_forged_pass_legitimises_staff_space_and_the_counterfeit_secure_space() {
    let (data, mut driver) =
        setup_loadout(13, &["forged-pass", "counterfeit-invitation", "garrote"]);
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;

    let world = driver.world();
    let staff_probe = world
        .rooms
        .iter()
        .find(|r| r.zone == Zone::Staff)
        .map(|r| Pos::new(r.floor, r.bounds.x, r.bounds.y))
        .expect("a staff room");
    let secure_probe = world
        .rooms
        .iter()
        .find(|r| r.zone == Zone::Secure)
        .map(|r| Pos::new(r.floor, r.bounds.x, r.bounds.y))
        .expect("a secure room");

    assert_eq!(
        verdict_for_pos(world, &data, player, staff_probe),
        AccessVerdict::AllowedByPass,
        "the forged pass covers staff space in civilian clothes"
    );
    assert_eq!(
        verdict_for_pos(world, &data, player, secure_probe),
        AccessVerdict::AllowedByInvitation,
        "the counterfeit invitation covers secure space"
    );

    // Without the kit, both are trespass.
    let (data2, driver2) = setup_loadout(13, &["garrote"]);
    let world2 = driver2.world();
    assert!(matches!(
        verdict_for_pos(world2, &data2, world2.player, staff_probe),
        AccessVerdict::Illegal(Zone::Staff)
    ));
    assert!(matches!(
        verdict_for_pos(world2, &data2, world2.player, secure_probe),
        AccessVerdict::Illegal(Zone::Secure)
    ));
}

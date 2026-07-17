//! Access and disguise rules.
//!
//! Access never blocks movement — the simulation lets you walk anywhere
//! you physically can. These rules define *legitimacy*: whether an actor's
//! presence on a tile is lawful given their worn disguise and carried
//! items. Perception consumes them to detect illegal access; the briefing
//! consumes them to report restricted areas.

use crate::data::{GameData, Zone};
use crate::world::{ActorId, World};

/// Why standing on a tile is (il)legal for an actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessVerdict {
    /// Circulation space or a zone the disguise permits.
    Allowed,
    /// Permitted only because a carried invitation covers a VIP zone.
    AllowedByInvitation,
    /// Permitted despite the zone because the disguise grants this
    /// specific room (authored partial access).
    AllowedByRoomGrant,
    /// Permitted because a carried forged staff pass covers staff space.
    AllowedByPass,
    /// The disguise does not cover this room's zone.
    Illegal(Zone),
}

impl AccessVerdict {
    pub fn is_allowed(self) -> bool {
        !matches!(self, AccessVerdict::Illegal(_))
    }
}

/// Evaluates the legitimacy of `actor` standing at its current position.
pub fn verdict_at(world: &World, data: &GameData, actor: ActorId) -> AccessVerdict {
    verdict_for_pos(world, data, actor, world.actor(actor).pos)
}

/// Evaluates the legitimacy of `actor` standing at `pos`.
pub fn verdict_for_pos(
    world: &World,
    data: &GameData,
    actor: ActorId,
    pos: crate::geom::Pos,
) -> AccessVerdict {
    let Some(room) = world.room_at(pos) else {
        // Corridors, stairs, and doorways are public circulation space.
        return AccessVerdict::Allowed;
    };
    let actor_ref = world.actor(actor);
    let Some(disguise) = data.disguise(&actor_ref.worn_disguise) else {
        return AccessVerdict::Illegal(room.zone);
    };
    if disguise.zones.contains(&room.zone) {
        return AccessVerdict::Allowed;
    }
    if disguise.extra_rooms.contains(&room.template) {
        return AccessVerdict::AllowedByRoomGrant;
    }
    if room.zone == Zone::Secure
        && disguise.secure_with_invitation
        && world.carries(actor, data, |spec| spec.invitation)
    {
        return AccessVerdict::AllowedByInvitation;
    }
    if room.zone == Zone::Staff && world.carries(actor, data, |spec| spec.staff_pass) {
        return AccessVerdict::AllowedByPass;
    }
    AccessVerdict::Illegal(room.zone)
}

/// Whether `actor` can pass a locked door. Unlocked doors are open to
/// everyone. The player must carry the key; NPCs whose legitimate access
/// covers the room behind the door are presumed to hold its key (their
/// working keys are not simulated as items — only the ones the player can
/// steal are).
pub fn can_pass_door(
    world: &World,
    data: &GameData,
    actor: ActorId,
    door: crate::map::DoorId,
) -> bool {
    let Some(key) = &world.door(door).locked_by else {
        return true;
    };
    if world.carries(actor, data, |spec| &spec.id == key) {
        return true;
    }
    let actor_ref = world.actor(actor);
    if actor_ref.is_player() {
        return false;
    }
    let Some(room) = world.rooms.iter().find(|r| r.doors.contains(&door)) else {
        return false;
    };
    data.disguise(&actor_ref.worn_disguise).is_some_and(|spec| {
        spec.zones.contains(&room.zone) || spec.extra_rooms.contains(&room.template)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::generate;

    #[test]
    fn civilian_is_legal_in_public_and_illegal_in_private() {
        let data = GameData::embedded().unwrap();
        let world = generate(&data, &crate::contract::MissionConfig::new(11, "nightclub")).unwrap();
        let player = world.player;
        assert_eq!(world.actor(player).worn_disguise, "civilian");

        // Player spawns in the entrance hall: public, allowed.
        assert!(verdict_at(&world, &data, player).is_allowed());

        // Any private room tile is illegal for the civilian disguise.
        let private = world
            .rooms
            .iter()
            .find(|r| r.zone == Zone::Personal)
            .expect("nightclub has private rooms");
        let pos = crate::geom::Pos::new(private.floor, private.bounds.x, private.bounds.y);
        assert_eq!(
            verdict_for_pos(&world, &data, player, pos),
            AccessVerdict::Illegal(Zone::Personal)
        );
    }

    #[test]
    fn invitation_makes_vip_legal_for_civilians() {
        let data = GameData::embedded().unwrap();
        let mut world =
            generate(&data, &crate::contract::MissionConfig::new(11, "nightclub")).unwrap();
        let player = world.player;
        let vip_room = world
            .rooms
            .iter()
            .find(|r| r.zone == Zone::Secure)
            .expect("nightclub has a VIP lounge");
        let pos = crate::geom::Pos::new(vip_room.floor, vip_room.bounds.x, vip_room.bounds.y);

        assert_eq!(
            verdict_for_pos(&world, &data, player, pos),
            AccessVerdict::Illegal(Zone::Secure)
        );

        // Hand the player an invitation: VIP becomes legitimate.
        let invitation = world
            .items
            .iter()
            .position(|i| i.spec == "vip-invitation")
            .expect("VIP invitations exist");
        world.items[invitation].location = crate::world::ItemLocation::CarriedBy(player);
        assert_eq!(
            verdict_for_pos(&world, &data, player, pos),
            AccessVerdict::AllowedByInvitation
        );
    }

    #[test]
    fn guard_and_manager_disguises_are_legal_everywhere() {
        let data = GameData::embedded().unwrap();
        let mut world =
            generate(&data, &crate::contract::MissionConfig::new(11, "nightclub")).unwrap();
        let player = world.player;
        for disguise in ["guard", "manager"] {
            world.actor_mut(player).worn_disguise = disguise.to_string();
            for room in world.rooms.clone() {
                let pos = crate::geom::Pos::new(room.floor, room.bounds.x, room.bounds.y);
                assert!(
                    verdict_for_pos(&world, &data, player, pos).is_allowed(),
                    "{disguise} should access {}",
                    room.name
                );
            }
        }
    }
}

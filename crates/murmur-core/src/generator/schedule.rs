//! The target's day, as beats.
//!
//! A schedule is a cycle. The target walks it forever: public beats where
//! it is escorted and effectively unattackable, private beats where it
//! steps away alone and a weapon will work. Difficulty stops being "can
//! you reach the target" — the planner already guarantees you can — and
//! becomes "can you be there when it is alone".
//!
//! Two properties are load-bearing and both are checked here rather than
//! discovered later:
//!
//! * **At least one alone beat, behind at most a lock.** Without it the
//!   mission is unwinnable by weapon, and the route planner would happily
//!   certify it anyway because reachability says nothing about protection.
//! * **The cycle recurs.** Every beat generated here is
//!   [`BeatTrigger::Sequential`], so a beat that exists is visited every
//!   cycle, forever. That is what lets the atemporal planner treat a
//!   reachable alone beat as one that eventually arrives.
//!
//! Dwells are *fitted*, not authored: generation measures the walking
//! between beats and spends the remainder of the cycle budget standing
//! still, so a mission's length is a property of the schedule rather than
//! of how far apart the generator happened to drop the rooms.

use crate::data::{GameData, Zone};
use crate::geom::Pos;
use crate::rng::Pcg32;
use crate::world::{Beat, BeatTrigger, Protection, Room, RoutineStep, Schedule};

use super::layout::Layout;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleError(pub String);

/// Rooms the target can be alone in: away from the public tiers, and not
/// sealed behind a lock it would need to be let through.
fn private_rooms(layout: &Layout) -> Vec<&Room> {
    layout
        .rooms
        .iter()
        .filter(|r| r.zone == Zone::Personal || r.zone == Zone::Secure)
        .collect()
}

/// Manhattan distance, floors counted as a stair climb. Only used to
/// apportion dwell time, so an estimate is enough — the exact walk is the
/// pathfinder's business.
fn travel_estimate(a: Pos, b: Pos) -> u32 {
    let dx = (a.x - b.x).unsigned_abs() as u32;
    let dy = (a.y - b.y).unsigned_abs() as u32;
    let df = u32::from(a.floor.abs_diff(b.floor));
    dx + dy + df * 12
}

/// Spreads `total` over `n` slots as evenly as integer arithmetic allows,
/// giving the remainder to the earliest slots. Deterministic and free of
/// floating point, so a schedule is identical on every target.
fn spread(total: u32, n: usize) -> Vec<u16> {
    if n == 0 {
        return Vec::new();
    }
    let base = total / n as u32;
    let extra = (total % n as u32) as usize;
    (0..n)
        .map(|i| {
            let d = base + u32::from(i < extra);
            d.min(u32::from(u16::MAX)) as u16
        })
        .collect()
}

/// Builds the target's schedule: a cycle of public beats with at least one
/// private beat folded in, dwells fitted to the venue's cycle budget.
///
/// `public_stops` are candidate positions for escorted beats — normally
/// the waypoints the target's disguise permits.
pub fn build_schedule(
    data: &GameData,
    layout: &Layout,
    public_stops: &[Pos],
    private_kill: bool,
    rng: &mut Pcg32,
) -> Result<Schedule, ScheduleError> {
    let spec = &data.population.target;

    // The alone beat is the mission's whole difficulty budget, so it is
    // chosen first and everything else fits around it.
    let candidates = private_rooms(layout);
    let mut private_stops: Vec<Pos> = Vec::new();
    for room in &candidates {
        if private_kill && room.zone != Zone::Personal {
            continue;
        }
        private_stops.extend(room.waypoints.iter().map(|w| w.pos));
    }
    if private_stops.is_empty() && private_kill {
        // Fall back to any private room: a personal-tier one was preferred
        // but the contract only requires the target be away from a crowd.
        for room in &candidates {
            private_stops.extend(room.waypoints.iter().map(|w| w.pos));
        }
    }
    if private_stops.is_empty() {
        return Err(ScheduleError(
            "no private-tier waypoint for the target to be alone at".into(),
        ));
    }
    if public_stops.is_empty() {
        return Err(ScheduleError("the target has nowhere public to be".into()));
    }

    let alone_count = rng
        .range_inclusive(
            data.population.private_beats_min.into(),
            data.population.private_beats_max.into(),
        )
        .max(1) as usize;
    let total = rng
        .range_inclusive(spec.schedule_min.into(), spec.schedule_max.into())
        .max(alone_count as u32 + 1) as usize;

    // Public beats first, then the private ones folded in at a fixed
    // stride, so the target is never alone twice running and the cycle
    // reads as a day rather than a shuffle.
    let mut beats: Vec<Beat> = Vec::new();
    for _ in 0..total - alone_count {
        beats.push(Beat {
            pos: *rng.pick(public_stops),
            dwell: 0,
            protection: Protection::Escorted,
            no_follow: false,
            trigger: BeatTrigger::Sequential,
            tag: String::new(),
        });
    }
    let stride = (beats.len() / alone_count).max(1);
    for i in 0..alone_count {
        let at = ((i + 1) * stride).min(beats.len());
        beats.insert(
            at,
            Beat {
                pos: *rng.pick(&private_stops),
                dwell: 0,
                protection: Protection::Alone,
                // Alone means alone: the detail waits outside.
                no_follow: true,
                trigger: BeatTrigger::Sequential,
                tag: format!("private-{i}"),
            },
        );
    }

    // Fit the dwells: whatever the cycle budget does not spend walking is
    // spent standing still.
    let cycle = rng.range_inclusive(
        data.population.cycle_turns_min.into(),
        data.population.cycle_turns_max.into(),
    );
    let n = beats.len();
    let mut travel = 0u32;
    for i in 0..n {
        travel += travel_estimate(beats[i].pos, beats[(i + 1) % n].pos);
    }
    let dwell_budget = cycle.saturating_sub(travel).max(n as u32);
    let dwells = spread(dwell_budget, n);
    for (beat, dwell) in beats.iter_mut().zip(dwells) {
        beat.dwell = dwell.max(1);
    }

    let dwell_remaining = beats[0].dwell;
    Ok(Schedule {
        beats,
        index: 0,
        dwell_remaining,
        resume_index: None,
    })
}

/// The routine mirroring a schedule, index for index. Everything that
/// predates beats — pathing, briefing facts, reachability proofs — reads
/// the routine, so the two must stay aligned; [`assert_aligned`] is the
/// guard.
pub fn routine_for(schedule: &Schedule) -> Vec<RoutineStep> {
    schedule
        .beats
        .iter()
        .map(|b| RoutineStep {
            pos: b.pos,
            wait: b.dwell,
        })
        .collect()
}

/// Checks the compatibility hinge: same length, same positions, same
/// dwells. A drift here would let a system reading the routine and one
/// reading the beats disagree about where the target is.
pub fn assert_aligned(schedule: &Schedule, routine: &[RoutineStep]) -> Result<(), ScheduleError> {
    if schedule.beats.len() != routine.len() {
        return Err(ScheduleError(format!(
            "schedule has {} beats but the routine has {} steps",
            schedule.beats.len(),
            routine.len()
        )));
    }
    for (i, (beat, step)) in schedule.beats.iter().zip(routine).enumerate() {
        if beat.pos != step.pos || beat.dwell != step.wait {
            return Err(ScheduleError(format!(
                "beat {i} and routine step {i} disagree"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_is_even_and_exact() {
        for total in [0u32, 1, 7, 100, 137] {
            for n in 1..8usize {
                let parts = spread(total, n);
                assert_eq!(parts.len(), n);
                assert_eq!(
                    parts.iter().map(|p| u32::from(*p)).sum::<u32>(),
                    total,
                    "spread must not lose or invent turns"
                );
                let lo = parts.iter().min().unwrap();
                let hi = parts.iter().max().unwrap();
                assert!(hi - lo <= 1, "spread must be even, got {parts:?}");
            }
        }
    }
}

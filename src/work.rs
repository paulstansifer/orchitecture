//! Workplace staffing and the monthly effects of staffed workplaces.
//!
//! A *workplace* is a placed `Place` whose kind has `work: Some(WorkEffect)`
//! (e.g. a kitchen or a carpenter's workshop). Each workplace instance is one
//! job. [`assign_work`] hands out the population's orcs to those jobs by the
//! player-set [`WorkPriority`] (with a per-instance shadow-priority tiebreak).
//! The staffing those jobs produce ([`workplace_staffing`]) then scales each
//! workplace's monthly effect, resolved through the shared month ledger in
//! `city_effect::compute_workshop_effect` (after construction).

use crate::city::ConstructedCity;
use crate::place::{place_location, FulfilledPorf, PlacedPlaceId};
use crate::population::{Individual, Population};
use crate::resource::{ToolKind, UniqueResource};
use bevy::prelude::{DetectChangesMut, Res, ResMut};
use serde::{Deserialize, Serialize};

/// How eagerly a workplace claims workers, relative to other workplaces.
/// Higher-priority workplaces are staffed first; ties within a level are
/// broken by shadow priority (see [`PlacedPlaceId::shadow`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum WorkPriority {
    VeryLow,
    Low,
    #[default]
    Medium,
    High,
    VeryHigh,
}

impl WorkPriority {
    /// Every level, lowest first — for building the UI dropdown.
    pub const ALL: [WorkPriority; 5] = [
        WorkPriority::VeryLow,
        WorkPriority::Low,
        WorkPriority::Medium,
        WorkPriority::High,
        WorkPriority::VeryHigh,
    ];

    /// Short human-readable label for UI display.
    pub fn label(self) -> &'static str {
        match self {
            WorkPriority::VeryLow => "Very low",
            WorkPriority::Low => "Low",
            WorkPriority::Medium => "Medium",
            WorkPriority::High => "High",
            WorkPriority::VeryHigh => "Very high",
        }
    }

    /// Ordering key: higher means staffed sooner (`VeryLow` = 0, `VeryHigh` = 4).
    pub fn rank(self) -> u8 {
        match self {
            WorkPriority::VeryLow => 0,
            WorkPriority::Low => 1,
            WorkPriority::Medium => 2,
            WorkPriority::High => 3,
            WorkPriority::VeryHigh => 4,
        }
    }
}

/// Effectiveness of a full-time worker.
const FULL_TIME: f32 = 1.0;
/// Effectiveness of each of the two jobs an orc works when doubled up (note
/// this is below the naive 50%, penalizing split attention).
const HALF_TIME: f32 = 0.4;

/// (Re)assign every orc to workplaces by priority. Returns `true` if any orc's
/// `work_jobs` changed, so change-detection-driven callers don't spuriously
/// mark their resource dirty (see [`sync_work`]).
///
/// Workplaces are processed by priority level, highest first. Within one level,
/// higher shadow priority comes first. At each level, with `avail` orcs left
/// and `jobs` workplaces:
///   * if `avail >= jobs`, each job gets one full-time orc and the rest fall to
///     the next level;
///   * otherwise orcs double up — each covers two half-time jobs (`HALF_TIME`
///     each), filling jobs in shadow order until the orcs (or jobs) run out.
///     If that still can't cover every job here, the orcs are exhausted and all
///     lower-priority levels go unstaffed.
pub fn assign_work(individuals: &mut [Individual], cw: &ConstructedCity) -> bool {
    // Every workplace instance, as (id, priority rank, shadow priority).
    let mut workplaces: Vec<(PlacedPlaceId, u8, f32)> = cw
        .placed_places
        .iter()
        .filter(|(_, pp)| cw.places[pp.place].work.is_some())
        .map(|(id, _)| {
            let core = place_location(cw, id);
            let prio = cw.work_priorities.get(&core).copied().unwrap_or_default();
            (id, prio.rank(), id.shadow())
        })
        .collect();
    // Highest priority first, then highest shadow first, then id for a fully
    // deterministic order regardless of `placed_places` iteration order.
    workplaces.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.total_cmp(&a.2)).then(a.0.cmp(&b.0)));

    let n = individuals.len();
    let mut new_jobs: Vec<Vec<(PlacedPlaceId, f32)>> = vec![Vec::new(); n];
    let mut orc = 0; // next unassigned orc; orcs [0, orc) are already staffed.

    let mut i = 0;
    while i < workplaces.len() {
        if orc >= n {
            break; // out of orcs
        }
        // Extent of the current priority level (all share `rank`).
        let rank = workplaces[i].1;
        let mut j = i;
        while j < workplaces.len() && workplaces[j].1 == rank {
            j += 1;
        }
        let group = &workplaces[i..j];
        let avail = n - orc;

        if avail >= group.len() {
            for (k, (id, _, _)) in group.iter().enumerate() {
                new_jobs[orc + k].push((*id, FULL_TIME));
            }
            orc += group.len();
        } else {
            // Not enough orcs: double up. `avail` orcs cover up to `2*avail`
            // jobs, in shadow order; job k goes to orc `orc + k/2`.
            let slots = (2 * avail).min(group.len());
            for k in 0..slots {
                new_jobs[orc + k / 2].push((group[k].0, HALF_TIME));
            }
            orc += slots.div_ceil(2);
            if slots < group.len() {
                break; // couldn't cover this level; nothing left for lower ones.
            }
        }
        i = j;
    }

    let mut changed = false;
    for (ind, jobs) in individuals.iter_mut().zip(new_jobs) {
        if ind.work_jobs != jobs {
            ind.work_jobs = jobs;
            changed = true;
        }
    }
    changed
}

/// Total staffing effectiveness at workplace `id`: the sum of every orc's
/// effectiveness for that workplace (0.0 if unstaffed, up to 1.0 full-time).
pub fn workplace_staffing(individuals: &[Individual], id: PlacedPlaceId) -> f32 {
    individuals
        .iter()
        .flat_map(|ind| ind.work_jobs.iter())
        .filter(|(jid, _)| *jid == id)
        .map(|(_, eff)| eff)
        .sum()
}

/// Re-runs [`assign_work`] whenever the world (workplaces) or population
/// changes. Mirrors `population::sync_assignments`: mutates through
/// `bypass_change_detection` and only `set_changed()` on a real change, since
/// this system's own run condition includes `resource_changed::<Population>`.
pub fn sync_work(mut population: ResMut<Population>, cw: Res<ConstructedCity>) {
    let changed = assign_work(&mut population.bypass_change_detection().individuals, &cw);
    if changed {
        population.set_changed();
    }
}

/// The tool installed in a placed workplace, if any (scans its fulfillments'
/// furniture slots). Drives `WorkEffect::ToolEffect` in
/// `city_effect::compute_workshop_effect`.
pub fn installed_tool_in(cw: &ConstructedCity, id: PlacedPlaceId) -> Option<ToolKind> {
    let pp = cw.placed_places.get(id)?;
    for f in &pp.fulfillments {
        if let FulfilledPorf::Furniture(loc) = f {
            if let Some(slots) = cw.furniture_slots.get(&loc.cube) {
                for item in slots.iter().flatten() {
                    if let UniqueResource::Tool(kind) = item {
                        return Some(*kind);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::{ParentRestriction, ParticularPlace, Place, WorkEffect};
    use crate::resource::Inventory;
    use crate::sparse3d::{Slot, SlotCoord};
    use bevy::math::IVec3;

    fn workplace_def() -> Place {
        Place {
            name: "workshop".to_string(),
            requirements: vec![],
            public_storage: false,
            accounting: None,
            quality_factors: vec![],
            assignable_for: None,
            work: Some(WorkEffect::TodoEffect),
            gate: None,
        }
    }

    /// Build a city of workplaces, one per `(priority, cube)`, cored at `cube`.
    fn city(workplaces: &[(WorkPriority, IVec3)]) -> ConstructedCity {
        let mut cw = ConstructedCity::new(Vec::new());
        cw.places = vec![workplace_def()];
        for (prio, cube) in workplaces {
            cw.placed_places.insert(ParticularPlace {
                place: 0,
                fulfillments: vec![FulfilledPorf::Furniture(SlotCoord {
                    cube: *cube,
                    slot: Slot::Room,
                })],
                contents: Inventory::new([]),
                restriction: ParentRestriction::Unrestricted,
            });
            cw.work_priorities.insert(*cube, *prio);
        }
        cw
    }

    fn cube(x: i32) -> IVec3 {
        IVec3::new(x, 0, 0)
    }

    fn people(n: usize) -> Vec<Individual> {
        (0..n).map(|_| Individual::default()).collect()
    }

    /// The placed place id whose core cube is `cube(x)`.
    fn id_at(cw: &ConstructedCity, x: i32) -> PlacedPlaceId {
        cw.placed_places
            .iter()
            .find(|(id, _)| place_location(cw, *id) == cube(x))
            .map(|(id, _)| id)
            .unwrap()
    }

    #[test]
    fn full_time_by_priority_level() {
        // 2 orcs, 3 workplaces at distinct levels: the two highest get staffed.
        let cw = city(&[
            (WorkPriority::High, cube(0)),
            (WorkPriority::Medium, cube(1)),
            (WorkPriority::Low, cube(2)),
        ]);
        let mut ppl = people(2);
        assign_work(&mut ppl, &cw);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 0)), 1.0);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 1)), 1.0);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 2)), 0.0);
    }

    #[test]
    fn shadow_priority_breaks_ties_within_level() {
        // 1 orc, 3 same-level workplaces: it doubles up across the two highest
        // shadow priorities (later-inserted = higher id = higher shadow); the
        // first-inserted (lowest shadow) goes unstaffed.
        let cw = city(&[
            (WorkPriority::Medium, cube(0)),
            (WorkPriority::Medium, cube(1)),
            (WorkPriority::Medium, cube(2)),
        ]);
        let mut ppl = people(1);
        assign_work(&mut ppl, &cw);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 0)), 0.0);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 1)), HALF_TIME);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 2)), HALF_TIME);
    }

    #[test]
    fn doubling_covers_a_whole_level() {
        // 2 orcs, 3 workplaces at one level, 1 at a lower level: the 3 same-level
        // jobs each get a half-timer; the lower level is left empty.
        let cw = city(&[
            (WorkPriority::High, cube(0)),
            (WorkPriority::High, cube(1)),
            (WorkPriority::High, cube(2)),
            (WorkPriority::Low, cube(3)),
        ]);
        let mut ppl = people(2);
        assign_work(&mut ppl, &cw);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 0)), HALF_TIME);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 1)), HALF_TIME);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 2)), HALF_TIME);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 3)), 0.0);
    }

    #[test]
    fn extra_orcs_leave_workplaces_full() {
        // More orcs than jobs: the single workplace is full-time, no more.
        let cw = city(&[(WorkPriority::Medium, cube(0))]);
        let mut ppl = people(3);
        assign_work(&mut ppl, &cw);
        assert_eq!(workplace_staffing(&ppl, id_at(&cw, 0)), 1.0);
    }
}

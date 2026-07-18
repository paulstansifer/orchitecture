use crate::city::ConstructedCity;
use crate::place::{AssignmentFlavor, PlacedPlaceId};
use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Default)]
pub struct Individual {
    /// The place this individual is assigned to for each flavor of need
    /// (e.g. the bedroom they sleep in). See `assign_places`.
    pub assignments: HashMap<AssignmentFlavor, PlacedPlaceId>,
    /// Fraction (0.0–1.0) of this individual's monthly potato ration that was
    /// satisfied. Shortfalls are prorated evenly across the population rather
    /// than leaving some individuals fully fed and others starved — see
    /// `city_effect::Eat::apply`.
    pub fed_fraction: f32,
}

impl Individual {
    pub fn assigned(&self, flavor: AssignmentFlavor) -> Option<PlacedPlaceId> {
        self.assignments.get(&flavor).copied()
    }

    /// The place this individual sleeps in, if any.
    pub fn home(&self) -> Option<PlacedPlaceId> {
        self.assigned(AssignmentFlavor::Sleep)
    }

    pub fn shelter(&self) -> f32 {
        if self.home().is_some() {
            1.0
        } else {
            0.25
        }
    }

    pub fn food(&self) -> f32 {
        0.25 + 0.75 * self.fed_fraction
    }

    pub fn inspiration(&self) -> f32 {
        1.0
    }

    pub fn morale(&self) -> f32 {
        self.shelter() * self.food() * self.inspiration()
    }
}

#[derive(Resource)]
pub struct Population {
    pub individuals: Vec<Individual>,
}

impl Default for Population {
    fn default() -> Self {
        Population {
            individuals: vec![Individual::default()],
        }
    }
}

pub fn spawn_population(mut commands: Commands) {
    commands.insert_resource(Population::default());
}

/// Assign individuals to places that are `assignable_for` `flavor` (e.g.
/// bedrooms for `Sleep`), one-to-one, keeping existing pairings stable: an
/// individual only loses its assignment if that place no longer qualifies
/// (destroyed, or reused for something else), and unassigned individuals only
/// take places nobody else already holds.
///
/// Returns `true` if any individual's assignment for `flavor` actually
/// changed, so callers driven by change detection (see `sync_assignments`)
/// don't mark their resource dirty on a no-op run.
pub fn assign_places(
    flavor: AssignmentFlavor,
    individuals: &mut [Individual],
    cw: &ConstructedCity,
) -> bool {
    use std::collections::HashSet;

    let eligible: HashSet<PlacedPlaceId> = cw
        .placed_places
        .iter()
        .filter(|(_, pp)| cw.places[pp.place].assignable_for == Some(flavor))
        .map(|(id, _)| id)
        .collect();

    let mut claimed: HashSet<PlacedPlaceId> = HashSet::new();
    let mut new_assignments: Vec<Option<PlacedPlaceId>> = vec![None; individuals.len()];
    for (i, individual) in individuals.iter().enumerate() {
        if let Some(place) = individual.assigned(flavor) {
            if eligible.contains(&place) && claimed.insert(place) {
                new_assignments[i] = Some(place);
            }
        }
    }

    let mut vacant: Vec<PlacedPlaceId> = eligible.difference(&claimed).copied().collect();
    vacant.sort_unstable();
    let mut vacant = vacant.into_iter();
    for assignment in new_assignments.iter_mut() {
        if assignment.is_none() {
            *assignment = vacant.next();
        }
    }

    let mut changed = false;
    for (individual, new_assignment) in individuals.iter_mut().zip(new_assignments) {
        if individual.assigned(flavor) != new_assignment {
            match new_assignment {
                Some(place) => {
                    individual.assignments.insert(flavor, place);
                }
                None => {
                    individual.assignments.remove(&flavor);
                }
            }
            changed = true;
        }
    }
    changed
}

/// Every flavor of assignment individuals can hold, in the order
/// `sync_assignments` processes them.
const ALL_ASSIGNMENT_FLAVORS: [AssignmentFlavor; 2] =
    [AssignmentFlavor::Sleep, AssignmentFlavor::Work];

/// Re-runs `assign_places` for every `AssignmentFlavor` whenever the world
/// (place count) or the population (individual count) changes.
///
/// Mutates through `bypass_change_detection` and only calls `set_changed`
/// when an assignment actually changed: `ResMut::deref_mut` marks a resource
/// changed unconditionally, and this system's own run condition includes
/// `resource_changed::<Population>`, so an unconditional mark would make it
/// re-trigger itself on every frame forever after the first real change.
pub fn sync_assignments(mut population: ResMut<Population>, cw: Res<ConstructedCity>) {
    let individuals = &mut population.bypass_change_detection().individuals;
    let mut changed = false;
    for flavor in ALL_ASSIGNMENT_FLAVORS {
        changed |= assign_places(flavor, individuals, &cw);
    }
    if changed {
        population.set_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eorf::EorfInfo;
    use crate::place::{FulfilledPorf, ParticularPlace, Place, PlaceReq, Porf};
    use bevy::math::IVec3;
    use std::collections::HashSet;

    fn bedroom_place() -> Place {
        Place {
            name: "bedroom".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("pallet".to_string()),
                min: 1,
                max: Some(1),
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            storage: None,
            rack_storage: false,
            quality_factors: vec![],
            assignable_for: Some(AssignmentFlavor::Sleep),
        }
    }

    fn placed(place: usize) -> ParticularPlace {
        ParticularPlace {
            place,
            fulfillments: vec![FulfilledPorf::Furniture(IVec3::ZERO)],
            contents: crate::resource::Inventory::new(1.0),
            restriction: crate::place::ParentRestriction::Unrestricted,
        }
    }

    fn city_with_bedrooms(n: usize) -> ConstructedCity {
        let mut cw = ConstructedCity::new(Vec::<EorfInfo>::new());
        cw.places = vec![bedroom_place()];
        for _ in 0..n {
            cw.placed_places.insert(placed(0));
        }
        cw
    }

    fn individuals(n: usize) -> Vec<Individual> {
        (0..n).map(|_| Individual::default()).collect()
    }

    fn assign_homes(individuals: &mut [Individual], cw: &ConstructedCity) -> bool {
        assign_places(AssignmentFlavor::Sleep, individuals, cw)
    }

    #[test]
    fn assigns_one_to_one() {
        let cw = city_with_bedrooms(2);
        let mut people = individuals(2);
        assign_homes(&mut people, &cw);
        let homes: HashSet<_> = people.iter().filter_map(|i| i.home()).collect();
        assert_eq!(homes.len(), 2);
    }

    #[test]
    fn more_individuals_than_bedrooms_leaves_some_homeless() {
        let cw = city_with_bedrooms(1);
        let mut people = individuals(2);
        assign_homes(&mut people, &cw);
        assert_eq!(people.iter().filter(|i| i.home().is_some()).count(), 1);
    }

    #[test]
    fn keeps_existing_assignment_stable_when_bedroom_added() {
        let cw1 = city_with_bedrooms(1);
        let mut people = individuals(1);
        assign_homes(&mut people, &cw1);
        let original_home = people[0].home();
        assert!(original_home.is_some());

        let cw2 = city_with_bedrooms(2);
        assign_homes(&mut people, &cw2);
        assert_eq!(people[0].home(), original_home);
    }

    #[test]
    fn evicts_when_its_bedroom_is_destroyed() {
        let cw1 = city_with_bedrooms(2);
        let mut people = individuals(2);
        assign_homes(&mut people, &cw1);
        assert!(people.iter().all(|i| i.home().is_some()));

        // Destroy bedroom 1: only bedroom 0 remains.
        let cw2 = city_with_bedrooms(1);
        assign_homes(&mut people, &cw2);
        assert_eq!(people.iter().filter(|i| i.home().is_some()).count(), 1);
        let (remaining, _) = cw2.placed_places.iter().next().unwrap();
        assert!(people.iter().any(|i| i.home() == Some(remaining)));
    }

    #[test]
    fn eviction_targets_the_destroyed_bedroom_not_a_shifted_id() {
        // Two bedrooms; destroy the *first*-inserted one. The individual in
        // the second bedroom must keep it -- with positional indices, the
        // survivor would have "slid down" into the destroyed slot and the
        // wrong individual would have been evicted.
        let mut cw = city_with_bedrooms(2);
        let mut people = individuals(2);
        assign_homes(&mut people, &cw);

        let first = cw.placed_places.ids()[0];
        let second = cw.placed_places.ids()[1];
        let keeper = people
            .iter()
            .position(|i| i.home() == Some(second))
            .unwrap();

        cw.placed_places.remove(first);
        assign_homes(&mut people, &cw);
        assert_eq!(
            people[keeper].home(),
            Some(second),
            "individual in the surviving bedroom keeps their home"
        );
        assert_eq!(people.iter().filter(|i| i.home().is_some()).count(), 1);
    }
}

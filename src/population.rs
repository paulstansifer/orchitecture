use crate::city::ConstructedCity;
use bevy::prelude::*;

#[derive(Default)]
pub struct Individual {
    /// Index into `ConstructedCity::placed_places` of the "bedroom" place
    /// this individual lives in.
    pub home: Option<usize>,
    pub fed_this_month: bool,
}

impl Individual {
    pub fn shelter(&self) -> f32 {
        if self.home.is_some() {
            1.0
        } else {
            0.25
        }
    }

    pub fn food(&self) -> f32 {
        if self.fed_this_month {
            1.0
        } else {
            0.25
        }
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

/// Assign individuals to "bedroom" places one-to-one, keeping existing
/// pairings stable: an individual only loses its home if that place is no
/// longer a bedroom (destroyed, or reused for something else), and homeless
/// individuals only take bedrooms nobody else already holds.
///
/// Returns `true` if any individual's home assignment actually changed, so
/// callers driven by change detection (see `sync_homes`) don't mark their
/// resource dirty on a no-op run.
pub fn assign_homes(individuals: &mut [Individual], cw: &ConstructedCity) -> bool {
    use std::collections::HashSet;

    let bedrooms: HashSet<usize> = (0..cw.placed_places.len())
        .filter(|&i| cw.places[cw.placed_places[i].place].name == "bedroom")
        .collect();

    let mut claimed: HashSet<usize> = HashSet::new();
    let mut new_homes: Vec<Option<usize>> = vec![None; individuals.len()];
    for (i, individual) in individuals.iter().enumerate() {
        if let Some(home) = individual.home {
            if bedrooms.contains(&home) && claimed.insert(home) {
                new_homes[i] = Some(home);
            }
        }
    }

    let mut vacant: Vec<usize> = bedrooms.difference(&claimed).copied().collect();
    vacant.sort_unstable();
    let mut vacant = vacant.into_iter();
    for home in new_homes.iter_mut() {
        if home.is_none() {
            *home = vacant.next();
        }
    }

    let mut changed = false;
    for (individual, new_home) in individuals.iter_mut().zip(new_homes) {
        if individual.home != new_home {
            individual.home = new_home;
            changed = true;
        }
    }
    changed
}

/// Re-runs `assign_homes` whenever the world (bedroom count) or the
/// population (individual count) changes.
///
/// Mutates through `bypass_change_detection` and only calls `set_changed`
/// when an assignment actually changed: `ResMut::deref_mut` marks a resource
/// changed unconditionally, and this system's own run condition includes
/// `resource_changed::<Population>`, so an unconditional mark would make it
/// re-trigger itself on every frame forever after the first real change.
pub fn sync_homes(mut population: ResMut<Population>, cw: Res<ConstructedCity>) {
    let changed = assign_homes(&mut population.bypass_change_detection().individuals, &cw);
    if changed {
        population.set_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eorf::EorfInfo;
    use crate::place::{FulfilledPorf, ParticularPlace, PlaceInfo, PlaceReq, Porf};
    use bevy::math::IVec3;
    use std::collections::HashSet;

    fn bedroom_place() -> PlaceInfo {
        PlaceInfo {
            name: "bedroom".to_string(),
            requirements: vec![PlaceReq {
                requirement: Porf::Furniture("pallet".to_string()),
                min: 1,
                max: Some(1),
                worker_visit_weight: 1.0,
                worker_visit_duration: 1.0,
            }],
            storage: None,
            quality_factors: vec![],
        }
    }

    fn placed(place: usize) -> ParticularPlace {
        ParticularPlace {
            place,
            fulfillments: vec![FulfilledPorf::Furniture(IVec3::ZERO)],
            contents: crate::resource::Inventory::new(1, 1.0),
        }
    }

    fn city_with_bedrooms(n: usize) -> ConstructedCity {
        let mut cw = ConstructedCity::new(Vec::<EorfInfo>::new());
        cw.places = vec![bedroom_place()];
        cw.placed_places = (0..n).map(|_| placed(0)).collect();
        cw
    }

    fn individuals(n: usize) -> Vec<Individual> {
        (0..n).map(|_| Individual::default()).collect()
    }

    #[test]
    fn assigns_one_to_one() {
        let cw = city_with_bedrooms(2);
        let mut people = individuals(2);
        assign_homes(&mut people, &cw);
        let homes: HashSet<_> = people.iter().filter_map(|i| i.home).collect();
        assert_eq!(homes.len(), 2);
    }

    #[test]
    fn more_individuals_than_bedrooms_leaves_some_homeless() {
        let cw = city_with_bedrooms(1);
        let mut people = individuals(2);
        assign_homes(&mut people, &cw);
        assert_eq!(people.iter().filter(|i| i.home.is_some()).count(), 1);
    }

    #[test]
    fn keeps_existing_assignment_stable_when_bedroom_added() {
        let cw1 = city_with_bedrooms(1);
        let mut people = individuals(1);
        assign_homes(&mut people, &cw1);
        let original_home = people[0].home;
        assert!(original_home.is_some());

        let cw2 = city_with_bedrooms(2);
        assign_homes(&mut people, &cw2);
        assert_eq!(people[0].home, original_home);
    }

    #[test]
    fn evicts_when_its_bedroom_is_destroyed() {
        let cw1 = city_with_bedrooms(2);
        let mut people = individuals(2);
        assign_homes(&mut people, &cw1);
        assert!(people.iter().all(|i| i.home.is_some()));

        // Destroy bedroom 1: only bedroom 0 remains.
        let cw2 = city_with_bedrooms(1);
        assign_homes(&mut people, &cw2);
        assert_eq!(people.iter().filter(|i| i.home.is_some()).count(), 1);
        assert!(people.iter().any(|i| i.home == Some(0)));
    }
}

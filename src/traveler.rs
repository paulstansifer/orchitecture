use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::city::ConstructedCity;
use crate::idea::{Idea, IdeaState};
use crate::resource::{ToolKind, UniformResource, UniqueResource};
use crate::surroundings::RoadNetwork;

pub const TRAVELER_CAPACITY: usize = 1;

/// Maximum road-weighted travel cost (`length * factor` summed along the
/// route to the city) a traveler's rejection-sampled origin may lie at --
/// keeps a traveler from materializing somewhere absurdly expensive to reach,
/// without hard-coding a particular shape or distance for "far away".
const MAX_TRAVELER_TRAVEL_COST: f32 = 250.0;

#[derive(Serialize, Deserialize, Clone)]
pub struct TravelerDemand {
    /// One option is chosen at random when rolling an offer.
    pub options: Vec<(UniformResource, std::ops::Range<u32>)>,
}

/// Which books a traveler might be carrying. Both fields describe a range of
/// possibilities rather than a fixed value, so one config can be a generalist
/// pedlar (any idea, a thin scattering) or a specialist (one subject, in depth).
#[derive(Serialize, Deserialize, Clone)]
pub struct BookSpec {
    /// Names of the ideas this traveler might carry a book on. Empty means any
    /// idea. Every name must exist in `ideas.ron` (see
    /// `validate_traveler_info`).
    #[serde(default)]
    pub ideas: Vec<String>,
    /// Fraction of the idea's segments the book covers, in `0.0..=1.0`. A
    /// degenerate range (`start >= end`) means exactly `start`.
    pub coverage: std::ops::Range<f32>,
}

impl BookSpec {
    /// The idea indices this spec may roll, as `idea::roll_book` wants them.
    /// An empty `ideas` list opens it up to everything.
    fn candidates(&self, ideas: &[Idea]) -> Vec<usize> {
        if self.ideas.is_empty() {
            return (0..ideas.len()).collect();
        }
        self.ideas
            .iter()
            .map(|name| {
                crate::idea::idea_by_name(ideas, name)
                    .expect("validate_traveler_info rejects unknown ideas")
            })
            .collect()
    }

    /// How many segments this book teaches, rolled from `coverage`.
    fn roll_segment_count(&self, rng: &mut impl rand::Rng) -> u32 {
        let coverage = if self.coverage.start < self.coverage.end {
            rng.random_range(self.coverage.start..self.coverage.end)
        } else {
            self.coverage.start
        };
        (coverage.clamp(0.0, 1.0) * crate::idea::SEGMENTS as f32).round() as u32
    }
}

/// What a traveler gives in exchange for their demands being met.
#[derive(Serialize, Deserialize, Clone)]
pub enum TravelerReward {
    Tool(ToolKind),
    Resource(UniformResource, std::ops::Range<u32>),
    Book(BookSpec),
}

/// A resolved `TravelerReward`, with any quantity range rolled to a concrete value.
#[derive(Serialize, Deserialize, Clone)]
pub enum ResolvedReward {
    Tool(ToolKind),
    Resource(UniformResource, u32),
    /// A specific book: which idea it covers, which of its segments, and the
    /// title it'll carry once it's on a shelf. Rolled up front so the player
    /// can see what they're being offered before inviting.
    Book {
        idea: usize,
        segments: u64,
        title: String,
    },
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Traveler {
    pub appear_chance: f32,
    pub demands: Vec<TravelerDemand>,
    pub reward: TravelerReward,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct IndividualTraveler {
    pub config_index: usize,
    /// Resolved demands: one `(resource, quantity)` per `TravelerDemand`.
    pub demands: Vec<(UniformResource, u32)>,
    pub reward: ResolvedReward,
    /// Path from the traveler's starting position toward the map origin.
    pub path: Vec<Vec2>,
}

#[derive(Resource, Serialize, Deserialize)]
pub struct TravelerState {
    pub configs: Vec<Traveler>,
    pub current_offer: Option<IndividualTraveler>,
    pub invited: bool,
}

/// Panics if a traveler's book spec names an idea that doesn't exist, or asks
/// for a nonsensical coverage -- either would otherwise surface much later, as
/// a book on nothing or a book that teaches nothing.
fn validate_traveler_info(configs: &[Traveler], ideas: &[Idea]) {
    for (idx, config) in configs.iter().enumerate() {
        let TravelerReward::Book(spec) = &config.reward else {
            continue;
        };
        for name in &spec.ideas {
            assert!(
                crate::idea::idea_by_name(ideas, name).is_some(),
                "traveler {idx} offers a book on unknown idea {name:?} in travelers.ron"
            );
        }
        assert!(
            spec.coverage.start > 0.0 && spec.coverage.start <= 1.0,
            "traveler {idx}'s book coverage must start within 0.0..=1.0, got {:?}",
            spec.coverage
        );
        assert!(
            spec.coverage.end <= 1.0,
            "traveler {idx}'s book coverage can't exceed 1.0, got {:?}",
            spec.coverage
        );
    }
}

pub fn setup_travelers(mut commands: Commands) {
    let ron_content = include_str!("../buildables/travelers.ron");
    let configs: Vec<Traveler> = ron::from_str(ron_content).expect("bad travelers.ron");
    validate_traveler_info(&configs, &crate::idea::load_idea_info());
    commands.insert_resource(TravelerState {
        configs,
        current_offer: None,
        invited: false,
    });
}

/// Rejection-samples a traveler origin from the road network's own nodes:
/// uniformly at random among those lying both outside the already-visible
/// circle (`view_radius`) and within `MAX_TRAVELER_TRAVEL_COST` of the city
/// by road. Sampling from the network's finite node set (rather than
/// resampling arbitrary map points and hoping one lands in the -- typically
/// thin -- valid annulus) means there's no attempt budget to exhaust and thus
/// no unconstrained fallback: if no node meets both criteria, the search
/// degrades gracefully to the cheapest node outside the circle, and only
/// falls back further (to the single farthest node) if literally every node
/// lies inside `view_radius`, which shouldn't happen on a real map.
fn sample_traveler_origin(view_radius: f32, roads: &RoadNetwork, rng: &mut impl rand::Rng) -> Vec2 {
    let outside_circle: Vec<usize> = (0..roads.nodes.len())
        .filter(|&n| roads.nodes[n].length() > view_radius)
        .collect();

    let in_budget: Vec<usize> = outside_circle
        .iter()
        .copied()
        .filter(|&n| roads.dist_to_city(n) <= MAX_TRAVELER_TRAVEL_COST)
        .collect();

    let node = if !in_budget.is_empty() {
        in_budget[rng.random_range(0..in_budget.len())]
    } else if !outside_circle.is_empty() {
        *outside_circle
            .iter()
            .min_by(|&&a, &&b| roads.dist_to_city(a).total_cmp(&roads.dist_to_city(b)))
            .unwrap()
    } else {
        (0..roads.nodes.len())
            .max_by(|&a, &b| {
                roads.nodes[a]
                    .length_squared()
                    .total_cmp(&roads.nodes[b].length_squared())
            })
            .unwrap_or(roads.city_node)
    };

    roads.nodes[node]
}

/// Roll a new offer for the coming month. Clears the previous offer and `invited` flag.
pub fn roll_traveler_offer(
    state: &mut TravelerState,
    view_radius: f32,
    roads: &RoadNetwork,
    ideas: &[Idea],
    rng: &mut impl rand::Rng,
) {
    state.invited = false;
    state.current_offer = None;

    for (idx, config) in state.configs.iter().enumerate() {
        if rng.random::<f32>() >= config.appear_chance {
            continue;
        }

        let mut demands = Vec::with_capacity(config.demands.len());
        for demand in &config.demands {
            let option_idx = rng.random_range(0..demand.options.len());
            let (resource, ref range) = demand.options[option_idx];
            let qty = rng.random_range(range.start..range.end);
            demands.push((resource, qty));
        }

        let effective_radius = if view_radius > 0.0 { view_radius } else { 30.0 };
        let start = sample_traveler_origin(effective_radius, roads, rng);

        // `start` is already an exact road-network node, so the lowest-cost
        // route to the city begins right at it.
        let path = roads.path_points(roads.nearest_node(start));

        let reward = match &config.reward {
            TravelerReward::Tool(kind) => ResolvedReward::Tool(*kind),
            TravelerReward::Resource(res, range) => {
                ResolvedReward::Resource(*res, rng.random_range(range.start..range.end))
            }
            TravelerReward::Book(spec) => {
                let candidates = spec.candidates(ideas);
                let n = spec.roll_segment_count(rng);
                let (idea, mask) = crate::idea::roll_book(&candidates, n, rng);
                ResolvedReward::Book {
                    idea,
                    segments: mask,
                    title: crate::idea::book_title(&ideas[idea].name, rng),
                }
            }
        };

        state.current_offer = Some(IndividualTraveler {
            config_index: idx,
            demands,
            reward,
            path,
        });
        break;
    }
}

/// A traveler's visit this month: its resolved demands (each claimed partly
/// from this month's market inflow, partly from pre-existing storage — see
/// `city_effect::compute_month_effects`) and reward.
///
/// `affordable` reflects whether *all* demands could be fully claimed, and is
/// computed regardless of `invited` — it's what drives the "Invite" checkbox's
/// enabled state, since the player needs to see affordability before deciding
/// to invite. `invited` reflects whether the player actually checked that
/// box. Only when *both* are true were the demands actually claimed
/// (`granted`/`granted_from_storage` are 0 for every demand otherwise), and
/// `apply()`/`apply_resource()` only have an effect in that case.
pub struct TravelerVisit {
    /// `(resource, desired, granted, granted_from_storage)`.
    pub demands: Vec<(UniformResource, u32, u32, u32)>,
    pub reward: ResolvedReward,
    pub path: Vec<Vec2>,
    pub affordable: bool,
    pub invited: bool,
}

impl TravelerVisit {
    fn active(&self) -> bool {
        self.affordable && self.invited
    }

    /// Deducts the storage-backed portion of each demand (the inflow-backed
    /// portion needs no physical action — it was simply never deposited),
    /// deposits a `Tool` reward into the first storage place, shelves a `Book`
    /// reward and learns what it teaches, and reveals the traveler's path. A
    /// `Resource` reward needs no action here: it was folded into this month's
    /// inflow by `compute_month_effects`, so it's already been claimed by
    /// Construction and/or deposited by the normal storage-fill/loss pass.
    /// No-op unless the visit was both affordable and invited.
    pub fn apply(
        &self,
        constructed: &mut ConstructedCity,
        farms: &mut crate::surroundings::farmstead::FarmsResource,
        idea_state: &mut IdeaState,
    ) {
        if !self.active() {
            return;
        }
        for &(res, _, _granted, from_storage) in &self.demands {
            if from_storage > 0 {
                crate::storage::consume_uniform(constructed, res, from_storage);
            }
        }
        match &self.reward {
            ResolvedReward::Tool(kind) => {
                crate::storage::deposit_tool(constructed, *kind);
            }
            ResolvedReward::Resource(..) => {}
            ResolvedReward::Book {
                idea,
                segments,
                title,
            } => {
                // Reading is what teaches, and it happens on arrival: the
                // segments are learned permanently even though the physical
                // book can later be lost with its bookcase. `affordable`
                // already required shelf room, so the deposit should succeed.
                idea_state.learn(*idea, *segments);
                crate::storage::deposit_unique(
                    constructed,
                    UniqueResource::Book {
                        title: title.clone(),
                    },
                );
            }
        }
        farms.traveler_reveals.push(self.path.clone());
    }

    pub fn describe(&self) -> String {
        if !self.invited {
            "A traveler is nearby but hasn't been invited.".to_string()
        } else if self.affordable {
            "A traveler's demands are met; their reward is delivered.".to_string()
        } else {
            "A traveler's demands can't be met this month; they're declined.".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::StdRng, SeedableRng};

    /// The bookseller end-to-end through the real month pipeline: a book offer
    /// is only affordable once there's shelf room, and accepting one both
    /// learns its segments and puts the book on a shelf. Driven through the
    /// headless REPL so the whole chain -- `roll_traveler_offer` ->
    /// `compute_month_effects`' `reward_deliverable` -> `TravelerVisit::apply`
    /// -> `IdeaState` -> `sync_idea_progress` -- is genuinely exercised.
    #[test]
    fn a_bookseller_visit_shelves_the_book_and_teaches_it() {
        use crate::headless::HeadlessSession;

        let mut session = HeadlessSession::new(1);
        let dispatch = |session: &mut HeadlessSession, cmd: &str| {
            session
                .dispatch(cmd)
                .unwrap_or_else(|e| panic!("{cmd}: {e}"))
        };

        // A bookroom (2 bookcases + a chair) is the only thing that gives books
        // anywhere to go; without it every book offer is unaffordable. The bin
        // is for the potatoes/canvas the bookseller wants in trade.
        dispatch(&mut session, "place 100 0 100 room bookcase");
        dispatch(&mut session, "place 100 0 101 room bookcase");
        dispatch(&mut session, "place 100 0 102 xwall chair");
        dispatch(&mut session, "place 100 0 103 room bin");
        dispatch(&mut session, "tick");

        // Advance until the bookseller turns up (they're behind two other
        // configs in a first-match-wins roll, so it takes a few months).
        let mut months = 0;
        let offer = loop {
            assert!(months < 200, "no book offer in 200 months");
            dispatch(&mut session, "advance");
            months += 1;
            let line = dispatch(&mut session, "query traveler").join(" ");
            if line.contains("reward=book_") {
                break line;
            }
        };

        // Stock what they ask for (potatoes plus one of canvas/straw) right
        // before accepting, so months of eating haven't drained it.
        dispatch(&mut session, "set_inventory Potato 40");
        dispatch(&mut session, "set_inventory Canvas 30");
        dispatch(&mut session, "set_inventory Straw 30");

        let before = dispatch(&mut session, "query ideas").join("\n");
        dispatch(&mut session, "invite_traveler");
        dispatch(&mut session, "advance");
        dispatch(&mut session, "tick");
        let after = dispatch(&mut session, "query ideas").join("\n");

        assert_ne!(
            before, after,
            "accepting a book offer ({offer}) should change what the city knows"
        );
        let learned: u32 = dispatch(&mut session, "query ideas")
            .iter()
            .filter_map(|l| {
                let (u, p) = (l.find("understood=")?, l.find("pending=")?);
                let understood: u32 = l[u + 11..l[u..].find('/').unwrap() + u].parse().ok()?;
                let pending: u32 = l[p + 8..].trim().parse().ok()?;
                Some(understood + pending)
            })
            .sum();
        assert_eq!(
            learned, 30,
            "a book teaches exactly 30 segments, understood or not"
        );

        let inventory = dispatch(&mut session, "query inventory").join(" ");
        assert!(
            inventory.to_lowercase().contains("book"),
            "the book itself should end up on a shelf: {inventory}"
        );
    }

    /// A chain of `squares` unit-length squares along the x-axis, city at the
    /// origin corner. Every edge has `trips == 0` (untouched, factor
    /// `FIELD_FACTOR == 5.0`), so the node `n` squares from the city costs
    /// `n * 5.0` to reach -- a predictable cost gradient to sample against.
    fn chain_network(squares: usize) -> RoadNetwork {
        let sq = |x: f32| {
            vec![
                Vec2::new(x, 0.0),
                Vec2::new(x + 1.0, 0.0),
                Vec2::new(x + 1.0, 1.0),
                Vec2::new(x, 1.0),
            ]
        };
        let polygons: Vec<Vec<Vec2>> = (0..squares).map(|i| sq(i as f32)).collect();
        let mut roads = RoadNetwork::build(&polygons, Vec2::ZERO);
        roads.recompute_dist();
        roads
    }

    fn book_seller() -> Traveler {
        Traveler {
            appear_chance: 1.0,
            demands: vec![TravelerDemand {
                options: vec![(UniformResource::Potato, 10..15)],
            }],
            reward: TravelerReward::Book(BookSpec {
                ideas: vec![],
                coverage: 0.6..0.6,
            }),
        }
    }

    /// A book offer is fully resolved when it's rolled, not when it's accepted:
    /// the player has to be able to see which idea (and how much of it) they're
    /// buying before deciding to invite.
    #[test]
    fn a_book_reward_resolves_to_a_concrete_idea_and_segment_set() {
        let ideas = crate::idea::load_idea_info();
        let roads = chain_network(60);
        let mut rng = StdRng::seed_from_u64(11);

        for _ in 0..50 {
            let mut state = TravelerState {
                configs: vec![book_seller()],
                current_offer: None,
                invited: false,
            };
            roll_traveler_offer(&mut state, 10.0, &roads, &ideas, &mut rng);

            let offer = state.current_offer.expect("appear_chance 1.0 always hits");
            let ResolvedReward::Book {
                idea,
                segments,
                title,
            } = offer.reward
            else {
                panic!("a Book config must resolve to a Book reward");
            };
            assert!(idea < ideas.len());
            assert_eq!(segments.count_ones(), 30);
            assert!(
                title.contains(&ideas[idea].name),
                "the title should name the idea it covers: {title:?}"
            );
        }
    }

    /// A spec restricted to one idea must never roll another, and coverage
    /// must translate into that fraction of the segment count.
    #[test]
    fn a_book_spec_honours_its_idea_list_and_coverage() {
        let ideas = crate::idea::load_idea_info();
        let roads = chain_network(60);
        let mut rng = StdRng::seed_from_u64(21);

        let mut config = book_seller();
        config.reward = TravelerReward::Book(BookSpec {
            ideas: vec!["Arithmetic".to_string()],
            coverage: 0.2..0.2,
        });

        for _ in 0..30 {
            let mut state = TravelerState {
                configs: vec![config.clone()],
                current_offer: None,
                invited: false,
            };
            roll_traveler_offer(&mut state, 10.0, &roads, &ideas, &mut rng);
            let ResolvedReward::Book { idea, segments, .. } =
                state.current_offer.expect("always appears").reward
            else {
                panic!("expected a book");
            };
            assert_eq!(ideas[idea].name, "Arithmetic");
            assert_eq!(segments.count_ones(), 10, "20% of 50 segments");
        }
    }

    /// A non-degenerate coverage range should actually vary between offers,
    /// and always stay inside the range it was given.
    #[test]
    fn a_coverage_range_varies_between_offers() {
        let ideas = crate::idea::load_idea_info();
        let roads = chain_network(60);
        let mut rng = StdRng::seed_from_u64(33);

        let mut config = book_seller();
        config.reward = TravelerReward::Book(BookSpec {
            ideas: vec![],
            coverage: 0.2..0.8,
        });

        let mut seen = std::collections::HashSet::new();
        for _ in 0..60 {
            let mut state = TravelerState {
                configs: vec![config.clone()],
                current_offer: None,
                invited: false,
            };
            roll_traveler_offer(&mut state, 10.0, &roads, &ideas, &mut rng);
            let ResolvedReward::Book { segments, .. } =
                state.current_offer.expect("always appears").reward
            else {
                panic!("expected a book");
            };
            let n = segments.count_ones();
            assert!((10..=40).contains(&n), "{n} segments is outside 0.2..0.8");
            seen.insert(n);
        }
        assert!(
            seen.len() > 1,
            "a range should not always roll the same size"
        );
    }

    #[test]
    #[should_panic(expected = "unknown idea")]
    fn traveler_validation_rejects_a_book_on_an_unknown_idea() {
        let mut config = book_seller();
        config.reward = TravelerReward::Book(BookSpec {
            ideas: vec!["Astrology".to_string()],
            coverage: 0.5..0.5,
        });
        validate_traveler_info(&[config], &crate::idea::load_idea_info());
    }

    /// The bundled file has to pass its own validation.
    #[test]
    fn the_bundled_travelers_validate() {
        let configs: Vec<Traveler> =
            ron::from_str(include_str!("../buildables/travelers.ron")).expect("bad travelers.ron");
        validate_traveler_info(&configs, &crate::idea::load_idea_info());
    }

    /// Accepting the offer both shelves the book and teaches its segments; the
    /// two are deliberately independent, so losing the book later doesn't
    /// unlearn anything.
    #[test]
    fn accepting_a_book_learns_its_segments() {
        let mut idea_state = IdeaState::new(3);
        let mut constructed = ConstructedCity::new(Vec::new());
        let mut farms =
            crate::surroundings::map::build_farms_resource(&mut StdRng::seed_from_u64(0));

        let visit = TravelerVisit {
            demands: vec![],
            reward: ResolvedReward::Book {
                idea: 1,
                segments: 0b1011,
                title: "Concerning Organization".to_string(),
            },
            path: vec![],
            affordable: true,
            invited: true,
        };
        visit.apply(&mut constructed, &mut farms, &mut idea_state);
        assert_eq!(idea_state.learned[1], 0b1011);
        assert_eq!(idea_state.learned[0], 0);
    }

    /// An offer the player never invited must not teach anything.
    #[test]
    fn declining_a_book_learns_nothing() {
        let mut idea_state = IdeaState::new(3);
        let mut constructed = ConstructedCity::new(Vec::new());
        let mut farms =
            crate::surroundings::map::build_farms_resource(&mut StdRng::seed_from_u64(0));

        let visit = TravelerVisit {
            demands: vec![],
            reward: ResolvedReward::Book {
                idea: 1,
                segments: 0b1011,
                title: "Concerning Organization".to_string(),
            },
            path: vec![],
            affordable: true,
            invited: false,
        };
        visit.apply(&mut constructed, &mut farms, &mut idea_state);
        assert_eq!(idea_state.learned[1], 0);
    }

    #[test]
    fn sample_traveler_origin_respects_view_radius_and_travel_cost() {
        // 60 unit edges each costing 5.0: nodes range from 0 up to 300,
        // straddling MAX_TRAVELER_TRAVEL_COST (250), so both acceptance and
        // rejection get exercised.
        let roads = chain_network(60);
        let mut rng = StdRng::seed_from_u64(1);

        for _ in 0..50 {
            let origin = sample_traveler_origin(10.0, &roads, &mut rng);
            assert!(
                origin.length() > 10.0,
                "must lie outside the visible circle: {origin:?}"
            );
            let cost = roads.dist_to_city(roads.nearest_node(origin));
            assert!(
                cost <= MAX_TRAVELER_TRAVEL_COST,
                "cost {cost} exceeds the max travel cost"
            );
        }
    }

    /// A view radius large enough to swallow every affordable node: every
    /// remaining candidate costs more than `MAX_TRAVELER_TRAVEL_COST`, so this
    /// exercises the graceful-degradation fallback. Regression test for a bug
    /// where the old continuous-space rejection sampler, on exhausting its
    /// resampling attempts (which real maps hit ~7% of the time), returned a
    /// fully unconstrained point -- observed in practice as a traveler
    /// appearing at the literal edge of the map.
    #[test]
    fn sample_traveler_origin_falls_back_to_the_cheapest_node_when_none_fit_the_budget() {
        let roads = chain_network(60);
        let view_radius = 55.0;
        let mut rng = StdRng::seed_from_u64(1);

        let expected = (0..roads.nodes.len())
            .filter(|&n| roads.nodes[n].length() > view_radius)
            .min_by(|&a, &b| roads.dist_to_city(a).total_cmp(&roads.dist_to_city(b)))
            .map(|n| roads.nodes[n])
            .unwrap();
        assert!(
            roads.dist_to_city(roads.nearest_node(expected)) > MAX_TRAVELER_TRAVEL_COST,
            "sanity check: the setup should leave no node within budget"
        );

        for _ in 0..10 {
            let origin = sample_traveler_origin(view_radius, &roads, &mut rng);
            assert_eq!(
                origin, expected,
                "must deterministically pick the cheapest option, not an arbitrary point"
            );
        }
    }
}

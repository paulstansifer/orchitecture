//! Ideas: the tech "tree", except each idea is a bitmask rather than a node.
//!
//! An idea is [`SEGMENTS`] numbered segments, and the dependency between ideas
//! is **per-segment**: segment #13 of an idea is only *understood* once segment
//! #13 of every idea it depends on is understood. So an idea isn't researched
//! all at once; it fills in raggedly, and a dependent idea can never be further
//! along than the intersection of its prerequisites.
//!
//! Segments are *learned* from books (see [`crate::traveler`]), which arrive
//! carrying an arbitrary scattering of segments — routinely for ideas whose
//! prerequisites you don't have yet. That gap between "learned" (it's written
//! down somewhere) and "understood" (learned, and so is everything underneath
//! it) is the whole point, and the two are shown differently in the UI.
//!
//! Learning is **permanent**: [`IdeaState`] only ever gains bits, so an idea's
//! progress is monotonic and anything gated on it (see [`crate::place::IdeaGate`])
//! never re-locks.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::city::ConstructedCity;

/// Segments per idea. Capped at 64 because progress is stored as a `u64` mask.
pub const SEGMENTS: u32 = 50;
const _: () = assert!(SEGMENTS <= 64);

/// Every segment set, i.e. a fully-understood idea.
pub const ALL_SEGMENTS: u64 = if SEGMENTS == 64 {
    u64::MAX
} else {
    (1u64 << SEGMENTS) - 1
};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Idea {
    pub name: String,
    /// Names of the ideas this one depends on, segment-for-segment.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Loads the idea definitions bundled at compile time, panicking on a reference
/// to an unknown idea or on a dependency cycle. The result is topologically
/// sorted, so every idea's prerequisites precede it — which is what lets
/// [`compute_understood`] resolve the whole DAG in a single forward pass, and
/// what lets the UI lay dependents out below their prerequisites.
pub fn load_idea_info() -> Vec<Idea> {
    let ron_content = include_str!("../buildables/ideas.ron");
    let ideas: Vec<Idea> = ron::from_str(ron_content).expect("bad ideas.ron");
    validate_idea_info(&ideas);
    topo_sort(ideas)
}

/// Panics if any idea depends on a name that doesn't exist, or on itself — a
/// typo in ideas.ron would otherwise silently produce an idea that can never be
/// understood.
fn validate_idea_info(ideas: &[Idea]) {
    for idea in ideas {
        for dep in &idea.depends_on {
            if dep == &idea.name {
                panic!("idea {:?} depends on itself in ideas.ron", idea.name);
            }
            if !ideas.iter().any(|other| &other.name == dep) {
                panic!(
                    "idea {:?} depends on unknown idea {dep:?} in ideas.ron",
                    idea.name
                );
            }
        }
    }
}

/// Kahn's algorithm, with ties broken by the original file order so the layout
/// is stable and editing ideas.ron doesn't shuffle unrelated rows. Panics on a
/// cycle.
fn topo_sort(ideas: Vec<Idea>) -> Vec<Idea> {
    let mut remaining: Vec<Option<Idea>> = ideas.into_iter().map(Some).collect();
    let mut placed: Vec<String> = Vec::with_capacity(remaining.len());
    let mut sorted: Vec<Idea> = Vec::with_capacity(remaining.len());

    while sorted.len() < remaining.len() {
        let ready = remaining.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|idea| idea.depends_on.iter().all(|d| placed.contains(d)))
        });
        let Some(idx) = ready else {
            let stuck: Vec<&str> = remaining
                .iter()
                .flatten()
                .map(|idea| idea.name.as_str())
                .collect();
            panic!("dependency cycle among ideas in ideas.ron: {stuck:?}");
        };
        let idea = remaining[idx].take().expect("just found a filled slot");
        placed.push(idea.name.clone());
        sorted.push(idea);
    }

    sorted
}

/// Each idea's dependencies, as indices into `ideas`. Resolved once so the
/// per-frame path never does name lookups.
pub fn dep_indices(ideas: &[Idea]) -> Vec<Vec<usize>> {
    ideas
        .iter()
        .map(|idea| {
            idea.depends_on
                .iter()
                .map(|dep| {
                    ideas
                        .iter()
                        .position(|other| &other.name == dep)
                        .expect("validate_idea_info rejects unknown dependencies")
                })
                .collect()
        })
        .collect()
}

/// The index of the idea named `name`, if there is one.
pub fn idea_by_name(ideas: &[Idea], name: &str) -> Option<usize> {
    ideas.iter().position(|idea| idea.name == name)
}

/// What the city has read, as one segment mask per idea. Authoritative and
/// permanent — bits are only ever added (see [`IdeaState::learn`]).
///
/// The *derived* "understood" masks live on [`ConstructedCity`] instead, so
/// that the many functions taking only `&ConstructedCity` (place formation,
/// workshop output) can consult them without threading this resource through;
/// [`sync_idea_progress`] keeps that cache current.
#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
pub struct IdeaState {
    pub learned: Vec<u64>,
}

/// A city that hasn't read anything, sized to the bundled idea list.
impl Default for IdeaState {
    fn default() -> Self {
        IdeaState::new(load_idea_info().len())
    }
}

impl IdeaState {
    pub fn new(idea_count: usize) -> Self {
        IdeaState {
            learned: vec![0; idea_count],
        }
    }

    /// Adds `mask`'s segments to `idea`. Returns whether anything was actually
    /// new, so callers can avoid marking resources changed for a no-op.
    pub fn learn(&mut self, idea: usize, mask: u64) -> bool {
        let before = self.learned[idea];
        self.learned[idea] |= mask & ALL_SEGMENTS;
        self.learned[idea] != before
    }
}

/// Resolves learned segments into understood ones: a segment is understood when
/// it's been learned *and* the same segment is understood in every prerequisite.
/// One forward pass suffices because `ideas` is topologically sorted.
pub fn compute_understood(deps: &[Vec<usize>], learned: &[u64]) -> Vec<u64> {
    let mut understood = vec![0u64; learned.len()];
    for idx in 0..learned.len() {
        let mut mask = learned[idx] & ALL_SEGMENTS;
        for &dep in &deps[idx] {
            debug_assert!(dep < idx, "ideas must be topologically sorted");
            mask &= understood[dep];
        }
        understood[idx] = mask;
    }
    understood
}

/// How far along an idea is, in `0.0..=1.0`.
pub fn progress(masks: &[u64], idea: usize) -> f32 {
    masks[idea].count_ones() as f32 / SEGMENTS as f32
}

/// Rolls a book: an idea chosen uniformly from `candidates` (prerequisites
/// deliberately ignored — a book on something you can't yet follow is a normal
/// outcome) and `n_segments` distinct segments of it, via a partial
/// Fisher–Yates shuffle. `n_segments` is clamped to [`SEGMENTS`].
///
/// `candidates` are indices into the idea list; callers that accept any idea
/// pass every index (see `traveler::BookSpec::candidates`).
pub fn roll_book(candidates: &[usize], n_segments: u32, rng: &mut impl rand::Rng) -> (usize, u64) {
    let idea = candidates[rng.random_range(0..candidates.len())];

    let mut pool: Vec<u32> = (0..SEGMENTS).collect();
    let mut mask = 0u64;
    for i in 0..n_segments.min(SEGMENTS) as usize {
        let pick = rng.random_range(i..pool.len());
        pool.swap(i, pick);
        mask |= 1u64 << pool[i];
    }

    (idea, mask)
}

const TITLE_PREFIXES: &[&str] = &[
    "A Treatise on",
    "Notes toward",
    "The Elements of",
    "Concerning",
    "Observations upon",
];

pub fn book_title(idea_name: &str, rng: &mut impl rand::Rng) -> String {
    let prefix = TITLE_PREFIXES[rng.random_range(0..TITLE_PREFIXES.len())];
    format!("{prefix} {idea_name}")
}

/// Refreshes [`ConstructedCity::understood`] from [`IdeaState`].
///
/// Only writes when the masks actually changed: `ResMut`'s `DerefMut` marks the
/// resource changed on any access, and a spurious `ConstructedCity` change
/// re-triggers global illumination and place syncing every frame (see the note
/// on `ui::update_economy_cache`). Scheduled ahead of `sync_places_system`, so
/// learning a segment cascades into place formation on the same frame.
pub fn sync_idea_progress(idea_state: Res<IdeaState>, mut constructed: ResMut<ConstructedCity>) {
    let understood = compute_understood(&constructed.idea_deps, &idea_state.learned);
    if understood != constructed.understood {
        constructed.understood = understood;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::StdRng, SeedableRng};

    fn idea(name: &str, depends_on: &[&str]) -> Idea {
        Idea {
            name: name.to_string(),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// The bundled file has to load, sort, and validate -- this is the only
    /// coverage the real ideas.ron gets, since everything else uses fixtures.
    #[test]
    fn the_bundled_ideas_load() {
        let ideas = load_idea_info();
        let deps = dep_indices(&ideas);
        let arithmetic = idea_by_name(&ideas, "Arithmetic").expect("Arithmetic exists");
        assert_eq!(deps[arithmetic].len(), 2);
        for &dep in &deps[arithmetic] {
            assert!(
                dep < arithmetic,
                "prerequisites must sort before dependents"
            );
        }
    }

    #[test]
    fn topo_sort_puts_prerequisites_first() {
        // Deliberately given in an order that requires reordering.
        let sorted = topo_sort(vec![
            idea("Arithmetic", &["Specialization", "Organization"]),
            idea("Specialization", &[]),
            idea("Organization", &[]),
        ]);
        let names: Vec<&str> = sorted.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["Specialization", "Organization", "Arithmetic"]);
    }

    #[test]
    #[should_panic(expected = "dependency cycle")]
    fn topo_sort_rejects_cycles() {
        topo_sort(vec![idea("A", &["B"]), idea("B", &["A"])]);
    }

    #[test]
    #[should_panic(expected = "unknown idea")]
    fn validation_rejects_unknown_dependencies() {
        validate_idea_info(&[idea("Arithmetic", &["Astrology"])]);
    }

    #[test]
    #[should_panic(expected = "depends on itself")]
    fn validation_rejects_self_dependency() {
        validate_idea_info(&[idea("Arithmetic", &["Arithmetic"])]);
    }

    /// The core rule: a dependent's segment #13 needs *its own* #13 learned and
    /// #13 understood in every prerequisite. Getting two out of three is not
    /// two-thirds of the way there.
    #[test]
    fn understanding_is_per_segment_across_prerequisites() {
        let ideas = vec![
            idea("Specialization", &[]),
            idea("Organization", &[]),
            idea("Arithmetic", &["Specialization", "Organization"]),
        ];
        let deps = dep_indices(&ideas);
        let bit = |n: u32| 1u64 << n;

        // Arithmetic 13 learned, but only one prerequisite has 13.
        let understood = compute_understood(&deps, &[bit(13), bit(7), bit(13)]);
        assert_eq!(understood[2], 0, "one prerequisite short is nothing");

        // Both prerequisites have 13 now.
        let understood = compute_understood(&deps, &[bit(13), bit(13), bit(13)]);
        assert_eq!(understood[2], bit(13));

        // Prerequisites fully understood, but the book on Arithmetic only
        // covered 13 and 20 -- you understand exactly those.
        let understood =
            compute_understood(&deps, &[ALL_SEGMENTS, ALL_SEGMENTS, bit(13) | bit(20)]);
        assert_eq!(understood[2], bit(13) | bit(20));
    }

    /// A segment can be learned and still not understood indefinitely -- that's
    /// the state the UI paints in its own color.
    #[test]
    fn learned_segments_can_outrun_understood_ones() {
        let ideas = vec![
            idea("Specialization", &[]),
            idea("Sums", &["Specialization"]),
        ];
        let deps = dep_indices(&ideas);
        let learned = [0, ALL_SEGMENTS];
        let understood = compute_understood(&deps, &learned);
        assert_eq!(learned[1].count_ones(), SEGMENTS);
        assert_eq!(understood[1], 0);
    }

    #[test]
    fn learn_accumulates_and_reports_novelty() {
        let mut state = IdeaState::new(2);
        assert!(state.learn(0, 0b101));
        assert!(!state.learn(0, 0b001), "already known");
        assert!(state.learn(0, 0b010));
        assert_eq!(state.learned[0], 0b111);
        assert_eq!(state.learned[1], 0);
    }

    /// Bits beyond `SEGMENTS` would show up as extra progress in a bar that only
    /// draws `SEGMENTS` of them.
    #[test]
    fn learn_ignores_out_of_range_segments() {
        let mut state = IdeaState::new(1);
        state.learn(0, u64::MAX);
        assert_eq!(state.learned[0].count_ones(), SEGMENTS);
    }

    #[test]
    fn roll_book_picks_distinct_segments_of_one_idea() {
        let mut rng = StdRng::seed_from_u64(7);
        for _ in 0..100 {
            let (which, mask) = roll_book(&[0, 1, 2], 30, &mut rng);
            assert!(which < 3);
            assert_eq!(mask.count_ones(), 30, "segments must be distinct");
            assert_eq!(mask & !ALL_SEGMENTS, 0, "no segments beyond SEGMENTS");
        }
    }

    /// A traveler restricted to particular ideas must never roll another one.
    #[test]
    fn roll_book_only_picks_from_its_candidates() {
        let mut rng = StdRng::seed_from_u64(5);
        for _ in 0..100 {
            let (which, _) = roll_book(&[2], 10, &mut rng);
            assert_eq!(which, 2);
        }
    }

    #[test]
    fn roll_book_clamps_to_the_segment_count() {
        let mut rng = StdRng::seed_from_u64(3);
        let (_, mask) = roll_book(&[0], 1000, &mut rng);
        assert_eq!(mask, ALL_SEGMENTS);
    }
}

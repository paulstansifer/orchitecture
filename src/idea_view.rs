//! Pure view-model for the Ideas window: turns the raw segment masks into rows
//! of colored segments and layout depths, with no egui involved (same split as
//! [`crate::ui_view`], so the interesting logic is testable headlessly).

use crate::idea::{Idea, SEGMENTS};
use crate::place::Place;

/// Progress at which an idea stops being marked "?" and starts being marked
/// with a check: the point where the city can be said to have the gist of it.
/// Independent of any particular [`crate::place::IdeaGate`] threshold -- it's a
/// reading of the idea itself, not of what it happens to unlock.
pub const CONFIDENT_AT: f32 = 0.5;

/// How one segment of one idea should be drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SegmentState {
    /// Learned, and every prerequisite's matching segment is understood too.
    Understood,
    /// Written down in a book somewhere, but at least one prerequisite is
    /// missing this segment — so it's not usable yet.
    LearnedOnly,
    /// Never read.
    Unknown,
}

/// Where a place gated on this idea currently stands.
#[derive(Clone, PartialEq, Debug)]
pub enum GateStatus {
    /// Not formable yet; the idea needs to reach this progress first.
    Locked { unlock_at: f32 },
    /// Formable, working at this fraction of full output.
    Unlocked { efficiency: f32 },
}

/// A place kind gated on this idea, for the marker's hover text.
#[derive(Clone, PartialEq, Debug)]
pub struct GatedPlaceView {
    pub name: String,
    pub status: GateStatus,
}

impl GatedPlaceView {
    /// One hover line, e.g. "carpenter's workshop — 40% efficiency".
    pub fn describe(&self) -> String {
        match self.status {
            GateStatus::Locked { unlock_at } => format!(
                "{} — needs {}%",
                self.name,
                (unlock_at * 100.0).round() as u32
            ),
            GateStatus::Unlocked { efficiency } => format!(
                "{} — unlocked, {}% efficiency",
                self.name,
                (efficiency * 100.0).round() as u32
            ),
        }
    }
}

pub struct IdeaRow {
    pub name: String,
    /// Longest path from a root. Determines which screen row this idea is
    /// drawn in, so dependents always sit below their prerequisites.
    pub depth: usize,
    /// Exactly [`SEGMENTS`] entries, in segment order.
    pub segments: Vec<SegmentState>,
    pub understood: u32,
    /// Learned but blocked — the count of [`SegmentState::LearnedOnly`].
    pub pending: u32,
    /// Indices into `IdeaTreeView::rows` of this idea's prerequisites. Always
    /// less than this row's own index, since ideas are topologically sorted.
    pub prereqs: Vec<usize>,
    /// Every place kind gated on this idea, locked or not — what the marker's
    /// hover text lists.
    pub gated_places: Vec<GatedPlaceView>,
}

impl IdeaRow {
    /// Fraction of this idea that's understood, in `0.0..=1.0`.
    pub fn progress(&self) -> f32 {
        self.understood as f32 / SEGMENTS as f32
    }

    /// Whether the city has the gist of this idea — drives the "?" / check
    /// marker after its name.
    pub fn confident(&self) -> bool {
        self.progress() >= CONFIDENT_AT
    }

    /// The marker drawn after the name.
    pub fn marker(&self) -> &'static str {
        if self.confident() {
            "✓"
        } else {
            "?"
        }
    }

    /// The line under the name, e.g. "32/50 understood, 9 awaiting
    /// prerequisites".
    pub fn summary(&self) -> String {
        let mut summary = format!("{}/{} understood", self.understood, SEGMENTS);
        if self.pending > 0 {
            summary.push_str(&format!(", {} awaiting prerequisites", self.pending));
        }
        summary
    }
}

pub struct IdeaTreeView {
    /// In topological order, so a row's prerequisites always precede it.
    pub rows: Vec<IdeaRow>,
}

impl IdeaTreeView {
    /// Row indices bucketed by depth: `layers()[d]` is every idea whose longest
    /// path from a root is `d`, in view order. The window draws one layer per
    /// screen row, so an idea always sits below everything it depends on.
    ///
    /// This has to bucket rather than scan: topological order guarantees a
    /// prerequisite *precedes* its dependent, not that depth increases
    /// monotonically along it. Given roots A and C and B depending on A, a
    /// perfectly good topological order is A, B, C — depths 0, 1, 0.
    pub fn layers(&self) -> Vec<Vec<usize>> {
        let depth = self.rows.iter().map(|row| row.depth).max();
        let Some(depth) = depth else {
            return Vec::new();
        };
        let mut layers = vec![Vec::new(); depth + 1];
        for (idx, row) in self.rows.iter().enumerate() {
            layers[row.depth].push(idx);
        }
        layers
    }
}

pub fn idea_tree_view(
    ideas: &[Idea],
    deps: &[Vec<usize>],
    learned: &[u64],
    understood: &[u64],
    places: &[Place],
) -> IdeaTreeView {
    let mut depths: Vec<usize> = Vec::with_capacity(ideas.len());
    let mut rows = Vec::with_capacity(ideas.len());

    for (idx, idea) in ideas.iter().enumerate() {
        // Every prerequisite precedes this idea, so its depth is already known.
        let depth = deps[idx]
            .iter()
            .map(|&dep| depths[dep] + 1)
            .max()
            .unwrap_or(0);
        depths.push(depth);

        let segments: Vec<SegmentState> = (0..SEGMENTS)
            .map(|bit| {
                let mask = 1u64 << bit;
                if understood[idx] & mask != 0 {
                    SegmentState::Understood
                } else if learned[idx] & mask != 0 {
                    SegmentState::LearnedOnly
                } else {
                    SegmentState::Unknown
                }
            })
            .collect();

        let progress = understood[idx].count_ones() as f32 / SEGMENTS as f32;
        let gated_places = places
            .iter()
            .filter_map(|place| {
                let gate = place.gate.as_ref().filter(|g| g.idea == idea.name)?;
                Some(GatedPlaceView {
                    name: place.name.clone(),
                    status: match gate.efficiency(progress) {
                        Some(efficiency) => GateStatus::Unlocked { efficiency },
                        None => GateStatus::Locked {
                            unlock_at: gate.unlock_at,
                        },
                    },
                })
            })
            .collect();

        rows.push(IdeaRow {
            name: idea.name.clone(),
            depth,
            understood: understood[idx].count_ones(),
            pending: (learned[idx] & !understood[idx]).count_ones(),
            segments,
            prereqs: deps[idx].clone(),
            gated_places,
        });
    }

    IdeaTreeView { rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idea::{compute_understood, dep_indices, ALL_SEGMENTS};

    fn ideas() -> Vec<Idea> {
        vec![
            Idea {
                name: "Specialization".to_string(),
                depends_on: vec![],
            },
            Idea {
                name: "Organization".to_string(),
                depends_on: vec![],
            },
            Idea {
                name: "Arithmetic".to_string(),
                depends_on: vec!["Specialization".to_string(), "Organization".to_string()],
            },
        ]
    }

    fn view(learned: &[u64]) -> IdeaTreeView {
        view_with_places(learned, &[])
    }

    fn view_with_places(learned: &[u64], places: &[Place]) -> IdeaTreeView {
        let ideas = ideas();
        let deps = dep_indices(&ideas);
        let understood = compute_understood(&deps, learned);
        idea_tree_view(&ideas, &deps, learned, &understood, places)
    }

    /// The three-way split is the whole point of the view: a segment you've read
    /// about but can't follow has to look different from both a segment you
    /// understand and one you've never seen.
    #[test]
    fn segments_split_three_ways() {
        let bit = |n: u32| 1u64 << n;
        // Arithmetic 5 and 13 are in a book; only 5 has both prerequisites.
        let v = view(&[bit(5), bit(5), bit(5) | bit(13)]);
        let arithmetic = &v.rows[2];

        assert_eq!(arithmetic.segments[5], SegmentState::Understood);
        assert_eq!(arithmetic.segments[13], SegmentState::LearnedOnly);
        assert_eq!(arithmetic.segments[0], SegmentState::Unknown);
        assert_eq!(arithmetic.understood, 1);
        assert_eq!(arithmetic.pending, 1);
        assert_eq!(
            arithmetic.summary(),
            "1/50 understood, 1 awaiting prerequisites"
        );
    }

    /// Nothing to await is worth not mentioning.
    #[test]
    fn the_summary_omits_pending_when_there_is_none() {
        let v = view(&[ALL_SEGMENTS, 0, 0]);
        assert_eq!(v.rows[0].summary(), "50/50 understood");
        assert_eq!(v.rows[0].progress(), 1.0);
    }

    #[test]
    fn depth_counts_the_longest_path_from_a_root() {
        let v = view(&[0, 0, 0]);
        assert_eq!(v.rows[0].depth, 0);
        assert_eq!(v.rows[1].depth, 0);
        assert_eq!(v.rows[2].depth, 1);
        assert_eq!(v.rows[2].prereqs, vec![0, 1]);
    }

    fn gated_place(name: &str, idea: &str, unlock_at: f32) -> Place {
        let mut place = Place {
            name: name.to_string(),
            requirements: vec![],
            public_storage: false,
            accounting: None,
            quality_factors: vec![],
            assignable_for: None,
            work: None,
            gate: None,
        };
        place.gate = Some(crate::place::IdeaGate {
            idea: idea.to_string(),
            unlock_at,
            full_at: 1.0,
        });
        place
    }

    /// The marker is a reading of the idea itself, so it flips at
    /// `CONFIDENT_AT` regardless of what any gate's threshold happens to be.
    #[test]
    fn the_marker_flips_at_the_confidence_threshold() {
        let below = (1u64 << 24) - 1; // 24/50
        let at = (1u64 << 25) - 1; // 25/50 == 50%
        let v = view(&[below, at, 0]);
        assert_eq!(v.rows[0].marker(), "?");
        assert!(!v.rows[0].confident());
        assert_eq!(v.rows[1].marker(), "✓");
        assert!(v.rows[1].confident());
    }

    /// A place gated on an idea shows up under that idea only, with its live
    /// status -- that's what the marker's hover has to say.
    #[test]
    fn gated_places_are_listed_under_their_own_idea_with_live_status() {
        let places = vec![
            gated_place("carpenter's workshop", "Specialization", 0.5),
            gated_place("counting house", "Arithmetic", 0.25),
        ];

        // Nothing understood: both gates locked.
        let v = view_with_places(&[0, 0, 0], &places);
        assert_eq!(
            v.rows[0].gated_places[0].status,
            GateStatus::Locked { unlock_at: 0.5 }
        );
        assert_eq!(v.rows[1].gated_places, vec![], "Organization gates nothing");
        assert_eq!(
            v.rows[0].gated_places[0].describe(),
            "carpenter's workshop — needs 50%"
        );

        // Three quarters of Specialization: unlocked, halfway up the ramp.
        let v = view_with_places(&[(1u64 << 38) - 1, 0, 0], &places);
        let GateStatus::Unlocked { efficiency } = v.rows[0].gated_places[0].status else {
            panic!("should be unlocked");
        };
        assert!((efficiency - 0.52).abs() < 0.01, "got {efficiency}");
        assert_eq!(
            v.rows[0].gated_places[0].describe(),
            "carpenter's workshop — unlocked, 52% efficiency"
        );
    }

    /// Ideas at the same depth share a screen row, and bucketing (rather than
    /// scanning the topologically-sorted list) is what makes that reliable.
    #[test]
    fn layers_group_ideas_by_depth() {
        let v = view(&[0, 0, 0]);
        assert_eq!(v.layers(), vec![vec![0, 1], vec![2]]);
    }

    /// Regression guard for the bucketing: a topological order can perfectly
    /// well run root, dependent, root -- depths 0, 1, 0 -- so a layer's members
    /// need not be contiguous in `rows`.
    #[test]
    fn layers_handle_depth_that_dips_back_down() {
        let ideas = vec![
            Idea {
                name: "A".to_string(),
                depends_on: vec![],
            },
            Idea {
                name: "B".to_string(),
                depends_on: vec!["A".to_string()],
            },
            Idea {
                name: "C".to_string(),
                depends_on: vec![],
            },
        ];
        let deps = dep_indices(&ideas);
        let learned = [0, 0, 0];
        let understood = compute_understood(&deps, &learned);
        let v = idea_tree_view(&ideas, &deps, &learned, &understood, &[]);

        assert_eq!(
            v.rows.iter().map(|r| r.depth).collect::<Vec<_>>(),
            vec![0, 1, 0]
        );
        assert_eq!(v.layers(), vec![vec![0, 2], vec![1]]);
    }

    #[test]
    fn every_row_has_one_entry_per_segment() {
        let v = view(&[0, 0, 0]);
        for row in &v.rows {
            assert_eq!(row.segments.len(), SEGMENTS as usize);
        }
    }
}

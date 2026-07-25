//! Pure view-model for the Ideas window: turns the raw segment masks into rows
//! of colored segments and layout depths, with no egui involved (same split as
//! [`crate::ui_view`], so the interesting logic is testable headlessly).

use crate::idea::{Idea, SEGMENTS};

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

pub struct IdeaRow {
    pub name: String,
    /// Longest path from a root, used to indent dependents under their
    /// prerequisites.
    pub depth: usize,
    /// Exactly [`SEGMENTS`] entries, in segment order.
    pub segments: Vec<SegmentState>,
    pub understood: u32,
    /// Learned but blocked — the count of [`SegmentState::LearnedOnly`].
    pub pending: u32,
    /// Indices into `IdeaTreeView::rows` of this idea's prerequisites. Always
    /// less than this row's own index, since ideas are topologically sorted.
    pub prereqs: Vec<usize>,
}

impl IdeaRow {
    /// Fraction of this idea that's understood, in `0.0..=1.0`.
    pub fn progress(&self) -> f32 {
        self.understood as f32 / SEGMENTS as f32
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

pub fn idea_tree_view(
    ideas: &[Idea],
    deps: &[Vec<usize>],
    learned: &[u64],
    understood: &[u64],
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

        rows.push(IdeaRow {
            name: idea.name.clone(),
            depth,
            understood: understood[idx].count_ones(),
            pending: (learned[idx] & !understood[idx]).count_ones(),
            segments,
            prereqs: deps[idx].clone(),
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
        let ideas = ideas();
        let deps = dep_indices(&ideas);
        let understood = compute_understood(&deps, learned);
        idea_tree_view(&ideas, &deps, learned, &understood)
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

    #[test]
    fn every_row_has_one_entry_per_segment() {
        let v = view(&[0, 0, 0]);
        for row in &v.rows {
            assert_eq!(row.segments.len(), SEGMENTS as usize);
        }
    }
}

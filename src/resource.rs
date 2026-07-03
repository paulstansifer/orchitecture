use rgb::Rgb;
use serde::{Deserialize, Serialize};

// Only need to store a quantity, since these are all equivalent
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum UniformResource {
    Potato,
    Timber,
    Straw,
    WoodBeam,
    Canvas,
    Fieldstone,
    Block,
    Lime,
    Plank,
}

impl UniformResource {
    pub const ALL: &'static [UniformResource] = &[
        UniformResource::Potato,
        UniformResource::Timber,
        UniformResource::Straw,
        UniformResource::WoodBeam,
        UniformResource::Canvas,
        UniformResource::Fieldstone,
        UniformResource::Block,
        UniformResource::Lime,
        UniformResource::Plank,
    ];
}

impl UniformResource {
    pub fn label(self) -> &'static str {
        match self {
            UniformResource::Potato => "Potatoes",
            UniformResource::Timber => "Timber",
            UniformResource::Straw => "Straw",
            UniformResource::WoodBeam => "Wood beams",
            UniformResource::Canvas => "Canvas",
            UniformResource::Fieldstone => "Fieldstone",
            UniformResource::Block => "Blocks",
            UniformResource::Lime => "Lime",
            UniformResource::Plank => "Planks",
        }
    }

    pub fn farmable(self) -> bool {
        // TODO: add quarries; Lime should not be farmable, and blocks should be getable
        // TODO: add ground-clearing; Fieldstone should not be farmable
        matches!(
            self,
            UniformResource::Potato
                | UniformResource::Canvas
                | UniformResource::Straw
                | UniformResource::Timber
                | UniformResource::Lime
                | UniformResource::Fieldstone
        )
    }

    pub fn inedible_farmables() -> &'static [UniformResource] {
        &[
            UniformResource::Canvas,
            UniformResource::Straw,
            UniformResource::Timber,
            UniformResource::Lime,
            UniformResource::Fieldstone,
        ]
    }
}

/// The kind of a tool. Tools are otherwise interchangeable, but their kind
/// determines what a farm produces when it specializes with that tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub enum ToolKind {
    Whipsaw,
}

/// How a tool transforms neighbouring production when a farm specializes:
/// it converts `input` produced by adjacent farms into `output`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Specialization {
    pub input: UniformResource,
    pub output: UniformResource,
}

impl ToolKind {
    pub fn label(self) -> &'static str {
        match self {
            ToolKind::Whipsaw => "Whipsaw",
        }
    }

    /// What this tool turns neighboring production into when a farm specializes.
    pub fn specialization(self) -> Specialization {
        match self {
            ToolKind::Whipsaw => Specialization {
                input: UniformResource::Timber,
                output: UniformResource::WoodBeam,
            },
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum UniqueResource {
    Book { title: String },
    Rug { color: Rgb<u8> },
    Tool(ToolKind),
}

impl UniqueResource {
    pub fn volume(&self) -> f32 {
        match self {
            UniqueResource::Book { .. } => 0.05,
            UniqueResource::Rug { .. } => 1.0,
            UniqueResource::Tool(_) => 0.5,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum InventoryEntry {
    Uniform(UniformResource, u16),
    Collection(Vec<UniqueResource>),
}

#[allow(dead_code)]
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Inventory {
    contents: Vec<InventoryEntry>,
    max_types: u8,
    max_volume: f32,
}

impl Inventory {
    pub fn new(max_types: u8, max_volume: f32) -> Self {
        Inventory {
            contents: Vec::new(),
            max_types,
            max_volume,
        }
    }

    /// Adds a quantity of a uniform resource, merging into an existing entry of the
    /// same kind if present.
    pub fn add_uniform(&mut self, res: UniformResource, qty: u16) {
        for entry in &mut self.contents {
            if let InventoryEntry::Uniform(existing, existing_qty) = entry {
                if *existing == res {
                    *existing_qty = existing_qty.saturating_add(qty);
                    return;
                }
            }
        }
        self.contents.push(InventoryEntry::Uniform(res, qty));
    }

    /// Per-kind totals of uniform resources held in this inventory.
    pub fn uniform_totals(&self) -> Vec<(UniformResource, u16)> {
        let mut totals: Vec<(UniformResource, u16)> = Vec::new();
        for entry in &self.contents {
            if let InventoryEntry::Uniform(res, qty) = entry {
                if let Some(slot) = totals.iter_mut().find(|(r, _)| r == res) {
                    slot.1 = slot.1.saturating_add(*qty);
                } else {
                    totals.push((*res, *qty));
                }
            }
        }
        totals
    }

    pub fn total_volume(&self) -> f32 {
        let mut res = 0.0;
        for entry in &self.contents {
            use crate::resource::InventoryEntry::*;
            match entry {
                Uniform(_, item_qty) => res += *item_qty as f32,
                Collection(items) => res += items.iter().map(|i| i.volume()).sum::<f32>(),
            }
        }
        res
    }

    pub fn may_add(&self, _new_stuff: &InventoryEntry) -> bool {
        todo!()
    }

    pub fn add_unique(&mut self, item: UniqueResource) {
        for entry in &mut self.contents {
            if let InventoryEntry::Collection(items) = entry {
                items.push(item);
                return;
            }
        }
        self.contents.push(InventoryEntry::Collection(vec![item]));
    }

    pub fn tool_count(&self) -> usize {
        let mut count = 0;
        for entry in &self.contents {
            if let InventoryEntry::Collection(items) = entry {
                count += items
                    .iter()
                    .filter(|i| matches!(i, UniqueResource::Tool(_)))
                    .count();
            }
        }
        count
    }

    /// Count tools of a specific kind held in this inventory.
    pub fn tool_count_of(&self, kind: ToolKind) -> usize {
        let mut count = 0;
        for entry in &self.contents {
            if let InventoryEntry::Collection(items) = entry {
                count += items
                    .iter()
                    .filter(|i| matches!(i, UniqueResource::Tool(k) if *k == kind))
                    .count();
            }
        }
        count
    }

    /// Remove a single matching unique item. Returns `true` if one was removed.
    pub fn remove_unique(&mut self, item: &UniqueResource) -> bool {
        for entry in &mut self.contents {
            if let InventoryEntry::Collection(items) = entry {
                if let Some(pos) = items.iter().position(|i| i == item) {
                    items.remove(pos);
                    return true;
                }
            }
        }
        false
    }

    pub fn subtract_uniform(&mut self, res: UniformResource, qty: u16) {
        for entry in &mut self.contents {
            if let InventoryEntry::Uniform(existing, existing_qty) = entry {
                if *existing == res {
                    *existing_qty = existing_qty.saturating_sub(qty);
                    return;
                }
            }
        }
    }
}

// How good is our bookkkeeping?
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Approximation {
    pub digits: u8,
    pub max: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    Exact,
    Approximate,
    Conservative,
}

// Simulate bookkeepping limitations; returns the rounded value and precision info
pub fn round(orig: u16, approx: Approximation) -> (u16, Precision) {
    let capped = orig > approx.max;
    let res = orig.min(approx.max);

    let too_long = u16::pow(10, approx.digits.into());

    let mut res_digits = res;
    let mut res_zeroes = 1;
    while res_digits >= too_long {
        res_digits /= 10;
        res_zeroes *= 10;
    }

    let res = res_digits * res_zeroes;

    let precision = if res == orig {
        Precision::Exact
    } else if capped {
        Precision::Conservative
    } else {
        Precision::Approximate
    };

    (res, precision)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCT: Approximation = Approximation {
        digits: 1,
        max: 100,
    };

    #[test]
    fn round_small_values_unchanged() {
        assert_eq!(round(9, ACCT), (9, Precision::Exact));
        assert_eq!(round(20, ACCT), (20, Precision::Exact));
        assert_eq!(round(10, ACCT), (10, Precision::Exact));
    }

    #[test]
    fn round_multi_digit_drops_to_one_sig_digit() {
        // 137 -> capped at 100, which is already 1 significant digit.
        assert_eq!(round(137, ACCT), (100, Precision::Conservative));
    }

    #[test]
    fn round_caps_at_max() {
        // Above max: capped to 100 and flagged as conservative.
        assert_eq!(round(250, ACCT), (100, Precision::Conservative));
    }

    #[test]
    fn round_truncates_significant_digits() {
        // Under the cap but more than one significant digit: 47 -> 40.
        assert_eq!(round(47, ACCT), (40, Precision::Approximate));
    }
}

use rgb::Rgb;
use serde::{Deserialize, Serialize};

// Only need to store a quantity, since these are all equivalent
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum UniformResource {
    Potato,
    Timber,
    WoodBeam,
    Canvas,
    Fieldstone,
    Block,
}

impl UniformResource {
    pub fn label(self) -> &'static str {
        match self {
            UniformResource::Potato => "Potatoes",
            UniformResource::Timber => "Timber",
            UniformResource::WoodBeam => "Wood beams",
            UniformResource::Canvas => "Canvas",
            UniformResource::Fieldstone => "Fieldstone",
            UniformResource::Block => "Blocks",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum UniqueResource {
    Book { title: String },
    Rug { color: Rgb<u8> },
}

impl UniqueResource {
    pub fn volume(&self) -> f32 {
        match self {
            UniqueResource::Book { .. } => 0.05,
            UniqueResource::Rug { .. } => 1.0,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum InventoryEntry {
    Uniform(UniformResource, u16),
    Collection(Vec<UniqueResource>),
}

#[allow(dead_code)]
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
        return res;
    }

    pub fn may_add(&self, _new_stuff: &InventoryEntry) -> bool {
        todo!()
    }
}

// How good is our bookkkeeping?
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Approximation {
    pub digits: u8,
    pub max: u16,
}

// Simulate bookkeepping limitations; returns whether we should display ">"
pub fn round(orig: u16, approx: Approximation) -> (u16, bool) {
    // Cap at the bookkeeping maximum we're able to record.
    let res = orig.min(approx.max);

    let too_long = u16::pow(10, approx.digits.into());

    let mut res_digits = res;
    let mut res_zeroes = 1;
    while res_digits >= too_long {
        res_digits /= 10;
        res_zeroes *= 10;
    }

    let res = res_digits * res_zeroes;

    (res, res < orig)
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
        assert_eq!(round(9, ACCT), (9, false));
        assert_eq!(round(20, ACCT), (20, false));
        assert_eq!(round(10, ACCT), (10, false));
    }

    #[test]
    fn round_multi_digit_drops_to_one_sig_digit() {
        // 137 -> capped at 100, which is already 1 significant digit.
        assert_eq!(round(137, ACCT), (100, true));
    }

    #[test]
    fn round_caps_at_max() {
        // Above max: capped to 100 and flagged as rounded down.
        assert_eq!(round(250, ACCT), (100, true));
    }

    #[test]
    fn round_truncates_significant_digits() {
        // Under the cap but more than one significant digit: 47 -> 40.
        assert_eq!(round(47, ACCT), (40, true));
    }
}

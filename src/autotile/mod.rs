pub mod compiler;
#[cfg(autotile_matching)]
pub mod display;
#[cfg(autotile_matching)]
pub mod matcher;
#[cfg(autotile_matching)]
pub mod meshes;
#[cfg(autotile_matching)]
pub mod motif;
pub mod parser;
#[cfg(test)]
pub mod test_helpers;

pub use compiler::*;
#[cfg(autotile_matching)]
pub use display::autotile_update_system;
#[cfg(autotile_matching)]
pub use matcher::*;
#[cfg(autotile_matching)]
pub use meshes::{
    load_autotile_handles, spawn_autotile_rules, spawn_empty_anchor_index, AutotileHandles,
    AutotileRules, EmptyAnchorRules,
};
#[cfg(autotile_matching)]
pub use motif::{collapse_motif_atoms, DefectAtom, MotifAtom, MotifOccurrence};
pub use parser::*;

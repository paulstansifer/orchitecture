#![cfg(test)]

use super::{AutotileResult, MeshSpec};
use crate::city::Cell;
use crate::eorf::EorfId;
use crate::sparse3d::Facing;

// ── MeshSpec helpers ─────────────────────────────────────────────────────────

pub fn atom(name: &str) -> MeshSpec {
    MeshSpec::Atom {
        name: name.to_owned(),
        rotation: 0,
        translation: None,
    }
}

pub fn atom_r(name: &str, r: i32) -> MeshSpec {
    MeshSpec::Atom {
        name: name.to_owned(),
        rotation: r,
        translation: None,
    }
}

pub fn atom_t(name: &str, t: (i32, i32, i32)) -> MeshSpec {
    MeshSpec::Atom {
        name: name.to_owned(),
        rotation: 0,
        translation: Some(t),
    }
}

pub fn atom_rt(name: &str, r: i32, t: (i32, i32, i32)) -> MeshSpec {
    MeshSpec::Atom {
        name: name.to_owned(),
        rotation: r,
        translation: Some(t),
    }
}

// ── Cell construction helpers ────────────────────────────────────────────────

pub fn make_cell(id: u32) -> Cell {
    Cell {
        id: EorfId(id),
        facing: Default::default(),
        evaluation: None,
        build_material: Default::default(),
    }
}

pub fn wall_cell() -> Cell {
    make_cell(0)
}

pub fn floor_cell() -> Cell {
    make_cell(1)
}

pub fn desk_cell() -> Cell {
    make_cell(1)
}

pub fn stairs_cell(facing: Facing) -> Cell {
    Cell {
        id: EorfId(2),
        facing,
        evaluation: None,
        build_material: Default::default(),
    }
}

pub fn col_cell() -> Cell {
    make_cell(1)
}

// ── Mesh result extraction helpers ───────────────────────────────────────────

pub fn mesh_rotation(r: &AutotileResult) -> i32 {
    match r {
        AutotileResult::Mesh { spec, .. } => spec.outer_rotation(),
        other => panic!("expected mesh, got {other:?}"),
    }
}

pub fn mesh_rotations(results: &[&AutotileResult]) -> Vec<i32> {
    let mut rs: Vec<i32> = results
        .iter()
        .map(|r| match r {
            AutotileResult::Mesh { spec, .. } => spec.outer_rotation(),
            other => panic!("expected mesh, got {other:?}"),
        })
        .collect();
    rs.sort();
    rs
}

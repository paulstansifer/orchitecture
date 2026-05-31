use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// Re-use the parser and types from the main crate. The module only imports
// from std, so it compiles cleanly as a build-dependency.
// `autotile_matching` is intentionally absent here; the cfg-gated matching
// code is ignored when this file is included.
#[allow(dead_code, unused_imports, unexpected_cfgs)]
mod autotile {
    include!("src/autotile.rs");
}

use autotile::{AutotileFile, AutotileResult, MeshSpec};

fn main() {
    // Register and set `autotile_matching` so the main crate can gate
    // the bevy/crate-dependent matching code that build.rs doesn't need.
    println!("cargo::rustc-check-cfg=cfg(autotile_matching)");
    println!("cargo:rustc-cfg=autotile_matching");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let buildables = manifest.join("buildables");

    // Re-run if any autotile file or atom scad file changes.
    println!("cargo:rerun-if-changed=buildables");

    let autotile_files = collect_autotile_files(&buildables);
    if autotile_files.is_empty() {
        return;
    }

    for path in &autotile_files {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let out_dir = buildables.join("autotile");
    fs::create_dir_all(&out_dir)
        .unwrap_or_else(|e| panic!("Failed to create {}: {e}", out_dir.display()));

    // Parse all autotile files and collect specs that need to be generated.
    let mut spec_map: HashMap<String, MeshSpec> = HashMap::new();
    for path in &autotile_files {
        let src = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));

        match autotile::parse(&src) {
            Ok(file) => collect_result_specs(&file, &mut spec_map),
            Err(e) => println!("cargo:warning=Parse error in {}: {e}", path.display()),
        }
    }

    for (name, spec) in &spec_map {
        let scad_deps = collect_scad_deps(spec, &buildables);
        for dep in &scad_deps {
            println!("cargo:rerun-if-changed={}", dep.display());
        }

        let all_inputs: Vec<&Path> = autotile_files
            .iter()
            .map(PathBuf::as_path)
            .chain(scad_deps.iter().map(PathBuf::as_path))
            .collect();

        generate_if_needed(name, spec, &buildables, &out_dir, &all_inputs);
    }
}

// ─── File discovery ───────────────────────────────────────────────────────────

fn collect_autotile_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return vec![];
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("autotile"))
        .collect();
    paths.sort(); // deterministic order
    paths
}

// ─── Spec collection ──────────────────────────────────────────────────────────

/// Adds every non-trivial top-level result spec to `map` (key = generated filename stem).
/// Atom:0 specs are trivial — they already exist as base meshes.
fn collect_result_specs(file: &AutotileFile, map: &mut HashMap<String, MeshSpec>) {
    for rule in &file.rules {
        for case in &rule.cases {
            if let AutotileResult::Mesh { spec, .. } = &case.result {
                if !is_trivial(spec) {
                    map.insert(spec_stem(spec), spec.clone());
                }
            }
        }
    }
}

fn is_trivial(spec: &MeshSpec) -> bool {
    matches!(spec, MeshSpec::Atom { rotation: 0, .. })
}

/// Deterministic filename stem for a spec (no extension).
pub fn spec_stem(spec: &MeshSpec) -> String {
    match spec {
        MeshSpec::Atom { name, rotation: 0 } => name.clone(),
        MeshSpec::Atom { name, rotation } => format!("{name}_r{rotation}"),
        MeshSpec::Union(a, b) => format!("union__{}__{}", spec_stem(a), spec_stem(b)),
        MeshSpec::Intersection(a, b) => format!("isect__{}__{}", spec_stem(a), spec_stem(b)),
    }
}

/// All atom .scad source files referenced by a spec.
fn collect_scad_deps(spec: &MeshSpec, buildables: &Path) -> Vec<PathBuf> {
    let mut deps = Vec::new();
    collect_scad_deps_rec(spec, buildables, &mut deps);
    deps.sort();
    deps.dedup();
    deps
}

fn collect_scad_deps_rec(spec: &MeshSpec, buildables: &Path, deps: &mut Vec<PathBuf>) {
    match spec {
        MeshSpec::Atom { name, .. } => deps.push(buildables.join(format!("{name}.scad"))),
        MeshSpec::Union(a, b) | MeshSpec::Intersection(a, b) => {
            collect_scad_deps_rec(a, buildables, deps);
            collect_scad_deps_rec(b, buildables, deps);
        }
    }
}

// ─── OpenSCAD code generation ─────────────────────────────────────────────────

/// Generate OpenSCAD source for a mesh spec.
/// Atom paths are absolute so the generated .scad files work from any working directory.
fn spec_to_scad(spec: &MeshSpec, buildables: &Path) -> String {
    match spec {
        MeshSpec::Atom { name, rotation } => {
            let scad = buildables.join(format!("{name}.scad"));
            let inc = format!("include <{}>", scad.display());
            if *rotation == 0 {
                inc
            } else {
                // Rotation is in 90° units around the Y axis.
                // In OpenSCAD (Z-up), game-Y ≡ OpenSCAD-Z, so we rotate around Z.
                format!("rotate([0, 0, {}])\n{}", rotation * 90, inc)
            }
        }
        MeshSpec::Union(a, b) => format!(
            "union() {{\n    {}\n    {}\n}}",
            spec_to_scad(a, buildables),
            spec_to_scad(b, buildables)
        ),
        MeshSpec::Intersection(a, b) => format!(
            "intersection() {{\n    {}\n    {}\n}}",
            spec_to_scad(a, buildables),
            spec_to_scad(b, buildables)
        ),
    }
}

/// Generate the cut-view variant: intersect the mesh with a jagged floor plane.
fn spec_to_cut_scad(spec: &MeshSpec, buildables: &Path) -> String {
    let inner = spec_to_scad(spec, buildables);
    // jagged.dat lives in buildables/; the cut .scad is written to buildables/autotile/
    // so the relative path is ../jagged.dat.
    let jagged = buildables.join("jagged.dat");
    format!(
        "intersection() {{\n    \
         {inner}\n    \
         union() {{\n        \
         surface(\"{jagged}\");\n        \
         translate([0,0,-13])\n        \
         cube([13,13,13]);\n    \
         }}\n\
         }}",
        jagged = jagged.display()
    )
}

// ─── Incremental build helpers ────────────────────────────────────────────────

fn needs_rebuild(output: &Path, inputs: &[&Path]) -> bool {
    let Ok(meta) = fs::metadata(output) else {
        return true;
    };
    let Ok(out_time) = meta.modified() else {
        return true;
    };
    inputs.iter().any(|&inp| {
        fs::metadata(inp)
            .and_then(|m| m.modified())
            .map(|t| t > out_time)
            .unwrap_or(false)
    })
}

// ─── Mesh generation ──────────────────────────────────────────────────────────

fn generate_if_needed(
    stem: &str,
    spec: &MeshSpec,
    buildables: &Path,
    out_dir: &Path,
    inputs: &[&Path],
) {
    let main_gltf = out_dir.join(format!("{stem}.gltf"));
    let cut_gltf = out_dir.join(format!("{stem}-cut-y-pos.gltf"));

    let need_main = needs_rebuild(&main_gltf, inputs);
    let need_cut = needs_rebuild(&cut_gltf, inputs);

    if !need_main && !need_cut {
        return;
    }

    if need_main {
        let scad_path = out_dir.join(format!("{stem}.scad"));
        let scad_src = spec_to_scad(spec, buildables);
        write_and_compile(&scad_path, &scad_src, &main_gltf);
    }

    if need_cut {
        let cut_scad_path = out_dir.join(format!("{stem}-cut-y-pos.scad"));
        let cut_src = spec_to_cut_scad(spec, buildables);
        write_and_compile(&cut_scad_path, &cut_src, &cut_gltf);
    }
}

fn write_and_compile(scad_path: &Path, scad_src: &str, gltf_out: &Path) {
    fs::write(scad_path, scad_src)
        .unwrap_or_else(|e| panic!("Failed to write {}: {e}", scad_path.display()));

    let tmp_stl = std::env::temp_dir().join("orchitecture_autotile.stl");

    let openscad = Command::new("openscad")
        .arg(scad_path)
        .arg("-o")
        .arg(&tmp_stl)
        .status();

    match openscad {
        Err(e) => {
            println!("cargo:warning=Could not run openscad (is it installed?): {e}");
            return;
        }
        Ok(s) if !s.success() => {
            println!(
                "cargo:warning=openscad failed (exit {s}) for {}",
                scad_path.display()
            );
            return;
        }
        Ok(_) => {}
    }

    let assimp = Command::new("assimp")
        .arg("export")
        .arg(&tmp_stl)
        .arg(gltf_out)
        .status();

    match assimp {
        Err(e) => println!("cargo:warning=Could not run assimp (is it installed?): {e}"),
        Ok(s) if !s.success() => println!(
            "cargo:warning=assimp failed (exit {s}) for {}",
            gltf_out.display()
        ),
        Ok(_) => {}
    }
}

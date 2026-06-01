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
/// Uses bare filenames — all source .scad files are copied into the output dir first.
fn spec_to_scad(spec: &MeshSpec) -> String {
    match spec {
        MeshSpec::Atom { name, rotation } => {
            let inc = format!("include <{name}.scad>");
            if *rotation == 0 {
                inc
            } else {
                // Rotation is in 90° units around the Y axis.
                // In OpenSCAD (Z-up), game-Y ≡ OpenSCAD-Z, so we rotate around Z.
                format!("rotate([0, 0, {}])\n{}", rotation, inc)
            }
        }
        MeshSpec::Union(a, b) => format!(
            "union() {{\n    {}\n    {}\n}}",
            spec_to_scad(a),
            spec_to_scad(b)
        ),
        MeshSpec::Intersection(a, b) => format!(
            "intersection() {{\n    {}\n    {}\n}}",
            spec_to_scad(a),
            spec_to_scad(b)
        ),
    }
}

/// Generate the cut-view variant: intersect the mesh with a jagged floor plane.
fn spec_to_cut_scad(spec: &MeshSpec) -> String {
    let inner = spec_to_scad(spec);
    // jagged.dat is copied into the output dir alongside the generated .scad files.
    format!(
        "intersection() {{\n    \
         {inner}\n    \
         union() {{\n        \
         surface(\"jagged.dat\");\n        \
         translate([0,0,-13])\n        \
         cube([13,13,13]);\n    \
         }}\n\
         }}"
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

/// Copy all .scad files and jagged.dat from `buildables/` into `out_dir` so that
/// OpenSCAD can resolve `include <>` and `surface()` calls without needing to
/// traverse upward past the working directory.
fn copy_scad_sources(buildables: &Path, out_dir: &Path) {
    let Ok(entries) = fs::read_dir(buildables) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let src = entry.path();
        let ext = src.extension().and_then(|s| s.to_str());
        let name = src.file_name().and_then(|s| s.to_str());
        let copy = ext == Some("scad") || name == Some("jagged.dat");
        if !copy {
            continue;
        }
        if let Some(fname) = src.file_name() {
            let dst = out_dir.join(fname);
            if let Err(e) = fs::copy(&src, &dst) {
                println!(
                    "cargo:warning=Failed to copy {} to {}: {e}",
                    src.display(),
                    dst.display()
                );
            }
        }
    }
}

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

    copy_scad_sources(buildables, out_dir);

    if need_main {
        let scad_path = out_dir.join(format!("{stem}.scad"));
        let scad_src = spec_to_scad(spec);
        write_and_compile(&scad_path, &scad_src, &main_gltf);
    }

    if need_cut {
        let cut_scad_path = out_dir.join(format!("{stem}-cut-y-pos.scad"));
        let cut_src = spec_to_cut_scad(spec);
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
        .output();

    match openscad {
        Err(e) => {
            println!("cargo:warning=Could not run openscad (is it installed?): {e}");
            return;
        }
        Ok(out) if !out.status.success() => {
            println!(
                "cargo:warning=openscad failed (exit {}) for {}",
                out.status,
                scad_path.display()
            );
            for line in String::from_utf8_lossy(&out.stderr).lines() {
                println!("cargo:warning=openscad stderr: {line}");
            }
            return;
        }
        Ok(_) => {}
    }

    let assimp = Command::new("assimp")
        .arg("export")
        .arg(&tmp_stl)
        .arg(gltf_out)
        .output();

    match assimp {
        Err(e) => println!("cargo:warning=Could not run assimp (is it installed?): {e}"),
        Ok(out) if !out.status.success() => {
            println!(
                "cargo:warning=assimp failed (exit {}) for {}",
                out.status,
                gltf_out.display()
            );
            for line in String::from_utf8_lossy(&out.stderr).lines() {
                println!("cargo:warning=assimp stderr: {line}");
            }
        }
        Ok(_) => {}
    }
}

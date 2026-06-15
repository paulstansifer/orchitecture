use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// Re-use the parser and types from the main crate. parser.rs only imports
// from std and anyhow, so it compiles cleanly as a build-dependency.
#[allow(dead_code, unused_imports, unexpected_cfgs)]
mod autotile {
    include!("src/autotile/parser.rs");
}

use autotile::{spec_stem, AutotileFile, AutotileResult, MeshSpec, UnorientedSlot};

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
    for path in &autotile_files {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let out_dir = buildables.join("autotile");
    fs::create_dir_all(&out_dir)
        .unwrap_or_else(|e| panic!("Failed to create {}: {e}", out_dir.display()));

    // Every top-level .scad source that something actually uses; anything left over
    // is reported as an orphan at the end.
    let mut referenced_scad: HashSet<PathBuf> = HashSet::new();

    // Parse all autotile files and collect specs that need to be generated.
    let mut spec_map: HashMap<String, (MeshSpec, UnorientedSlot)> = HashMap::new();
    for path in &autotile_files {
        let src = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));

        match autotile::parse(&src) {
            Ok(file) => collect_result_specs(&file, &mut spec_map),
            Err(e) => println!("cargo:warning=Parse error in {}: {e}", path.display()),
        }
    }

    for (name, (spec, slot)) in &spec_map {
        let scad_deps = collect_scad_deps(spec, &buildables);
        for dep in &scad_deps {
            println!("cargo:rerun-if-changed={}", dep.display());
            referenced_scad.insert(dep.clone());
        }

        let all_inputs: Vec<&Path> = autotile_files
            .iter()
            .map(PathBuf::as_path)
            .chain(scad_deps.iter().map(PathBuf::as_path))
            .collect();

        generate_if_needed(name, spec, *slot, &buildables, &out_dir, &all_inputs, true);
    }

    // Generate the fallback meshes for the structures in structures.ron.
    generate_structure_meshes(&buildables, &out_dir, &spec_map, &mut referenced_scad);

    // Now that we know every referenced .scad, warn about the ones nothing uses.
    warn_unreferenced_scad(&buildables, &referenced_scad);
}

// ─── Structure fallback meshes ──────────────────────────────────────────────────

/// A structure's mesh-relevant fields, hand-parsed from structures.ron (we only
/// need two of them, and the RON enum/Option fields don't survive `IgnoredAny`).
struct StructureMeshInfo {
    name: String,
    furniture: bool,
}

/// Minimal extractor for the `name` and `furniture` fields of each entry in
/// structures.ron. The file is a list of `( name: "...", ..., furniture: true )`
/// records; `furniture` is absent (false) for most entries.
fn parse_structure_infos(src: &str) -> Vec<StructureMeshInfo> {
    let mut out = Vec::new();
    let mut current: Option<StructureMeshInfo> = None;
    for line in src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name:") {
            if let Some(prev) = current.take() {
                out.push(prev);
            }
            // rest looks like `"desk",` — take the contents of the first "..." .
            let after_quote = rest.trim().trim_start_matches('"');
            let name = after_quote[..after_quote.find('"').unwrap_or(after_quote.len())].to_owned();
            current = Some(StructureMeshInfo {
                name,
                furniture: false,
            });
        } else if line.starts_with("furniture:") && line.contains("true") {
            if let Some(cur) = current.as_mut() {
                cur.furniture = true;
            }
        }
    }
    if let Some(prev) = current.take() {
        out.push(prev);
    }
    out
}

/// Generate `buildables/autotile/{stem}.gltf` (and a `-cut-y-pos` variant for
/// non-furniture) for every structure in structures.ron, where `stem` is the
/// structure name with spaces turned into underscores. Furniture vanishes in
/// cutaway, so it gets no cut mesh; everything else is cut. Structures with no
/// `{stem}.scad` (roof, column) are drawn entirely by the autotile rules and need
/// no standalone mesh.
fn generate_structure_meshes(
    buildables: &Path,
    out_dir: &Path,
    spec_map: &HashMap<String, (MeshSpec, UnorientedSlot)>,
    referenced_scad: &mut HashSet<PathBuf>,
) {
    let ron_path = buildables.join("structures.ron");
    println!("cargo:rerun-if-changed={}", ron_path.display());
    let src = match fs::read_to_string(&ron_path) {
        Ok(s) => s,
        Err(e) => {
            println!("cargo:warning=Failed to read {}: {e}", ron_path.display());
            return;
        }
    };

    for info in parse_structure_infos(&src) {
        let stem = info.name.replace(' ', "_");
        let scad = buildables.join(format!("{stem}.scad"));
        if !scad.exists() {
            continue;
        }
        println!("cargo:rerun-if-changed={}", scad.display());
        referenced_scad.insert(scad.clone());

        // If the autotile rules already emit this stem, they produce the same atom
        // mesh (and cut); don't regenerate it.
        if spec_map.contains_key(&stem) {
            continue;
        }

        // A trivial (unrotated) atom; slot only matters for rotated atoms.
        let spec = MeshSpec::Atom {
            name: stem.clone(),
            rotation: 0,
        };
        let inputs: Vec<&Path> = vec![ron_path.as_path(), scad.as_path()];
        generate_if_needed(
            &stem,
            &spec,
            UnorientedSlot::Room,
            buildables,
            out_dir,
            &inputs,
            !info.furniture,
        );
    }
}

// ─── Orphan .scad detection ─────────────────────────────────────────────────────

/// Targets of `include <...>` / `use <...>` directives in a .scad file.
fn scad_includes(path: &Path) -> Vec<String> {
    let Ok(src) = fs::read_to_string(path) else {
        return vec![];
    };
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim_start();
        let rest = line
            .strip_prefix("include")
            .or_else(|| line.strip_prefix("use"));
        let Some(rest) = rest.map(str::trim_start).and_then(|r| r.strip_prefix('<')) else {
            continue;
        };
        if let Some(end) = rest.find('>') {
            out.push(rest[..end].trim().to_owned());
        }
    }
    out
}

/// Warn about top-level `buildables/*.scad` files that no autotile rule or
/// structure references (directly or via `include`/`use`).
fn warn_unreferenced_scad(buildables: &Path, referenced: &HashSet<PathBuf>) {
    // Pull in transitive includes so a helper .scad reached only via `include`
    // from a referenced file isn't flagged.
    let mut referenced = referenced.clone();
    let mut queue: Vec<PathBuf> = referenced.iter().cloned().collect();
    while let Some(path) = queue.pop() {
        for inc in scad_includes(&path) {
            let inc_path = buildables.join(inc);
            if referenced.insert(inc_path.clone()) {
                queue.push(inc_path);
            }
        }
    }

    let Ok(entries) = fs::read_dir(buildables) else {
        return;
    };
    let mut orphans: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("scad"))
        .filter(|p| !referenced.contains(p))
        .collect();
    orphans.sort();
    for path in orphans {
        println!(
            "cargo:warning={} is not referenced by structures.autotile or structures.ron",
            path.display()
        );
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

fn collect_result_specs(
    file: &AutotileFile,
    map: &mut HashMap<String, (MeshSpec, UnorientedSlot)>,
) {
    for rule in &file.rules {
        let slot = rule.slot;
        for case in &rule.cases {
            if let AutotileResult::Mesh { spec, .. } = &case.result {
                map.insert(spec_stem(spec, slot), (spec.clone(), slot));
            }
        }
    }
}

fn is_trivial(spec: &MeshSpec) -> bool {
    matches!(spec, MeshSpec::Atom { rotation: 0, .. })
    // Rotation is a runtime-only variant; build.rs never produces it.
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
        MeshSpec::Rotation(_, _) => unreachable!("Rotation is a runtime-only variant"),
    }
}

// ─── OpenSCAD code generation ─────────────────────────────────────────────────

/// Returns (pre_translate, post_translate) pivot strings for rotating around the
/// correct centre point for a given slot.
fn pivot_for_slot(slot: UnorientedSlot) -> (&'static str, &'static str) {
    match slot {
        UnorientedSlot::Wall => ("[-0.5, 0, 0]", "[0.5, 0, 0]"),
        _ => ("[-0.5, -0.5, 0]", "[0.5, 0.5, 0]"),
    }
}

/// Generate OpenSCAD source for a mesh spec.
/// Uses bare filenames — all source .scad files are copied into the output dir first.
fn spec_to_scad(spec: &MeshSpec, slot: UnorientedSlot) -> String {
    match spec {
        MeshSpec::Atom { name, rotation } => {
            let inc = format!("include <{name}.scad>");
            if *rotation == 0 {
                inc
            } else {
                // Rotation is in degrees around Z (OpenSCAD Z-up, game-Y ≡ OpenSCAD-Z).
                // Translate so the pivot lands at the origin before rotating, then back.
                let (pre, post) = pivot_for_slot(slot);
                format!("translate({post})\nrotate([0, 0, {rotation}])\ntranslate({pre})\n{inc}")
            }
        }
        MeshSpec::Union(a, b) => format!(
            "union() {{\n    {}\n    {}\n}}",
            spec_to_scad(a, slot),
            spec_to_scad(b, slot)
        ),
        MeshSpec::Intersection(a, b) => format!(
            "intersection() {{\n    {}\n    {}\n}}",
            spec_to_scad(a, slot),
            spec_to_scad(b, slot)
        ),
        MeshSpec::Rotation(_, _) => unreachable!("Rotation is a runtime-only variant"),
    }
}

/// Generate the cut-view variant: intersect the mesh with a jagged floor plane.
fn spec_to_cut_scad(spec: &MeshSpec, slot: UnorientedSlot) -> String {
    let inner = spec_to_scad(spec, slot);
    // jagged.dat is copied into the output dir alongside the generated .scad files.
    format!(
        "intersection() {{\n    \
         {inner}\n    \
         translate([-.15,-.15,.25])\n \
         scale([1/10,1/10, 1/13])\n \
         union() {{\n \
             surface(\"jagged.dat\");\n \
             translate([0,0,-13])\n \
             cube([13,13,13]);\n \
         }}\n \
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
    slot: UnorientedSlot,
    buildables: &Path,
    out_dir: &Path,
    inputs: &[&Path],
    want_cut: bool,
) {
    let main_gltf = out_dir.join(format!("{stem}.gltf"));
    let cut_gltf = out_dir.join(format!("{stem}-cut-y-pos.gltf"));

    if !want_cut {
        // Furniture and other cut-less meshes: make sure no stale cut mesh lingers.
        let _ = fs::remove_file(&cut_gltf);
    }

    let need_main = needs_rebuild(&main_gltf, inputs);
    let need_cut = want_cut && needs_rebuild(&cut_gltf, inputs);

    if !need_main && !need_cut {
        return;
    }

    copy_scad_sources(buildables, out_dir);

    if need_main {
        let scad_path = out_dir.join(format!("{stem}.scad"));
        if is_trivial(spec) {
            // The atom source was already copied into out_dir; compile it directly
            // to avoid writing "include <stem.scad>" into stem.scad (circular).
            compile_scad(&scad_path, &main_gltf);
        } else {
            let scad_src = spec_to_scad(spec, slot);
            write_and_compile(&scad_path, &scad_src, &main_gltf);
        }
    }

    if need_cut {
        let cut_scad_path = out_dir.join(format!("{stem}-cut-y-pos.scad"));
        let cut_src = spec_to_cut_scad(spec, slot);
        write_and_compile(&cut_scad_path, &cut_src, &cut_gltf);
    }
}

fn compile_scad(scad_path: &Path, gltf_out: &Path) {
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
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("Current top level object is empty.") {
                // Empty geometry is a valid signal; write a sentinel empty .gltf.
                let _ = fs::write(gltf_out, "");
                return;
            }
            println!(
                "cargo:warning=openscad failed (exit {}) for {}",
                out.status,
                scad_path.display()
            );
            for line in stderr.lines() {
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

fn write_and_compile(scad_path: &Path, scad_src: &str, gltf_out: &Path) {
    fs::write(scad_path, scad_src)
        .unwrap_or_else(|e| panic!("Failed to write {}: {e}", scad_path.display()));
    compile_scad(scad_path, gltf_out);
}

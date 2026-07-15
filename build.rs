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

use autotile::{spec_stem, AutotileFile, AutotiledMeshes, MeshSpec, UnorientedSlot};

fn main() {
    // Register and set `autotile_matching` so the main crate can gate
    // the bevy/crate-dependent matching code that build.rs doesn't need.
    println!("cargo::rustc-check-cfg=cfg(autotile_matching)");
    println!("cargo:rustc-cfg=autotile_matching");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    convert_sprites(&manifest);

    let buildables = manifest.join("buildables");

    // Re-run if any autotile file or atom scad file changes.
    println!("cargo:rerun-if-changed=buildables");

    let autotile_files = collect_autotile_files(&buildables);
    for path in &autotile_files {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    // Wings3D has no headless export, so every buildables/*.wings must have a
    // hand-exported {stem}.glb sitting next to it. Missing exports fail the build;
    // stale ones (older than the .wings source) just warn.
    let wings_files = collect_wings_files(&buildables);
    let wings_stems = check_wings_exports(&wings_files);

    let out_dir = manifest.join("assets/generated/autotile");
    fs::create_dir_all(&out_dir)
        .unwrap_or_else(|e| panic!("Failed to create {}: {e}", out_dir.display()));

    let atomic_out_dir = out_dir.join("atomic");
    fs::create_dir_all(&atomic_out_dir)
        .unwrap_or_else(|e| panic!("Failed to create {}: {e}", atomic_out_dir.display()));

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
        let mut wings_atoms = Vec::new();
        collect_wings_atoms(spec, &wings_stems, &mut wings_atoms);

        if !wings_atoms.is_empty() {
            // Wings3D-sourced meshes are opaque pre-baked glTF: there's no OpenSCAD
            // source to re-run with a rotate()/translate() wrapper or CSG operator,
            // so only a bare, untransformed atom reference is supported.
            let plain_wings_atom = match spec {
                MeshSpec::Atom {
                    name: atom_name,
                    rotation: 0,
                    translation: None,
                } if wings_stems.contains(atom_name) => Some(atom_name.clone()),
                _ => None,
            };

            let Some(atom_name) = plain_wings_atom else {
                panic!(
                    "structures.autotile: the mesh spec for \"{name}\" uses {} in a rotated, \
                     translated, union, or intersection expression, but Wings3D-sourced meshes \
                     don't support baked rotation/translation or CSG (there's no OpenSCAD \
                     source to recompile). Reference {} directly, with no `:rotation`, \
                     `:translation`, `+`, or `*`.",
                    wings_atoms.join(", "),
                    if wings_atoms.len() == 1 { "it" } else { "them" }
                );
            };

            let glb = buildables.join(format!("{atom_name}.glb"));
            let main_gltf = out_dir.join(format!("{name}.gltf"));
            copy_glb_if_needed(&glb, &main_gltf);
            // No CSG support means no cut-away variant either.
            let cut_gltf = out_dir.join(format!("{name}-cut-y-pos.gltf"));
            let _ = fs::remove_file(&cut_gltf);
            continue;
        }

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

    // Generate the fallback meshes for the structures in elements.ron/furniture.ron.
    generate_structure_meshes(
        &buildables,
        &out_dir,
        &spec_map,
        &mut referenced_scad,
        &wings_stems,
    );

    // Generate atomic meshes in the atomic/ subdirectory.
    generate_atomic_meshes(&buildables, &atomic_out_dir, &mut referenced_scad);
    generate_wings_atomic_meshes(&atomic_out_dir, &wings_files, &buildables);

    // Generate a JSON file mapping structures to their category (elements vs furniture).
    generate_structure_categories(&buildables, &out_dir);

    // Now that we know every referenced .scad, warn about the ones nothing uses.
    warn_unreferenced_scad(&buildables, &referenced_scad);
}

// ─── Atomic mesh generation ────────────────────────────────────────────────

/// Generate .gltf files for all atomic meshes (input .scad files to CSG operations)
/// in the atomic/ subdirectory.
fn generate_atomic_meshes(
    buildables: &Path,
    atomic_out_dir: &Path,
    referenced_scad: &mut HashSet<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(buildables) else {
        return;
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some("scad") = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };

        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };

        // Skip utility meshes (starting with u_)
        if stem.starts_with("u_") {
            continue;
        }

        // Skip the autotile directory
        if path
            .parent()
            .map(|p| p.ends_with("autotile"))
            .unwrap_or(false)
        {
            continue;
        }

        referenced_scad.insert(path.clone());

        // Generate unrotated atom
        let spec = MeshSpec::Atom {
            name: stem.to_string(),
            rotation: 0,
            translation: None,
        };

        let inputs: Vec<&Path> = vec![path.as_path()];
        generate_if_needed(
            stem,
            &spec,
            UnorientedSlot::Room,
            buildables,
            atomic_out_dir,
            &inputs,
            false, // no cut mesh for atomic
        );
    }
}

// ─── Eorf fallback meshes ──────────────────────────────────────────────────

/// A structure's mesh-relevant fields, hand-parsed from elements.ron/furniture.ron
/// (we only need the name, and the RON enum/Option fields don't survive `IgnoredAny`).
struct StructureMeshInfo {
    name: String,
    furniture: bool,
}

/// Minimal extractor for the `name` field of each entry in a `( name: "...", ... )`
/// list file (elements.ron or furniture.ron). `furniture` is set uniformly per
/// call, since the file itself (not a per-entry field) now determines it.
fn parse_structure_infos(src: &str, furniture: bool) -> Vec<StructureMeshInfo> {
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name:") {
            // rest looks like `"desk",` — take the contents of the first "..." .
            let after_quote = rest.trim().trim_start_matches('"');
            let name = after_quote[..after_quote.find('"').unwrap_or(after_quote.len())].to_owned();
            out.push(StructureMeshInfo { name, furniture });
        }
    }
    out
}

/// Extract all atomic mesh names from a MeshSpec recursively.
fn extract_atomic_meshes(spec: &MeshSpec, meshes: &mut Vec<String>) {
    match spec {
        MeshSpec::Atom { name, .. } => {
            if !meshes.contains(name) {
                meshes.push(name.clone());
            }
        }
        MeshSpec::Union(a, b) | MeshSpec::Intersection(a, b) => {
            extract_atomic_meshes(a, meshes);
            extract_atomic_meshes(b, meshes);
        }
        MeshSpec::Rotation(_angle, inner) => {
            extract_atomic_meshes(inner, meshes);
        }
    }
}

/// Extract atomic mesh inputs from autotile rules, organized by structure name.
fn extract_structure_atomic_meshes(
    file: &autotile::AutotileFile<autotile::AutotiledMeshes>,
) -> HashMap<String, Vec<String>> {
    let mut mapping: HashMap<String, Vec<String>> = HashMap::new();

    for rule in &file.rules {
        let meshes = mapping
            .entry(rule.structure_name.clone())
            .or_insert_with(Vec::new);

        for case in &rule.cases {
            if let autotile::AutotiledMeshes::Mesh { spec } = &case.result {
                let mut atomic_names = Vec::new();
                extract_atomic_meshes(spec, &mut atomic_names);
                for name in atomic_names {
                    if !meshes.contains(&name) {
                        meshes.push(name);
                    }
                }
            }
        }
    }

    mapping
}

/// Generate a JSON file mapping structure names AND their component meshes to their
/// category (elements or furniture). This ensures that structures like "column"
/// which are composed of "column_caps", "column_middle", "column_floating" all get
/// categorized together.
fn generate_structure_categories(buildables: &Path, out_dir: &Path) {
    let elements_path = buildables.join("elements.ron");
    let furniture_path = buildables.join("furniture.ron");
    let autotile_path = buildables.join("structures.autotile");

    // Parse elements and furniture structure names
    let mut element_names: Vec<String> = Vec::new();
    let mut furniture_names: Vec<String> = Vec::new();

    if let Ok(src) = fs::read_to_string(&elements_path) {
        for info in parse_structure_infos(&src, false) {
            element_names.push(info.name);
        }
    }

    if let Ok(src) = fs::read_to_string(&furniture_path) {
        for info in parse_structure_infos(&src, true) {
            furniture_names.push(info.name);
        }
    }

    let mut categories: HashMap<String, String> = HashMap::new();

    // If we can parse the autotile file, use it to find atomic mesh inputs
    if let Ok(src) = fs::read_to_string(&autotile_path) {
        if let Ok(file) = autotile::parse(&src) {
            let structure_atomic_meshes = extract_structure_atomic_meshes(&file);

            // Map element structures and their atomic mesh inputs
            for structure_name in &element_names {
                if !structure_name.starts_with("u_") {
                    categories.insert(structure_name.clone(), "elements".to_string());
                }
                // Also map any atomic meshes used by this structure
                if let Some(meshes) = structure_atomic_meshes.get(structure_name) {
                    for mesh in meshes {
                        if !mesh.starts_with("u_") {
                            categories.insert(mesh.clone(), "elements".to_string());
                        }
                    }
                }
            }

            // Map furniture structures and their atomic mesh inputs
            for structure_name in &furniture_names {
                if !structure_name.starts_with("u_") {
                    categories.insert(structure_name.clone(), "furniture".to_string());
                }
                // Also map any atomic meshes used by this structure
                if let Some(meshes) = structure_atomic_meshes.get(structure_name) {
                    for mesh in meshes {
                        if !mesh.starts_with("u_") {
                            categories.insert(mesh.clone(), "furniture".to_string());
                        }
                    }
                }
            }
        }
    } else {
        // Fallback: just map the structure names if we can't parse autotile
        for name in &element_names {
            if !name.starts_with("u_") {
                categories.insert(name.clone(), "elements".to_string());
            }
        }
        for name in &furniture_names {
            if !name.starts_with("u_") {
                categories.insert(name.clone(), "furniture".to_string());
            }
        }
    }

    // Generate JSON
    let mut json = String::from("{\n");
    let mut first = true;

    for (name, category) in &categories {
        if !first {
            json.push_str(",\n");
        }
        first = false;
        json.push_str(&format!("  \"{}\": \"{}\"", name, category));
    }

    json.push_str("\n}\n");

    // Write the file
    let output_path = out_dir.join("structure_categories.json");
    let _ = fs::write(&output_path, json);
}

/// Generate `buildables/autotile/{stem}.gltf` (and a `-cut-y-pos` variant for
/// non-furniture) for every structure in elements.ron/furniture.ron, where `stem`
/// is the structure name with spaces turned into underscores. Furniture vanishes
/// in cutaway, so it gets no cut mesh; everything else is cut. Structures with no
/// `{stem}.scad` (roof, column) are drawn entirely by the autotile rules and need
/// no standalone mesh.
fn generate_structure_meshes(
    buildables: &Path,
    out_dir: &Path,
    spec_map: &HashMap<String, (MeshSpec, UnorientedSlot)>,
    referenced_scad: &mut HashSet<PathBuf>,
    wings_stems: &HashSet<String>,
) {
    let elements_path = buildables.join("elements.ron");
    let furniture_path = buildables.join("furniture.ron");
    println!("cargo:rerun-if-changed={}", elements_path.display());
    println!("cargo:rerun-if-changed={}", furniture_path.display());

    let mut infos = Vec::new();
    for (path, furniture) in [(&elements_path, false), (&furniture_path, true)] {
        match fs::read_to_string(path) {
            Ok(src) => infos.extend(parse_structure_infos(&src, furniture)),
            Err(e) => println!("cargo:warning=Failed to read {}: {e}", path.display()),
        }
    }

    for info in infos {
        let stem = info.name.replace(' ', "_");
        let scad = buildables.join(format!("{stem}.scad"));

        // If the autotile rules already emit this stem, they produce the same atom
        // mesh (and cut); don't regenerate it.
        if spec_map.contains_key(&stem) {
            continue;
        }

        if scad.exists() {
            println!("cargo:rerun-if-changed={}", scad.display());
            referenced_scad.insert(scad.clone());

            // A trivial (unrotated) atom; slot only matters for rotated atoms.
            let spec = MeshSpec::Atom {
                name: stem.clone(),
                rotation: 0,
                translation: None,
            };
            let inputs: Vec<&Path> = vec![
                elements_path.as_path(),
                furniture_path.as_path(),
                scad.as_path(),
            ];
            generate_if_needed(
                &stem,
                &spec,
                UnorientedSlot::Room,
                buildables,
                out_dir,
                &inputs,
                !info.furniture,
            );
        } else if wings_stems.contains(&stem) {
            // No CSG support for Wings3D-sourced meshes means no cut-away variant.
            let glb = buildables.join(format!("{stem}.glb"));
            let main_gltf = out_dir.join(format!("{stem}.gltf"));
            let cut_gltf = out_dir.join(format!("{stem}-cut-y-pos.gltf"));
            let _ = fs::remove_file(&cut_gltf);
            copy_glb_if_needed(&glb, &main_gltf);
        }
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
            "cargo:warning={} is not referenced by structures.autotile, elements.ron, or furniture.ron",
            path.display()
        );
    }
}

// ─── File discovery ───────────────────────────────────────────────────────────

/// Only `structures.autotile` holds mesh-selection rules; other `*.autotile` files (e.g.
/// `motifs.autotile`) use a different result grammar and aren't mesh manifests.
fn collect_autotile_files(dir: &Path) -> Vec<PathBuf> {
    let path = dir.join("structures.autotile");
    if path.exists() {
        vec![path]
    } else {
        vec![]
    }
}

fn collect_wings_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return vec![];
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("wings"))
        .collect();
    paths.sort(); // deterministic order
    paths
}

// ─── Wings3D export freshness ───────────────────────────────────────────────

/// Wings3D has no headless/scriptable export, so every `buildables/*.wings` file
/// needs a hand-exported `{stem}.glb` sitting next to it (File > Export Selected/All
/// > glTF Binary in the Wings3D GUI). A missing export fails the build outright,
/// since there's nothing we could generate in its place; a stale one (older than
/// the .wings source, meaning it predates the last edit) only warns, since it may
/// still be an intentional/usable export.
///
/// Returns the stems of all `.wings` files (all of which are confirmed to have a
/// `.glb` by the time this returns, since a missing one panics).
fn check_wings_exports(wings_files: &[PathBuf]) -> HashSet<String> {
    let mut stems = HashSet::new();
    let mut missing = Vec::new();

    for wings in wings_files {
        println!("cargo:rerun-if-changed={}", wings.display());
        let stem = wings
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap()
            .to_owned();
        let glb = wings.with_extension("glb");
        println!("cargo:rerun-if-changed={}", glb.display());

        match fs::metadata(&glb) {
            Err(_) => missing.push(glb),
            Ok(glb_meta) => {
                let wings_meta = fs::metadata(wings)
                    .unwrap_or_else(|e| panic!("Failed to stat {}: {e}", wings.display()));
                if let (Ok(glb_time), Ok(wings_time)) = (glb_meta.modified(), wings_meta.modified())
                {
                    if glb_time < wings_time {
                        println!(
                            "cargo:warning={} is older than {} — re-export it from Wings3D \
                             (File > Export Selected/All > glTF Binary)",
                            glb.display(),
                            wings.display()
                        );
                    }
                }
                stems.insert(stem);
            }
        }
    }

    if !missing.is_empty() {
        let list = missing
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        panic!(
            "Missing Wings3D export(s): {list}. Wings3D has no headless export, so export each \
             .wings file to a matching .glb by hand in the Wings3D GUI (File > Export Selected/All \
             > glTF Binary) and commit the .glb alongside the .wings source."
        );
    }

    stems
}

/// Copy a hand-exported `.glb` to a generated `.gltf` path if the `.glb` is newer.
/// Bevy's glTF loader auto-detects binary vs. text glTF from the file's magic bytes,
/// not its extension, so a `.glb`'s bytes work fine at a `.gltf`-suffixed path.
fn copy_glb_if_needed(glb: &Path, gltf_out: &Path) {
    if !needs_rebuild(gltf_out, &[glb]) {
        return;
    }
    if let Err(e) = fs::copy(glb, gltf_out) {
        println!(
            "cargo:warning=Failed to copy {} to {}: {e}",
            glb.display(),
            gltf_out.display()
        );
    }
}

/// Generate `.gltf` files (by copying the hand-exported `.glb`) for all Wings3D
/// atomic meshes, in the atomic/ subdirectory — mirrors `generate_atomic_meshes`
/// for `.scad` files.
fn generate_wings_atomic_meshes(atomic_out_dir: &Path, wings_files: &[PathBuf], buildables: &Path) {
    for wings in wings_files {
        let stem = wings.file_stem().and_then(|s| s.to_str()).unwrap();
        if stem.starts_with("u_") {
            continue;
        }
        let glb = buildables.join(format!("{stem}.glb"));
        let out = atomic_out_dir.join(format!("{stem}.gltf"));
        copy_glb_if_needed(&glb, &out);
    }
}

/// Wings3D-backed atom names referenced anywhere in `spec` (deduplicated).
fn collect_wings_atoms(spec: &MeshSpec, wings_stems: &HashSet<String>, found: &mut Vec<String>) {
    match spec {
        MeshSpec::Atom { name, .. } => {
            if wings_stems.contains(name) && !found.contains(name) {
                found.push(name.clone());
            }
        }
        MeshSpec::Union(a, b) | MeshSpec::Intersection(a, b) => {
            collect_wings_atoms(a, wings_stems, found);
            collect_wings_atoms(b, wings_stems, found);
        }
        MeshSpec::Rotation(_, inner) => collect_wings_atoms(inner, wings_stems, found),
    }
}

// ─── Spec collection ──────────────────────────────────────────────────────────

fn collect_result_specs(
    file: &AutotileFile<AutotiledMeshes>,
    map: &mut HashMap<String, (MeshSpec, UnorientedSlot)>,
) {
    for rule in &file.rules {
        let slot = rule.slot;
        for case in &rule.cases {
            if let AutotiledMeshes::Mesh { spec, .. } = &case.result {
                map.insert(spec_stem(spec, slot), (spec.clone(), slot));
            }
        }
    }
}

fn is_trivial(spec: &MeshSpec) -> bool {
    matches!(
        spec,
        MeshSpec::Atom {
            rotation: 0,
            translation: None,
            ..
        }
    )
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
        MeshSpec::Atom {
            name,
            rotation,
            translation,
        } => {
            let inc = format!("include <{name}.scad>");
            // Apply baked rotation first (innermost), then translation (outermost).
            // In OpenSCAD, transforms are right-to-left: the last line in the source
            // is the first applied to the geometry.
            let rotated = if *rotation == 0 {
                inc
            } else {
                // Rotation is in degrees around Z (OpenSCAD Z-up, game-Y ≡ OpenSCAD-Z).
                // Translate so the pivot lands at the origin before rotating, then back.
                let (pre, post) = pivot_for_slot(slot);
                format!("translate({post})\nrotate([0, 0, {rotation}])\ntranslate({pre})\n{inc}")
            };
            if let Some((dx, dy, dz)) = translation {
                // Game (dx, dy, dz) → OpenSCAD [dx, -dz, dy]
                // (OpenSCAD is Z-up; game Y-up ≡ OpenSCAD Z, game Z ≡ OpenSCAD -Y)
                format!("translate([{dx}, {neg_dz}, {dy}])\n{rotated}", neg_dz = -dz)
            } else {
                rotated
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

    // OpenSCAD is Z-up; the game (and every mesh format it loads, including
    // Wings3D-sourced glTF) is Y-up. Apply the correction here, as the outermost
    // (last-applied) transform, so every OpenSCAD-derived asset comes out of the
    // build already in game coordinates — including "plain" atoms referenced
    // directly by elements.ron/furniture.ron/structures.autotile with no baked
    // rotation/translation of their own.
    let out_dir = scad_path.parent().unwrap();
    let target_name = scad_path.file_name().unwrap().to_str().unwrap();
    let wrapper_path = out_dir.join(format!("{target_name}.yup.scad"));
    let wrapper_src = format!("rotate([-90, 0, 0]) {{\n    include <{target_name}>\n}}\n");
    fs::write(&wrapper_path, &wrapper_src)
        .unwrap_or_else(|e| panic!("Failed to write {}: {e}", wrapper_path.display()));

    let openscad = Command::new("openscad")
        .arg(&wrapper_path)
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

const SPRITE_SIZES: &[(&str, u32, u32)] = &[("24x18", 24, 18), ("16x12", 16, 12)];

fn convert_sprites(manifest: &Path) {
    let sprites_dir = manifest.join("sprites");
    println!("cargo:rerun-if-changed=sprites");
    let entries = match fs::read_dir(&sprites_dir) {
        Ok(e) => e,
        Err(e) => {
            println!("cargo:warning=Could not read sprites/: {e}");
            return;
        }
    };
    let svgs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("svg"))
        .collect();

    for &(dir_name, w, h) in SPRITE_SIZES {
        let out_dir = manifest.join("assets/generated/sprites").join(dir_name);
        fs::create_dir_all(&out_dir)
            .unwrap_or_else(|e| panic!("Failed to create {}: {e}", out_dir.display()));
        for path in &svgs {
            let stem = path.file_stem().unwrap().to_str().unwrap();
            let out = out_dir.join(format!("{stem}.png"));
            println!("cargo:rerun-if-changed={}", out.display());
            match Command::new("rsvg-convert")
                .args(["-w", &w.to_string(), "-h", &h.to_string()])
                .arg(path)
                .arg("-o")
                .arg(&out)
                .status()
            {
                Err(e) => {
                    println!("cargo:warning=rsvg-convert not found (skipping sprites): {e}")
                }
                Ok(s) if !s.success() => {
                    println!("cargo:warning=rsvg-convert failed for {}", path.display())
                }
                Ok(_) => {}
            }
        }
    }
}

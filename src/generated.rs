//! Facts about the build's generated assets, recorded by `build.rs` at compile
//! time. The game must never look at `buildables/` or `assets/generated/` on
//! disk at runtime: the web build has no filesystem, so such checks silently
//! come back "missing" there.

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

/// True if `stem`'s mesh comes from a hand-modeled `buildables/{stem}.wings`
/// export rather than a `{stem}.scad` compile.
pub fn is_wings_sourced(stem: &str) -> bool {
    WINGS_STEMS.contains(&stem)
}

/// Whether `assets/generated/autotile/{name}.gltf` exists, and if so whether
/// it has content: `Some(false)` is an empty sentinel (an intentionally
/// invisible cut variant).
pub fn autotile_gltf(name: &str) -> Option<bool> {
    AUTOTILE_GLTFS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|&(_, non_empty)| non_empty)
}

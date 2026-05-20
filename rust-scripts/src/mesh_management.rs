use std::collections::HashMap;

use bevy::asset::{AssetServer, Handle};
use bevy::scene::Scene;

/// Load all .gltf files from the `buildables/` directory via Bevy's AssetServer.
/// The asset root is expected to be the project root (parent of `rust-scripts/`).
pub fn load_mesh_handles(asset_server: &AssetServer) -> HashMap<String, Handle<Scene>> {
    let mut handles = HashMap::new();

    let buildables_path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../buildables"));
    let dir = match std::fs::read_dir(buildables_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Could not read buildables/: {e}");
            return handles;
        }
    };

    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("gltf") {
            if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                let asset_path = format!("buildables/{filename}#Scene0");
                let handle: Handle<Scene> = asset_server.load(&asset_path);
                handles.insert(filename.to_string(), handle);
            }
        }
    }

    handles
}

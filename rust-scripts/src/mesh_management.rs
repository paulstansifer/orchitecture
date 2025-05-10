use std::collections::HashMap;

use godot::{
    classes::{GltfDocument, GltfState, Mesh, MeshInstance3D},
    global::Error,
    prelude::*,
};

#[derive(PartialEq, Eq, Hash, Clone, Copy, PartialOrd, Ord, Debug)]
pub struct MeshId(usize);

pub fn load_meshes() -> HashMap<String, Gd<Mesh>> {
    let mut meshes: HashMap<String, Gd<Mesh>> = HashMap::new();

    let mut dir = godot::classes::DirAccess::open("res://buildables/").unwrap();
    dir.list_dir_begin();
    let mut filename = dir.get_next();
    while !filename.is_empty() {
        if filename.ends_with(".gltf") {
            let path = format!("res://buildables/{}", filename);

            let mut gltf_document_load = GltfDocument::new_gd();
            let gltf_state = GltfState::new_gd();
            if gltf_document_load.append_from_file(&path, &gltf_state) == Error::OK {
                let root_node = gltf_document_load.generate_scene(&gltf_state).unwrap();
                let mi: Gd<MeshInstance3D> =
                    root_node.get_child(0).unwrap().get_child(0).unwrap().cast();

                meshes.insert(filename.to_string(), mi.get_mesh().unwrap());
            }
        }
        filename = dir.get_next();
    }

    meshes
}

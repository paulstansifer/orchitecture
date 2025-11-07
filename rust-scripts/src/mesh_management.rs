use std::collections::HashMap;

use godot::{
    classes::{GltfDocument, GltfState, ImporterMeshInstance3D, Mesh, MeshInstance3D},
    global::Error,
    prelude::*,
};

// #[derive(PartialEq, Eq, Hash, Clone, Copy, PartialOrd, Ord, Debug)]
// pub struct MeshId(usize);

pub fn load_meshes() -> HashMap<String, Gd<Mesh>> {
    let mut meshes: HashMap<String, Gd<Mesh>> = HashMap::new();

    let mut dir = godot::classes::DirAccess::open("res://buildables/").unwrap();
    dir.list_dir_begin();
    let mut filename = dir.get_next();
    while !filename.is_empty() {
        if filename.ends_with(".gltf") {
            let path = format!("res://buildables/{}", filename);
            // godot_print!("### Loading {}", path);

            let mut gltf_document_load = GltfDocument::new_gd();
            let gltf_state = GltfState::new_gd();
            if gltf_document_load.append_from_file(&path, &gltf_state) == Error::OK {
                let root_node = gltf_document_load.generate_scene(&gltf_state).unwrap();
                // godot_print!(
                //     "### {:?} --> {:?}",
                //     root_node.get_child(0).unwrap().get_child(0).unwrap(),
                //     root_node
                //         .get_child(0)
                //         .unwrap()
                //         .get_child(0)
                //         .unwrap()
                //         .get_children()
                // );

                let mi_node: Gd<Node> = root_node.get_child(0).unwrap().get_child(0).unwrap();

                let mesh = if mi_node.is_class("MeshInstance3D") {
                    mi_node.cast::<MeshInstance3D>().get_mesh().unwrap()
                } else if mi_node.is_class("ImporterMeshInstance3D") {
                    mi_node
                        .cast::<ImporterMeshInstance3D>()
                        .get_mesh()
                        .unwrap()
                        .get_mesh()
                        .unwrap()
                        .upcast()
                } else {
                    panic!("Oh dear");
                };

                meshes.insert(filename.to_string(), mesh);
            }
        }
        filename = dir.get_next();
    }

    meshes
}

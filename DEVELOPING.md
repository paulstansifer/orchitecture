
# Orchitecture

Orchitecture uses the Bevy game engine, and uses Burn to the convolutional neural nets that evaluate strucutres.

# Code map

Game parts:
  * main.rs: initialization
  * camera.rs: 3rd-person camera
  * ui.rs: UI
  * world.rs: ground and exterior lighting
  * ceiling_lights.rs: Adds lighting inside a `WallGrid`

`WallGrid` and related concepts:
  * wall_grid.rs: `WallGrid` represents the world as `Sparse3d<Cell>`.
    Core types (`Cell`, `VantageEvaluation`), grid-mutation methods (drag/click/undo),
    `apply_changes` (syncs ECS entities to grid state), and `cell_transform`.
  * visibility.rs: hides parts of the `WallGrid` so we can see inside.
  * sparse3d.rs: storage for walls, ceilings and "room objects" on a sparse cubic grid
  * structure.rs: `Structure`s are the walls, doors, desks, etc. that occupy `Cell`s.
  * serialization.rs: text format for `Sparse3D<Cell>`

Quality neural net (`qnn/` module):
  * qnn/train.rs: training binary entry point (`fn main`) and training loop
  * qnn/model.rs: `Cnn` model architecture and `Args` hyperparameters
  * qnn/translate.rs: convert `Sparse3d` data into voxels the QNN can use
  * qnn/adapter.rs: run the QNN (actually used in the game itself)

Not very important, for non-user-constructed buildings:
  * build_helpers.rs: tools for rapidly defining structure parts
  * example_structures.rs: some examples
  * llm_rooms.rs: LLMs tried to generate some rooms with build_helper.rs

# Assets

The checked-in .gtlf files are programmatically-generated from the OpenSCAD files (eventually, this should be part of the build process). To regenerate them:
```
for f in buildables/*.scad; do dest="$(echo "$f" | sed 's/.scad/.gltf/')"; openscad "$f" -o /tmp/tmp.stl && assimp export /tmp/tmp.stl "$dest"; done;
```

(You need to do `sudo apt install openscad assimp-utils` to get those programs.)
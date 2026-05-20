# Code Organization Thoughts

## Current module map

```
main.rs              game binary entry point; setup() + update_visibility_system()
lib.rs               barrel of pub mods
camera.rs            CameraState, GameCamera, camera_input_system
input.rs             BuildState, WallCursorMarker, RoomCursorMarker,
                       building_input_system, cursor_system, apply_changes,
                       cursor_world_pos (private)
ui.rs                UiState, discover_training_files, ui_system
wall_grid.rs         WallGrid, Cell, OfflineCell, compute_visibility,
                       cell_transform, MODELS thread-local
structure.rs         StructureInfo, StructureEmbedding, PlacementStyle,
                       Structure, StructureList, load_structure_info
mesh_management.rs   load_mesh_handles
sparse3d.rs          Sparse3D, SlotLocation, RelSlot, Facing, Slot, Rotateable
qnn_adapter.rs       ModelHolder
qnn.rs               training loop
qnn_translate.rs     sparse3d_to_tensor
serialization.rs     serialize/deserialize Sparse3D to text
build_helpers.rs     helper fns for constructing Sparse3D programmatically
example_structures.rs  hardcoded example layouts
llm_rooms.rs         LLM-generated room layouts
```

---

## Issues

### 1. `update_visibility_system` is stranded in `main.rs`

This is a meaty system that operates on `WallGrid` data and spawns/despawns
entities. It belongs alongside the rest of the WallGrid-related ECS logic, not
in the binary. `main.rs` should ideally just wire up the `App` and delegate
everything else.

**Proposal:** Move `update_visibility_system` into `wall_grid.rs` (or a new
`visibility.rs`), and expose it from the library so `main.rs` just registers it.

### 2. `apply_changes` is in `input.rs` but belongs to the WallGrid layer

`apply_changes` takes WallGrid cell-change deltas and reflects them in the ECS
(despawn/spawn). Both `input.rs` and `ui.rs` import it from `input.rs`, which
creates an odd dependency: the UI layer depends on the input layer for a utility
that is really part of the grid layer.

**Proposal:** Move `apply_changes` (and probably `cell_transform` which it
transitively needs) into `wall_grid.rs`. They're both just "how to turn a
`SlotLocation + Cell` into a Bevy entity."

### 3. `cursor_world_pos` is private but needed in two places

It lives in `input.rs` as a private function. When `update_visibility_system`
needed the cursor world position we had to route it through `BuildState.focus_pos`
as a workaround. If it were `pub(crate)`, `update_visibility_system` could call
it directly and the `focus_pos` field could go away.

**Proposal:** Make `cursor_world_pos` `pub(crate)` (or just `pub`).

### 4. ML inference lives inside `WallGrid`

`WallGrid::metrics_at` calls `MODELS` (a thread-local) and `qnn_translate`.
This means `wall_grid.rs` depends on the ML stack, which is a heavy layering
violation—the grid is purely spatial data that should not know about
neural networks.

**Proposal:** Make `metrics_at` a free function in `qnn_adapter.rs` (or a small
new `metrics.rs`):
```rust
pub fn metrics_at(contents: &Sparse3D<Cell>, structures: &[StructureInfo], location: Vec3) -> Vec<f32>
```
The `MODELS` thread-local moves there too. `WallGrid` itself becomes ML-free.

### 5. `structure.rs` and `mesh_management.rs` are tightly coupled and thin

`load_mesh_handles` takes an `AssetServer` and returns handles keyed by the
filenames that `StructureInfo.main_mesh` / `y_cut_mesh` reference. These two
files exist only to serve each other. Combined they're ~80 lines.

**Proposal:** Merge into `structure.rs`. The combined file would own the full
"what is a structure and how do we load it" story.

### 6. `setup` in `main.rs` does too many things

`setup` currently:
- loads structure infos and mesh handles
- builds `WallGrid`
- spawns camera, light, ground plane
- spawns cursor entities

Each of these has a natural home elsewhere. The cursor spawning belongs in
`input.rs`, the camera spawn in `camera.rs`, the WallGrid bootstrap in
`wall_grid.rs`, etc.—as additional `Startup` systems rather than one monolithic
function.

### 7. The `EguiPrimaryContextPass` scheduling split is easy to miss

`ui_system` must run in `EguiPrimaryContextPass` while everything else runs in
`Update`. This is invisible from the system definitions themselves. It's worth a
comment at the registration site in `main.rs` so the next reader doesn't wonder
why it's different.

---

## Proposed target module layout

```
main.rs              App setup only; registers systems, inserts resources
camera.rs            CameraState, GameCamera, camera_input_system,
                       spawn_camera (Startup system)
input.rs             BuildState, cursor markers, building_input_system,
                       cursor_system, pub(crate) cursor_world_pos
ui.rs                UiState, discover_training_files, ui_system
wall_grid.rs         WallGrid, Cell, compute_visibility, cell_transform,
                       apply_changes, update_visibility_system,
                       spawn_world (ground plane + light Startup system)
structure.rs         StructureInfo, StructureEmbedding, PlacementStyle,
                       Structure, StructureList, load_structure_info,
                       load_mesh_handles, spawn_structures (Startup system)
sparse3d.rs          (unchanged)
qnn_adapter.rs       ModelHolder, metrics_at, MODELS thread-local
qnn.rs               (unchanged)
qnn_translate.rs     (unchanged)
serialization.rs     (unchanged)
build_helpers.rs     (unchanged)
example_structures.rs  (unchanged)
llm_rooms.rs         (unchanged)
```

The dependency graph would then flow cleanly:
- `sparse3d` ← everything
- `structure` ← `wall_grid`, `input`, `ui`
- `qnn_adapter` ← `wall_grid` (only for metrics_at call site)
- `wall_grid` ← `input`, `ui`, `main`
- `camera`, `input`, `ui` ← `main`

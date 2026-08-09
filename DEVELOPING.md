
# Orchitecture

Orchitecture uses the Bevy game engine, and uses Burn to the convolutional neural nets that evaluate strucutres.

## Gameplay

The player starts with three market stalls at an empty crossroads.

The player makes construction plans, then invites farms from the surrounding countryside to participate in the market; the farms trade for resources they want (temporarily increasing their future surplus); the leftovers go to the player, who can use them to construct buildings. Sometimes "traveler"s come, trading more exotic resources, like tools and planks, for market resources. The population of the city starts at 1; each occupant needs food and shelter.

The player builds things that can, for example:
  * store excess resources
  * attract travelers
  * house additional city inhabitants and meet their needs
  * operate as workshops to process resources

Depending on the kind of place, various aspects of the architecture may matter; almost everything works better indoors, some buildings are more inspiring if built from finer materials or with spacious architecture, or (this is where the neural nets come in) with more interesting, coherent architecture. Eventually, the crossroads becomes a proper city, with a library, a city hall, a hospital, and schools.

The player can also influence the surrounding farms by changing their specialties, giving them tools (so they can produce more advanced resources).

# Code map

Game parts:
  * main.rs: initialization
  * simulation.rs: `SimulationPlugin` — the change-detection-gated systems that keep
    derived state (idea progress, places, navigation, assignments, work) in step with
    the city grid. Added by *both* main.rs and the headless harness, so the two can't
    drift; anything added here must stay headless-safe (no rendering, assets, or egui).
  * camera.rs: 3rd-person camera
  * ui.rs: UI
  * idea.rs: `Idea`s are arranged in a DAG and gained piecemeal from books.
  * idea_view.rs / idea_ui.rs: the UI for `Idea`s.
  * scene.rs: ground, roads, and exterior lighting
  * ceiling_lights.rs: Adds lighting inside the city grid (abandoned in favor of global illumination)

City grid and related concepts:
  * city.rs: `ConstructedCity`/`ProposedCity` represent the world as `Sparse3D<Cell>`.
    Core types (`Cell`, `VantageEvaluation`), `apply_changes` (syncs ECS entities to grid state),
    and `cell_transform`.
  * construction.rs: grid-mutation methods on `ProposedCity` — `wall_drag`, `floor_drag`,
    `room_plop`, `drag`, `click`, `undo`, and `load_from_offline`.
  * cutaway.rs: hides parts of the city grid so we can see inside.
  * sparse3d.rs: storage for walls, ceilings and "room objects" on a sparse cubic grid
  * eorf.rs: `Eorf`s (`EorfInfo`/`FurnitureOrElement`) are the walls, doors, desks,
    etc. that occupy `Cell`s — either an `Element` (structural, priced by build
    material) or `Furniture` (fixed cost, no cutaway mesh).
  * place.rs: `Place`s (formerly "stations") are automatically, deterministically
    formed from nearby Furniture/nested-`Place` requirements (see `sync_places`,
    run after every edit) — e.g. a bedroom formed around a pallet. A `Place`'s
    location is its core (first) requirement's location, resolved recursively
    through `Porf` (Place-or-Furniture) requirements.
  * serialization.rs: text format for `Sparse3D<Cell>`
  * pathing.rs: route-finding and connectedness over the city grid, via `bevy_northstar`
  * flood_fill.rs: generic multi-source flood fill over a cubic grid, plus
    `has_sky_above` (shared sky-visibility seed predicate); falloff/transmission
    are supplied by the caller
  * evaluation.rs: `compute_outdoorsness` — flood-fills how "outdoors" each cell
    is, from sky-visible cells (see global_illumination.rs for the analogous
    light computation)

3D autotile system (makes meshes responsive to nearby structures):
  * autotile/parser.rs: Parse the "structures.autotile" file...
  * autotile/compiler.rs: ...turn that representation into oriented match rules...
  * autotile/matcher.rs: ...use that to figure out whether a rule is triggered...
  * autotile/display.rs: ...and show the appropriate meshes

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

`build.rs` generates the meshes used at runtime into `assets/generated/autotile/`:
  * For every mesh spec referenced by a `structures.autotile` rule.
  * For every structure in `elements.ron`/`furniture.ron`: its fallback mesh is
    `assets/generated/autotile/{name}.gltf`, compiled from `buildables/{name}.scad`
    (where `{name}` is the structure name with spaces turned into underscores, so
    `"market stand"` → `market_stand`). Furniture has no cutaway mesh (it vanishes
    when cut away); every other structure also gets a `-cut-y-pos` variant.
    Structures drawn entirely by autotile rules (e.g. roof, column) have no
    `{name}.scad` and need no standalone mesh.

`build.rs` also warns about any `buildables/*.scad` that no autotile rule or
structure references.

## Wings3D-sourced meshes

A mesh can also come from a hand-modeled `buildables/{name}.wings` file instead of
`{name}.scad`. Wings3D has no headless/scriptable export, so `build.rs` can't
compile `.wings` → mesh itself: export a matching `buildables/{name}.glb` by hand
from the Wings3D GUI (File > Export Selected/All > glTF Binary) and commit it
alongside the `.wings` source. `build.rs` fails the build if a `.wings` file has no
matching `.glb`, and warns (without failing) if the `.glb` is older than the
`.wings` file, since that usually means the export is stale.

Once the `.glb` exists, `{name}` can be used as a mesh name in `structures.autotile`,
`elements.ron`, or `furniture.ron` exactly like a `.scad`-backed atom — except
Wings3D-sourced meshes are opaque pre-baked glTF, so they don't support baked
rotation/translation (`:90`, `:+x`) or CSG (`,`/`*`) in `structures.autotile`, and
get no cutaway (`-cut-y-pos`) variant.

The checked-in .gtlf files are programmatically-generated from the OpenSCAD files (eventually, this should be part of the build process). To regenerate them:
```
for f in buildables/*.scad; do dest="$(echo "$f" | sed 's/.scad/.gltf/')"; openscad "$f" -o /tmp/tmp.stl && assimp export /tmp/tmp.stl "$dest"; done;
```

(You need to do `sudo apt install openscad assimp-utils` to get those programs.)


The files used at runtime are located in:
  * `assets/generated/autotile/`: 3D models of structures (generated by `build.rs`)
  * `assets/generated/sprites/`: resource icon PNGs (generated by `build.rs` from `sprites/*.svg`)
  * `assets/static/models/`: saved weights for the QNN
  * `assets/static/training/`: example structures
  * `assets/static/shaders/`: shaders
  * `assets/static/orcs/`: orc character and sprite-sheet assets

# Running tests on headless Linux

`cargo test --lib` requires three system libraries and one SVG converter that are
not installed by default on minimal Ubuntu images.  Install them once, then tests
run normally:

```
sudo apt-get install -y libasound2-dev libudev-dev libwayland-dev librsvg2-bin
```

Without them the build fails deep inside `alsa-sys` with a pkg-config error that
names none of this project, which is a confusing way to lose ten minutes.

Claude Code sessions do this automatically: `.claude/settings.json` registers a
`SessionStart` hook running `.claude/install-build-deps.sh`, which installs the
four packages above (plus `openscad`/`assimp-utils`, best-effort, for mesh
regeneration) the first time a container needs them. It exits immediately when
they're already present, and does nothing at all where there's no `apt-get`
(macOS) or no passwordless sudo — so it's a convenience, not a dependency. The
CI test workflow installs the same four packages.

# Headless testing mode

`cargo run --bin headless [-- --seed <n>]` starts a line-oriented stdin/stdout REPL
over a real Bevy `App` (via `MinimalPlugins` — no window, renderer, or GPU), driving
the game's actual resources (city grid, farms, places, population, clock) and its
real change-detection-gated systems (`rebuild_navigation_grid`, `sync_homes`) — meant
for scripted (e.g. LLM-driven) verification of non-graphical changes, including
change-detection behavior itself. Send `help` as the first command for the full list:
placing/removing structures and boxes, propose-then-construct with sandbox on/off,
undo/redo, advancing time, inviting/configuring farms, querying cells, structures,
places, farms, outdoorsness, inventory, pathfinding, and the raw serialized city
(`dump`).

Mutating commands only mutate resources; they don't advance the schedule. Call
`tick` to run one `Update` pass — that's when `resource_changed`-gated systems
actually react, and `query changed` reports which resources changed as observed by
a persistent system (so it reflects genuine Bevy change tracking, not a same-frame
snapshot). See `src/headless.rs` for the implementation and protocol details.

Headless commands can also be used for testing; see `src/place.rs` for an example.

# Profiling

Two levels, depending on whether you just need a number or need to know which
system is slow:

* **Quick FPS/frame-time/entity-count check**: opt-in via `--perf-log`, since
  the console spam isn't wanted on every run:

  ```
  cargo run --release --bin game -- --perf-log
  ```

  This turns on `FrameTimeDiagnosticsPlugin`, `EntityCountDiagnosticsPlugin`, and
  `LogDiagnosticsPlugin`, which log a line to the console roughly once a second
  with FPS, frame time, and entity count. Run a release build for numbers that
  reflect real performance.

* **Per-system/render-stage breakdown via Tracy**: build with the `trace_tracy`
  feature —

  ```
  cargo run --release --bin game --features trace_tracy
  ```

  This instruments Bevy's schedule (every system, plus render stages) with
  `tracing` spans sent over a local socket. To view them, install the Tracy
  profiler GUI (not packaged for apt/snap — download a release build, or build
  from source, from https://github.com/wolfpld/tracy) and run its
  `tracy-profiler` binary *before or while* the game is running; it connects
  automatically and shows a live, zoomable timeline of frame time broken down by
  system. This is the way to find out which specific system (e.g.
  `update_global_illumination`, `sync_places_system`, `rebuild_navigation_grid`,
  `autotile_update_system`) is responsible for a slow frame in a large scene,
  rather than just knowing that frames are slow.

  `trace_tracy` adds noticeable overhead of its own, so use it to find the
  offending system, then go back to the plain `LogDiagnosticsPlugin` numbers (or
  Tracy's own before/after comparison) to confirm a fix actually helped.

* **Change-frequency tripwire**: always on in debug builds (no flag), unlike
  the above. `change_diagnostics.rs` tracks how often each "big bag of data"
  resource (`ConstructedCity`, `Population`, `IdeaState`, `EorfList`,
  `FarmsResource`) is marked changed, and logs a `warn!` if any of them changes
  on more than 10% of frames — which normally means some system gated on
  `resource_changed` for that resource (a flood fill, nav-grid rebuild, place
  sync, ...) is silently running every frame instead of only on real edits.
  This should not fire in ordinary play; if it does, see `change_guard.rs`
  (`Guarded<T>`, `edit_if_changed`) for the fix pattern — taking `&mut T` out
  of a `ResMut<T>` marks it changed regardless of whether anything was
  actually written, which egui code especially trips over by binding widgets
  straight to resource fields.
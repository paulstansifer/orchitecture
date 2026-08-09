
  * More architectural details?
  * Probably pull some things out of build.rs
  * Finished goods:
    * Baskets, barrels, candles, clothes
  * Maybe refactor material assignment; seems like GI has added a bit of a mess.
  * Don't derive `Clone` or `Copy` on resources; define conserving operations, and a `.clone_for_simulation()` to use when imagining outcomes 
  * Wings3D objects are rotated 180 degrees from how they should be rendered
  * Make it so a hearth doesn't work unless it has a chimney running up to open space.
  * Add "pane", which is brought by travelers, a requirement for windows.
    * Make doors more costly, too, since they'll be the early source of light.
  * Add a notion of "minimum stock" to control workshop rates (and drive auto-invites for farms?). But how does it interact with long, expensive construction projects? -- I guess we only take-from-storage at a certain rate, but it seems pretty opaque.

# Walk-around mode:
  * Make textures and try pixel-art texturing
  * Add wandering workers
  * Try pixel-art heads on 3D bodies

# LLM-suggested cleanups

## Dead / unwired code
  * `src/ceiling_lights.rs` — `update_ceiling_lights` (`#[allow(dead_code)]`, explicitly "kept
    for future use") and `update_window_lights` are both unwired; the whole file's four consts
    only serve this unused path. Ask before deleting: this looks like a deliberately-staged
    feature (interior/window lighting), not dead cruft.
  * `src/bin/sprite_sheet.rs:59` `BULK_WORLD_UNITS = 0.0` permanently disables ~30 lines of
    mesh-inflation code. The doc comment already says "0 = disabled", so this is a documented
    toggle, not a mystery — worth asking whether to rip out the inflation code or keep the knob.

## Complexity / structure worth simplifying
  * `src/cutaway.rs` — `compute_floor_edge` already delegates seeding/BFS to named helpers
    (`descend_to_floor`, `ground_floor_fill`, `find_wall_seeds`, `climb_wall_column`,
    `upper_floor_fill`); only two small post-processing passes remain inline (hidden-floor-above-
    rooms, floor-cut replacement) as low-risk extraction candidates.
  * `src/autotile/parser.rs` — the pattern-row-collection loop in `parse_cases` (now ~741-796)
    mixes header/row/blank-line/pipe handling and could be factored into named helpers.

## Planned structural refactors

Roughly in dependency order; each is independently landable.

  * **Collapse the three parallel capacity families in `storage.rs`.** Bulk
    (`place_capacity_ceiling` / `place_free_capacity_for` / `storage_free_capacity`),
    Rack (`rack_capacity_ceiling` / `rack_free_capacity_for` / `rack_free_capacity`),
    and Book (`book_free_capacity_for` / `book_free_capacity`, no ceiling fn) have
    identical shapes. One set generic over `StorageKind` should replace ~8 functions.
  * **Move `ConstructedCity`'s five per-cube side tables onto `Cell`.**
    `furniture_restrictions`, `bin_resource_restrictions`, `rack_restrictions`,
    `furniture_slots`, and `work_priorities` are all keyed by the cube of a Room-slot
    placement, all must be hand-evicted in both `set_cell` and `take_cell`, and there are
    ~64 references across 7 files. `Cell` already derives `Serialize`/`Deserialize` and
    lives in the `Sparse3D`, so folding them in makes removal automatic, makes the state
    round-trip through save/load for free, and would have made the `load_from_offline`
    staleness bug (since fixed in `replace_contents`) unrepresentable rather than merely
    fixed. A cheaper interim step is a single
    `HashMap<IVec3, PlacementState>`, which keeps the eviction but reduces it to one line.
    Watch out for: `Cell`'s `PartialEq` (used for proposals), how often it's cloned, and
    compatibility with already-saved maps — most cells (walls, floors) carry none of this
    state, so it likely wants to be an `Option<Box<PlacementState>>`.
  * **Let modules own their own wiring.** 53 modules; only four (`GiPlugin`, `ModelPlugin`,
    `DebugVoxelsPlugin`, `GridPreviewPlugin`) register themselves as Bevy `Plugin`s. The
    rest live in main.rs's ~180 lines of `insert_resource`/`add_systems` plus a 60-line
    import list, which means ordering constraints that belong to a module ("spawn_structures
    before spawn_grid before spawn_initial_places") are stated far from the code they
    constrain. The shared `SimulationPlugin` is the first slice of this.
  * **Bundle the simulation arguments.** 14 `#[allow(clippy::too_many_arguments)]` remain.
    `advance_month` takes 10 references and `compute_month_effects` 7 — and `EffectContext`
    already bundles 6 of the same things. One `Sim<'a>` struct collapses all three shapes
    and deletes several of the suppressions.
  * **Give `build_ui.rs` a view module.** The pure-view-model split is applied to three UI
    surfaces (`ui`/`ui_view`, `idea_ui`/`idea_view`, `surroundings::ui`/`ui_view`) and the
    view modules are where the UI tests live. `build_ui.rs` is the largest UI file (1071
    lines) with the most logic — affordability, install menus, requirement bars — and has
    neither a view module nor tests.

## Smaller loose ends
  * `src/ceiling_lights.rs` has zero external references (426 lines, six
    `#[allow(dead_code)]`). See the "Dead / unwired code" note above — still awaiting a
    keep-or-delete call.
  * `buildables/u_cube.scad` and `buildables/u_wall_corner.scad` are unreferenced by
    `structures.autotile`, `elements.ron`, and `furniture.ron`; build.rs already warns about
    both. Either dead assets or a wiring gap.
  * Clippy is advisory in CI (`.github/workflows/test.yml`) rather than a gate, because of
    the ~26 warnings — 20 of them the `too_many_arguments` the argument-bundling item above
    would clear. Flip it to `-D warnings` once that lands.

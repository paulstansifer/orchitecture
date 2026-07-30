
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

## Duplicated logic (candidates for a shared helper)
  * `src/cutaway.rs` — `octant_hidden` vs. the `is_cut_face` closure duplicate the same
    half-space test (not identically — `is_cut_face` adds y/slot filtering); the near-identical
    diff-and-spawn blocks for regular vs. proposed cut entities (~808-837 vs. 839-875) are still
    fully duplicated. (The "three identical render-layer-sync loops" note is stale — one of the
    three now toggles a `CutawayHidden` marker instead, so it's only two.) Touches active
    per-frame ECS logic — ask before refactoring.
  * `src/autotile/matcher.rs` vs `src/autotile/compiler.rs` — both independently encode the
    "cases grouped by `group`, first full match wins" invariant with no type-level guarantee
    they stay in sync; also near-duplicate `rel_slot_to_unoriented`/`slot_to_unoriented`. Core
    autotile dispatch logic — ask before refactoring.
  * `src/construction.rs` — `propose`/`restore_desired` share a "compute cell, write/take,
    record delta" pattern; `room_drag`/`floor_drag` compute identical footprint bounds. Core
    building logic — ask before refactoring.

## Complexity / structure worth simplifying
  * `src/input.rs` — `building_input_system` is now ~254 lines (153-369 is stale) covering many
    unrelated concerns (layers, rotation, cutaway cycling, undo/redo, drag-building, picking).
    Substantial refactor of actively-used input code — ask before splitting up.
  * `src/cutaway.rs` — `compute_floor_edge` already delegates seeding/BFS to named helpers
    (`descend_to_floor`, `ground_floor_fill`, `find_wall_seeds`, `climb_wall_column`,
    `upper_floor_fill`); only two small post-processing passes remain inline (hidden-floor-above-
    rooms, floor-cut replacement) as low-risk extraction candidates.
  * `src/city.rs` — `x_mesh_and_rotations`, `ring_rotation`, `protrude_axis` (now ~782-823) all
    match over `Slot` in parallel, separately-maintained ways — three places to update if `Slot`
    grows a variant. Rendering-only but easy to get subtly wrong — ask before refactoring.
  * `src/autotile/parser.rs` — the pattern-row-collection loop in `parse_cases` (now ~741-796)
    mixes header/row/blank-line/pipe handling and could be factored into named helpers.
  * `src/serialization.rs` — deserialization `panic!`s on malformed room/wall lines (now around
    line 164/167) rather than returning `Result`, inconsistent with the rest of
    `deserialize_sparse3d`. Changing this changes error-propagation behavior for callers — ask
    before changing.
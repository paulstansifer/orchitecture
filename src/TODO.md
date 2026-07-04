  * Make the materials make sense
    * Categorize as for floors, walls, roofs, etc.
    * Sort out resource requirements 
  * More architectural details?
  * Probably pull some things out of build.rs
  * More raw goods:
    * Lime (quarried?) 
  * Finished goods:
    * Baskets, barrels, candles, clothes
  * Split `WallGrid` into multiple things to make change-detection meaningful
  * Shadow-only rendering seems to be path-dependent in some way.
  * Maybe refactor material assignment; seems like GI has added a bit of a mess.
  * Don't derive `Clone` or `Copy` on resources; define conserving operations, and a `.clone_for_simulation()` to use when imagining outcomes 
# Zoomed-out mode:
  * May recruit wayfarers as builders/specialists if a free station is available


# Walk-around mode:
  * Make textures and try pixel-art texturing
  * Add wandering workers
  * Try pixel-art heads on 3D bodies

# LLM-suggested cleanups

## Dead / unwired code
  * `src/ceiling_lights.rs` — `update_ceiling_lights` (`#[allow(dead_code)]`) and
    `update_window_lights` (commented out in `src/main.rs:124`) are both unwired; the whole
    file's four consts only serve this unused path.
  * `src/qnn/translate.rs` — several large commented-out blocks (debug prints, a disabled test
    at 499-516, alternate augmentation code at 143-147).
  * `src/resource.rs:44` `UniformResource::farmable()` and `:183-185` `Inventory::may_add`
    (a `todo!()` stub) — no callers found for either.
  * `src/structure.rs:73` `StructureList::find_by_name` — no callers (everything uses the free
    function or `ConstructedCity::find_structure_by_name` instead).
  * `src/bin/sprite_sheet.rs:59` `BULK_WORLD_UNITS = 0.0` permanently disables ~30 lines of
    mesh-inflation code (508-526, 597-623).
  * `src/scene.rs:9,31-44` — commented-out fill-light constant and spawn loop.
  * `src/build_ui.rs:655-723` — `station_resource_totals`/`construction_cost` look unreferenced
    in this file; worth a crate-wide check.

## Duplicated logic (candidates for a shared helper)
  * `src/qnn/adapter.rs`, `translate.rs:343`, `translate.rs:538-541` — the embedding tuple
    `vec![semb.tall, semb.decorative, semb.passable, semb.striated]` is duplicated 3x.
  * `src/autotile/meshes.rs` — `spawn_autotile_rules` and `load_autotile_handles` both parse
    `structures.autotile` independently instead of sharing one parsed resource.
  * `src/cutaway.rs` — several internal duplications: `octant_hidden` vs. the `is_cut_face`
    closure (515-557); three identical render-layer-sync loops (749-780); near-identical
    diff-and-spawn blocks for regular vs. proposed cut entities (793-860).
  * `src/autotile/matcher.rs` vs `src/autotile/compiler.rs` — both independently encode the
    "cases grouped by `group`, first full match wins" invariant with no type-level guarantee
    they stay in sync; also near-duplicate `rel_slot_to_unoriented`/`slot_to_unoriented`.
  * `src/sparse3d.rs` — `Chunk::iter`/`iter_mut` and `Index`/`IndexMut` all re-derive the same
    flat-index formula (4 copies); `collect_at_point` repeats the same lookup-and-push pattern
    4x for Room/XWall/Floor/ZWall.
  * `src/construction.rs` — `propose`/`restore_desired` share a "compute cell, write/take,
    record delta" pattern; `room_drag`/`floor_drag` compute identical footprint bounds.
  * `src/build_ui.rs` — the "load map contents" sequence (`clear_proposal_entities` →
    `clear_proposed_cut_entities` → `load_from_offline` → `apply_changes`) is repeated 3x.
  * `src/input.rs:232-238` vs `:281-337` — the `if sandbox.enabled { construct+apply_changes }
    else { apply_proposal_changes }` dispatch is repeated verbatim.
  * `src/ui.rs` — the "find the storage/market-stand station by name" lookup is duplicated 3x
    inside `shared_ui_system`.
  * `src/camera.rs` / `src/walk_input.rs` / `src/ortho_camera.rs` — cursor-delta tracking
    boilerplate duplicated with inconsistent `windows.single()` error handling.
  * `src/build_helpers.rs` — `Cell { .. }` literal boilerplate repeated across `wall()`,
    `wall_off_drops`, `set_vantage`.

## Complexity / structure worth simplifying
  * `src/input.rs:153-369` — `building_input_system` is ~215 lines covering many unrelated
    concerns (layers, rotation, cutaway cycling, undo/redo, drag-building, picking).
  * `src/ui.rs:21-380` — `shared_ui_system` is ~360 lines mixing UI rendering with end-of-month
    simulation logic; the "advance month" block (300-370) is a good extraction candidate.
  * `src/cutaway.rs:388-512` — `compute_floor_edge` (~125 lines) mixes seeding, BFS, and two
    post-processing passes that could be named helpers.
  * `src/city.rs:562-603` — `x_mesh_and_rotations`, `ring_rotation`, `protrude_axis` all match
    over `Slot` in parallel, separately-maintained ways — three places to update if `Slot`
    grows a variant.
  * `src/autotile/parser.rs:395-443` — the pattern-row-collection loop in `parse_cases` mixes
    several concerns and could be factored out.
  * `src/serialization.rs:149-154` — deserialization `panic!`s on malformed input rather than
    returning `Result`, inconsistent with the rest of the function's error handling.

## Stale / misleading comments worth a pass
  * `src/autotile/parser.rs:692` — a self-doubt TODO questioning consistency between `offset`
    and `is_dead_slot` that's likely resolved (both have test coverage) and could be deleted
    or turned into a real check.
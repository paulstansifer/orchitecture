
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
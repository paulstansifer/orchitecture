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
  * Icons seems to not be antialiased?
  * Don't derive `Clone` or `Copy` on resources; define conserving operations, and a `.clone_for_simulation()` to use when imagining outcomes 
# Zoomed-out mode:
  * May recruit wayfarers as builders/specialists if a free station is available


# Walk-around mode:
  * Make textures and try pixel-art texturing
  * Add wandering workers
  * Try pixel-art heads on 3D bodies
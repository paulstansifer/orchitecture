  * More architectural details?
  * Shadows are currently pretty rough; tweak them?
  * Materials + textures (timber, fieldstone, crafted wood, quarried stone, brick)
  * Split autotile.rs into three concerns (parsing, compiling, and mesh selection). Probably pull some things out of build.rs
  * Finish switching to absolute slot coordinates in most places



# Roofs

"Roof" will be a RoomPlop structure. (Hmm, I guess a RoomDrag structure) If it has a roof above it, it is a solid cube. Otherwise, it autotiles to slant upwards to adjacent roofs. I think we can get good shapes with this, but the 3D aspect is hard to visualize.

  /R\   /R
 /RRR\ /RR
RRRRRRvRRR
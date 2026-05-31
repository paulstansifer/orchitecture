  * More architectural details?
  * Shadows are currently pretty rough; tweak them?
  * Materials + textures (timber, fieldstone, crafted wood, quarried stone, brick)


# Autotile

The display of each tile depends on the tiles around it. So, consecutive windows should merge,
railings should follow staircases up, columns should have tops and bottoms.

Sometimes, the components should be extractible 9-tile-style, from a single model.


# Roofs

"Roof" will be a RoomPlop structure. (Hmm, I guess a RoomDrag structure) If it has a roof above it, it is a solid cube. Otherwise, it autotiles to slant upwards to adjacent roofs. I think we can get good shapes with this, but the 3D aspect is hard to visualize.

  /R\   /R
 /RRR\ /RR
RRRRRRvRRR
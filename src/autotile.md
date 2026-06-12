# 3D Autotile
Autotile rules files need to be preprocessed at build time (by "build.rs") to generate all the needed model files, and used at runtime to place them in the appropriate locations.

An autotile pattern file consists of rules. There should be at least one rule per structure. Let's use EBNF here for the overall structure.

rule           = "==" structure name, ":", slot, "==\n", { pattern }
structure name = identifier
slot           = "wall" | "room" | "floor"
pattern        = [ pattern type, { pattern line } ], "-->" result

(Any line may end with spaces and/or a Bash-style comment.)


## Autotile patterns

pattern type   = single type | "|", single type, { "|", single type }, "|"
single type    = "H" | "H narrow" | "V" | "V narrow"
pattern line   = " ", { "%" | " " | "." | "=" | letter }, "\n"
               | [ " " ], "|", { layer segment, "|" }       (* multi-layer *)

Pattern lines are interpreted together as a 2D grid, in the XZ plane (unoriented) for "H" pattenrs or the XY or ZY planes (with up at the top, but otherwise unoriented) for "V" patterns. The 2D pattern must have exactly one "@", indicating where the structure in question is. More details later.

Horizontal patterns (`H:`) contain walls, rooms, and dead slots as in the following repeating pattern:
W.W.
RWRW
W.W.
If the @ is a wall slot, the pattern needs disambiguation: the slots on the same line are treated as room slots, and the ones on adjacent lines are dead slots.

Vertical patterns (`V:`) contain floor, wall, and room slots:
F.F.
RWRW
F.F.
RWRW

The two "narrow" pattern types are single-slot companions to the full patterns;
their one live slot sits directly "behind" the `R` (room) position of the
corresponding full pattern. They mirror each other: walls are the surface that
tiles in the vertical plane, floors the surface that tiles in the horizontal
plane.

Narrow vertical patterns (`V narrow:`) are wall-anchored and contain only the
depth-facing wall slot (the wall "behind" the room, perpendicular to the V
plane), for vertical wall stacking:
....
W.W.
....
W.W.

Narrow horizontal patterns (`H narrow:`) are floor-anchored and contain only floor slots
(the floor of the room's own cube), for horizontal floor tiling in the XZ plane:
....
F.F.
....
F.F.

All patterns are 4-way symmetric in the Y axis. But orientation is respected; all models will be rotated to match the orientation of the @ structure.

### Multi-layer patterns

A rule may stack several same-shaped grids along the plane's third axis (Y for
the H family, Z for the V family) by writing the pattern types as a
pipe-delimited list, e.g. `|H|H narrow|H|:` or `|H|H|H narrow|H|:`. Each pattern
line then carries one `|`-delimited segment per layer; segment `k` is layer
`k`'s row. For example, one line of a trivial 3-layer rule is `|   | @ |   |`
(an optional leading space before the first `|` is allowed for alignment).

Rules:
  * All layers must be the same family (all `H`/`H narrow`, or all `V`/`V narrow`).
  * The **leftmost** layer is the highest world coordinate; each regular layer is
    one cube lower than the previous.
  * `narrow` layers are the **boundary** between regular layers — a floor for the
    H family, the depth-facing wall for the V family — and are **optional**: the
    regular layers stay one cube apart whether or not a narrow sits between them.
    A rule may start with a narrow layer, but two narrow layers may not be
    adjacent, and a multi-layer rule must contain at least one regular layer.
  * Exactly one `@` total, in any layer.

Single-layer rules keep the original bare syntax (no pipes; pattern lines start
with a space).

A particular position can only match (as the `@`) once per rule (the highest match takes priority). If there are multiple rules for one structure, they can all match; and their patterns will be overlaid.

The last pattern in a rule may be omitted; it will be an `else` pattern that always matches (equivalent to just an `@`, in any orientation).

Placing anything other than a space in a dead slot is an error.

Characters match as follows:
  * `@`: the structure in question
  * `=`: another copy of the same structure
  * `.`: empty space
  * ` `: anything, empty or not.
  * `F`: floor
  * `W`: wall (the structure called "wall": not windows or doors, etc.)
  * `S`: stairs
  * `R`: railing

## Results

result       = [ "(multi)" ], mesh spec | "none"

"(multi)" before a mesh spec indicates that a pattern may match repeatedly at the same spot in different orientations, with the resulting meshes overlayed. Otherwise, matching in multiple orienations is an error (unless, of course, the pattern is symmetric in those orientations)!

mesh spec    =   mesh spec "+" mesh spec | mesh spec "*" mesh spec 
               | "(" mesh spec ")" | identifer [ ":" number ]

mesh specs respect operator precenence in the way you expect; I'm not gonna write that all out, though. Identifers name an actual mesh, the colon indicates a rotation (around the y axis), "+" is union, and "*" is intersection.

The mesh operations need to be evaluated by generating an OpenSCAD file like: 

```
intersection {
    include <mesh_file_a>
    include <mesh_file_b>
}
```

Then `openscad <filename> -o /tmp/tmp.stl && assimp export /tmp/tmp.stl /tmp/tmp.gltf` generates a loadable model file.

Then, for each mesh, we also need to generate a "cut" version for the cutaway view:

```
intersection() {
    include <mesh_atom>
    union() {
        surface("jagged.dat");
        translate([0,0,-13])
        cube([13,13,13]);
    }
}
```

(and likewise make it loadable)

## Implementation

The text format is parsed into an `AutotilePattern`, and then compiled into an `AutotileOriented`, which has all relevant orientations (some patterns are symmetric, so there may be 1, 2, or 4 sub-patterns). This improves performance and also prevents symmetric patterns from issuing an error when they inevitably match multiple times.


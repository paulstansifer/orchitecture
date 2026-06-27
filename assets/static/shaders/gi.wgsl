// Adjacency-aware trilinear global illumination, added on top of standard PBR.
//
// See src/gi_material.rs for the data layout. The GI texture is one voxel per grid
// cube: R = sky illuminance, G/B/A = boundary transmission on the cube's -X/-Y/-Z
// faces (0 = wall, 0.5 = window/doorway, 1 = open).
//
// We fetch the 2x2x2 voxel neighborhood and weight each of the 8 cells by its
// trilinear weight times its *reachability* from the cell the fragment sits in. A
// neighbor's reachability is the transmission along the most-open monotone path to
// it (max over path orderings; product of the crossed boundaries). This stops light
// from leaking across walls even diagonally at corners: a cell walled off on every
// path gets weight 0. Open interiors (all transmissions 1) reduce to plain trilinear
// (smooth, no visible grid); a wall (0) collapses to nearest-neighbor (no bleed).

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> gi_min_cube: vec4<f32>; // xyz = world cube of voxel (0,0,0); w = intensity
@group(#{MATERIAL_BIND_GROUP}) @binding(101)
var<uniform> gi_resolution: vec4<f32>; // xyz = (rx, ry, rz)
@group(#{MATERIAL_BIND_GROUP}) @binding(102)
var gi_tex: texture_3d<f32>;

fn load_voxel(idx: vec3<i32>) -> vec4<f32> {
    let hi = vec3<i32>(gi_resolution.xyz) - vec3<i32>(1);
    let c = clamp(idx, vec3<i32>(0), hi);
    return textureLoad(gi_tex, c, 0);
}

// Reachability of cell `c` from the home cell `h` (both corners of the 2x2x2 cube,
// components in {0,1}). The boundary arrays hold the transmission for crossing the
// single 0<->1 step on each axis: xb[j+2k] = -X face of the i=1 cell, yb[i+2k] =
// -Y face of the j=1 cell, zb[i+2j] = -Z face of the k=1 cell. For multi-axis
// (diagonal) cells we take the most-open path (max over orderings, product of steps).
fn reach(
    c: vec3<i32>,
    h: vec3<i32>,
    xb: array<f32, 4>,
    yb: array<f32, 4>,
    zb: array<f32, 4>,
) -> f32 {
    let dx = c.x != h.x;
    let dy = c.y != h.y;
    let dz = c.z != h.z;
    let n = i32(dx) + i32(dy) + i32(dz);

    if n == 0 {
        return 1.0;
    }
    if n == 1 {
        if dx { return xb[c.y + 2 * c.z]; }
        if dy { return yb[c.x + 2 * c.z]; }
        return zb[c.x + 2 * c.y];
    }
    if n == 2 {
        if !dz { // X and Y differ (k fixed)
            let p1 = xb[h.y + 2 * h.z] * yb[c.x + 2 * h.z];
            let p2 = yb[h.x + 2 * h.z] * xb[c.y + 2 * h.z];
            return max(p1, p2);
        }
        if !dy { // X and Z differ (j fixed)
            let p1 = xb[h.y + 2 * h.z] * zb[c.x + 2 * h.y];
            let p2 = zb[h.x + 2 * h.y] * xb[h.y + 2 * c.z];
            return max(p1, p2);
        }
        // Y and Z differ (i fixed)
        let p1 = yb[h.x + 2 * h.z] * zb[h.x + 2 * c.y];
        let p2 = zb[h.x + 2 * h.y] * yb[h.x + 2 * c.z];
        return max(p1, p2);
    }
    // n == 3: all six axis orderings.
    let xyz = xb[h.y + 2 * h.z] * yb[c.x + 2 * h.z] * zb[c.x + 2 * c.y];
    let xzy = xb[h.y + 2 * h.z] * zb[c.x + 2 * h.y] * yb[c.x + 2 * c.z];
    let yxz = yb[h.x + 2 * h.z] * xb[c.y + 2 * h.z] * zb[c.x + 2 * c.y];
    let yzx = yb[h.x + 2 * h.z] * zb[h.x + 2 * c.y] * xb[c.y + 2 * c.z];
    let zxy = zb[h.x + 2 * h.y] * xb[h.y + 2 * c.z] * yb[c.x + 2 * c.z];
    let zyx = zb[h.x + 2 * h.y] * yb[h.x + 2 * c.z] * xb[c.y + 2 * c.z];
    return max(max(max(xyz, xzy), max(yxz, yzx)), max(zxy, zyx));
}

// Adjacency-aware sample of the illuminance field at world position `p`.
fn sample_gi(p: vec3<f32>) -> f32 {
    // Cube centers sit at integers in this space.
    let q = p - gi_min_cube.xyz - vec3<f32>(0.5);
    let base = floor(q);
    let f = q - base;
    let bi = vec3<i32>(base);

    // Fetch the 2x2x2 neighborhood: v[i + 2j + 4k], each component in {0,1}.
    var v: array<vec4<f32>, 8>;
    for (var k = 0; k < 2; k = k + 1) {
        for (var j = 0; j < 2; j = j + 1) {
            for (var i = 0; i < 2; i = i + 1) {
                v[i + 2 * j + 4 * k] = load_voxel(bi + vec3<i32>(i, j, k));
            }
        }
    }

    // Boundary transmissions for the single 0<->1 step on each axis.
    var xb: array<f32, 4>;
    var yb: array<f32, 4>;
    var zb: array<f32, 4>;
    for (var k = 0; k < 2; k = k + 1) {
        for (var j = 0; j < 2; j = j + 1) {
            xb[j + 2 * k] = v[1 + 2 * j + 4 * k].g;
        }
    }
    for (var k = 0; k < 2; k = k + 1) {
        for (var i = 0; i < 2; i = i + 1) {
            yb[i + 2 * k] = v[i + 2 + 4 * k].b;
        }
    }
    for (var j = 0; j < 2; j = j + 1) {
        for (var i = 0; i < 2; i = i + 1) {
            zb[i + 2 * j] = v[i + 2 * j + 4].a;
        }
    }

    // The cell the fragment is in (nearest center on each axis).
    let h = vec3<i32>(
        select(0, 1, f.x >= 0.5),
        select(0, 1, f.y >= 0.5),
        select(0, 1, f.z >= 0.5),
    );

    var num = 0.0;
    var den = 0.0;
    for (var k = 0; k < 2; k = k + 1) {
        for (var j = 0; j < 2; j = j + 1) {
            for (var i = 0; i < 2; i = i + 1) {
                let c = vec3<i32>(i, j, k);
                let tw = select(1.0 - f.x, f.x, i == 1)
                    * select(1.0 - f.y, f.y, j == 1)
                    * select(1.0 - f.z, f.z, k == 1);
                let w = tw * reach(c, h, xb, yb, zb);
                num = num + w * v[i + 2 * j + 4 * k].r;
                den = den + w;
            }
        }
    }
    return num / max(den, 1e-5);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);

    // Bias the sample point along the surface normal so each thin-wall face samples
    // the cube it faces into (bright-room side stays bright, dark side stays dark).
    let sample_pos = in.world_position.xyz + pbr_input.world_normal * 0.25;
    let gi = sample_gi(sample_pos);
    let albedo = pbr_input.material.base_color.rgb;
    out.color = vec4<f32>(out.color.rgb + albedo * gi * gi_min_cube.w, out.color.a);

    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}

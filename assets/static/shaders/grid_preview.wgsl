#import bevy_pbr::forward_io::VertexOutput

struct GridPreviewMaterial {
    cursor_xz: vec4<f32>, // xy = cursor world x,z
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> material: GridPreviewMaterial;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let wx = mesh.world_position.x;
    let wz = mesh.world_position.z;
    let cx = material.cursor_xz.x;
    let cz = material.cursor_xz.y;

    let dist = distance(vec2<f32>(wx, wz), vec2<f32>(cx, cz));
    let fade = 1.0 - smoothstep(5.0, 7.0, dist);
    if fade <= 0.001 {
        discard;
    }

    // Anti-aliased lines at integer world coordinates.
    let line_dist_x = min(fract(wx), 1.0 - fract(wx));
    let line_dist_z = min(fract(wz), 1.0 - fract(wz));
    let fw_x = fwidth(wx);
    let fw_z = fwidth(wz);
    let line_x = 1.0 - smoothstep(fw_x * 0.25, fw_x * 0.75, line_dist_x);
    let line_z = 1.0 - smoothstep(fw_z * 0.25, fw_z * 0.75, line_dist_z);
    let on_line = max(line_x, line_z);

    if on_line < 0.01 {
        discard;
    }

    return vec4<f32>(0.0, 0.0, 0.0, on_line * fade);
}

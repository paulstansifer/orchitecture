#import bevy_pbr::forward_io::VertexOutput

struct RingMaterial {
    params: vec4<f32>, // x = elapsed time, y = striped flag
    color: vec4<f32>,  // solid ring color, used when params.y == 0
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> material: RingMaterial;

const PI: f32 = 3.14159265;
const STRIPE_COUNT: f32 = 10.0;
const ROTATION_SPEED: f32 = 0.4; // radians/sec

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    // The mesh's UVs are centered at (0.5, 0.5) with the circle's edge at
    // radius 0.5, so this is stable regardless of world-space scale/stretch.
    let centered = mesh.uv - vec2<f32>(0.5, 0.5);
    let radius = length(centered) * 2.0;

    // A ring band, anti-aliased against the derivative of `radius`.
    let fw = fwidth(radius);
    let outer = 1.0 - smoothstep(0.9 - fw, 0.9 + fw, radius);
    let inner = smoothstep(0.6 - fw, 0.6 + fw, radius);
    let ring = outer * inner;
    if ring <= 0.001 {
        discard;
    }

    if material.params.y == 0.0 {
        return vec4<f32>(material.color.rgb, ring * material.color.a);
    }

    let angle = atan2(centered.y, centered.x) + material.params.x * ROTATION_SPEED;
    let stripe = fract(angle / (2.0 * PI) * STRIPE_COUNT);
    let blue = vec3<f32>(0.15, 0.45, 1.0);
    let white = vec3<f32>(1.0, 1.0, 1.0);
    let color = select(blue, white, stripe < 0.5);

    return vec4<f32>(color, ring * 0.85);
}

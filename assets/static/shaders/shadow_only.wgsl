// Shadow-caster-only material: discards in the main (color) pass so the mesh is
// invisible to the camera, while still writing depth in the light's shadow/prepass
// (a separate depth-only pipeline that never runs this fragment) so it keeps
// casting a shadow.
//
// This is how cutaway-hidden geometry casts shadows: render layers can't do it,
// because `queue_shadows` filters casters by the *camera's* layers, so a caster
// must share a layer with the camera to cast into its view — which also makes it
// visible. Discarding in the color pass decouples the two.

#import bevy_pbr::forward_io::VertexOutput

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    discard;
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}

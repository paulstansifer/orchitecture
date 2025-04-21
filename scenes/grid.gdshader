shader_type spatial;

// Based on https://godotforums.org/d/36314-draw-grid-on-floor/10

render_mode blend_mix, unshaded;

uniform float gridlineOnEvery = 1.0;
uniform float lineThickness = .01;
uniform vec2 originOffset = vec2(0.0, 0.0);
uniform vec4 color: source_color = vec4(0.0, 1.0, 1.0, .5);
uniform float fade_inner_radius = 4.0;
uniform float fade_outer_radius = 6.0;

varying vec2 pointWorld;

void vertex(){
	pointWorld = (MODEL_MATRIX * vec4(VERTEX, 1.0) ).xz - originOffset;
}

void fragment() {
	float distance = length(pointWorld);
	float fade = smoothstep(fade_inner_radius, fade_outer_radius, distance);

	float thick = lineThickness / gridlineOnEvery;
	vec2 df = fract(pointWorld / gridlineOnEvery + thick / 2.0);
	float grid = step(thick, df.x) * step(thick, df.y);
	ALBEDO = color.rgb;
	ALPHA = (1.0 - grid) * color.a * (1.0 - fade);
}

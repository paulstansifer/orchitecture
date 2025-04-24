shader_type spatial;

// Loosely based on https://godotforums.org/d/36314-draw-grid-on-floor/10

render_mode blend_mix, unshaded;

uniform float gridlineOnEvery = 1.0;
uniform float lineThickness = .05;
uniform vec4 color: source_color = vec4(0.0, 0.7, 1.0, 1.0);
uniform float fade_inner_radius = 4.0;
uniform float fade_outer_radius = 6.0;
uniform float uv_scale = 1.0; // Add UV scale

void vertex() {
	// No transformation needed here
}

void fragment() {
	vec2 pointWorld = (UV - vec2(0.5, 0.5)) * 16.0; // Use scaled UV coordinates

	float distance = length(pointWorld);
	float fade = smoothstep(fade_inner_radius, fade_outer_radius, distance);

	float thick = lineThickness / gridlineOnEvery;
	vec2 df = fract(pointWorld / gridlineOnEvery + thick / 2.0);
	float grid = step(thick, df.x) * step(thick, df.y);
	ALBEDO = color.rgb;
	ALPHA = (1.0 - grid) * color.a * (1.0 - fade);
}

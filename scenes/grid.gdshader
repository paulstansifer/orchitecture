shader_type spatial;

// Loosely based on https://godotforums.org/d/36314-draw-grid-on-floor/10

render_mode blend_mix, unshaded;

uniform float fade_inner_radius = 2.0;
uniform float fade_outer_radius = 3.5;

void fragment() {
	vec2 pointWorld = (UV - vec2(0.5, 0.5)) * 16.0; // Use scaled UV coordinates
	
	float xpos = max(mod(pointWorld.x, 1.0), 1.0-mod(pointWorld.x, 1.0));
	float ypos = max(mod(pointWorld.y, 1.0), 1.0-mod(pointWorld.y, 1.0));
	
	float dist = length(pointWorld);
	float fade = 1.0-smoothstep(fade_inner_radius, fade_outer_radius, dist);
	
	ALBEDO = vec3(0.7, 0.7, 0.0);
	ALPHA = smoothstep(0.97, 0.99, max(xpos, ypos)) * fade * 0.5;
}

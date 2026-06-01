// Railing

module railing(length, height, post_radius, rail_radius) {

    // Posts
    for (i = [0:0.2:length]) {
        translate([i, 0, 0]) {
            cylinder(h = height, r = post_radius, $fn = 20);
        }
    }

    translate([-.1, -.1, height])
    cube([0.8, .2, .1]);
    
    translate([0.7, 0, height])
    cylinder(h=0.1, r=0.1, $fn=20);
}

color([0.3, 0.3, 0.5])
railing(length = 0.6, height = .4, post_radius = 0.05, rail_radius = 0.03);

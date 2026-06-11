// Railing


union() {
    length = 0.6;
    height=0.4;
    post_radius=0.05;
    rail_radius=0.03;

    // Posts
    for (i = [0.1:0.2:length+0.1]) {
        translate([i, 0, 0]) {
            cylinder(h = height, r = post_radius, $fn = 20);
        }
    }

    translate([0, -.1, height])
    cube([0.7, .2, .1]);

    translate([0.7, 0, height])
    cylinder(h=0.1, r=0.1, $fn=20);
}
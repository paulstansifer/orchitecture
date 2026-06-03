length=1;
height=.4;
post_radius=0.05;
rail_radius=0.03;

// Posts
for (i = [0:0.2:1]) {
    translate([i * length, 0, 0]) {
        cylinder(h = height, r = post_radius, $fn = 20);
    }
}

translate([-.1, -.1, height])
cube([1.2, .2, .1]);

translate([0,-.1, 0])
cube([1,.1,.1]);

rotate([0,90,0])
linear_extrude(1, false, convexity=1, twist=0, slices=1) {
    polygon([[0, -.2], [-.1, -.1], [0, 0]]);
}

translate([1,0,0])
scale([-1,1,1])
rotate([0,0,0])
linear_extrude(1, false, convexity=1, twist=0, slices=1) {
    polygon([[0, 0], [-.1, -.1], [0, -.1]]);
}
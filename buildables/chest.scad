
translate([0.2, 0.1, 0.1])
rotate([90,0,90])
        linear_extrude(.6, false, convexity=1, twist=0, slices=1) {
polygon([[0, 0.0], [0.3, 0],
[0.3, 0.16], [0.26, 0.2], [0.04, 0.2], [0, 0.16]]);
        }
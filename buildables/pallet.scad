
translate([0.2, 0.1, 0.1])
rotate([90,0,90])
        linear_extrude(.6, false, convexity=1, twist=0, slices=1) {
polygon([[0, 0.02], [0.02, 0], [0.28, 0], [0.3, 0.02],
[0.3, 0.08], [0.28, 0.1], [0.02, 0.1], [0, 0.08]]);
        }
color([0.3, 0.3, 0.5])
translate([0.5, 0, 0.0]) {
    translate([0,0,0.2])
    cylinder(h = 0.6, r = 0.07, $fn = 20);
    translate([0,0,0.1])
    cylinder(h = 0.1, r1 = 0.0, r2 = 0.1, $fn = 20);
    translate([0,0,0.8])
    cylinder(h = 0.1, r1 = 0.1, r2 = 0.0, $fn = 20);
}

color([0.3, 0.3, 0.5])
translate([0.5, 0, 0.0]) {
    cylinder(h = 1.0, r = 0.07, $fn = 20);
    cylinder(h = 0.2, r = 0.1, $fn = 20);
    translate([0,0,0.8])
    cylinder(h = 0.2, r = 0.1, $fn = 20);
}

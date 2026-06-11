color([0.3, 0.3, 0.5])
translate([0.5, 0, 0.1]) {
    cylinder(h = .8, r = 0.07, $fn = 20);
    cylinder(h = 0.1, r = 0.1, $fn = 20);
    translate([0,0,0.7])
    cylinder(h = 0.1, r = 0.1, $fn = 20);
}

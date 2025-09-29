
intersection() {
    color([0.3, 0.3, 0.5])
    difference() {
        translate([-0.1,-0.1,-0.1])
        cube([1.2, .2, 1.2], false);

        rotate([90, 0, 0])
        {
            translate([.5, .5, 0])
            cube([.9, .9, .3], center = true);
        }

    }
    color([0.5, 0.5, 0.5])
    translate([-.15,-.15,.25])
    scale([1/10,1/10, 1/13])
    union() {
        surface("jagged.dat");
        translate([0,0,-13])
        cube([13,13,13]);
    }
}
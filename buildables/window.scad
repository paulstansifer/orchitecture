color([0.3, 0.3, 0.5])
difference() {
    translate([0.1,-0.1,0.1])
    cube([.8, .2, .8], false);

    rotate([90, 0, 0])
    {
        translate([.5, .5, 0])
        cube([.7, .7, .3], center = true);
    }

}

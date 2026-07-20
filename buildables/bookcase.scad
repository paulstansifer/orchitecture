color([0.6, 0.4, 0.2])
translate([0.15, 0.15, 0.1]) {
    // Shelf 
    for(z=[0.2 : .15 : .75]) {
        translate([0,0,z])
        cube([.75, 0.3, 0.02]);
    }
    translate([0.0, 0, 0.0])
    cube([.02, 0.3, .65]);

    translate([0.73, 0, 0.0])
    cube([.02, 0.3, .65]);
}
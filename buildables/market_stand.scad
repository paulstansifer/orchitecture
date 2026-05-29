color([0.6, 0.4, 0.2])
translate([0.15, 0.4, 0.1]) {
    // Desk 
    translate([0,0,.25])
    cube([.7, 0.3, 0.05]);
    
    // Drape over the front
    translate([0,.3, .05])
    cube([.7, 0.01, .3-.05]);

    // Legs
    for(x=[0.025, 0.675]) {
        translate([x, 0.025, 0])
        cylinder(0.25, r=0.025, $fn=10);

        // Front legs go up to support the awning
        translate([x, 0.25, 0])
        cylinder(0.70, r=0.025, $fn=10);
    }
    
    
    for(x=[0.025, 0.675]) {       
        translate([x-0.025,0,0])
        rotate([90,0,90])
        linear_extrude(.05, false, convexity=1, twist=0, slices=5) {
            // I just eyeballed this. Sorry.
            polygon([[0.13, .70], [0.25, .77], [0.37, .70]]);
        }
    }

    // Awning 
    translate([0,.25,.75+.02])
    {
        rotate([-30,0,0])
        translate([0,0,-0.005])
        cube([.7, 0.3, 0.01]);

        rotate([180+30,0,0])
        translate([0,0,-0.005])
        cube([.7, 0.3, 0.01]);
    }

}
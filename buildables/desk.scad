color([0.6, 0.4, 0.2])
translate([0.25, 0.4, 0.1]) {
    // Desk 
    translate([0,0,.25])
    cube([.5, 0.3, 0.05]);

    // Legs
    for(x=[0.025, 0.475])
    for(y=[0.025, 0.275])
    translate([x, y, 0])
    cylinder(0.25, r=0.025, $fn=10);


    // Chair
    translate([0.125, -0.25, 0.15]) {
        // Seat
        cube([0.25, 0.25, .05]);

        // Backrest
        cube([0.25, 0.05, 0.25]);

        // Legs
        for(x=[0.025, 0.225])
        for(y=[0.045, 0.225])
        translate([x, y, -0.15])
        cylinder(0.15, r=0.025, $fn=10);
    }
}
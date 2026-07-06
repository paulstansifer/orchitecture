translate([0.4, -.1, 0.25]) {
 
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


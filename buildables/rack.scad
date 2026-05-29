color([0.6, 0.4, 0.2])
translate([0.15, 0.15, 0.1]) {
    // Shelf 
    for(z=[0.2 : .15 : .75]) {
        translate([0,0,z])
        cube([.75, 0.3, 0.02]);
    }
    // Legs
    for(x=[0.025, 0.725])
    for(y=[0.025, 0.275])
    translate([x, y, 0])
    cylinder(0.65, r=0.025, $fn=10);
}
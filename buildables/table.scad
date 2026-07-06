translate([.25, .25, 0]) {
    
    translate([0,0,.25])
    cube([.5, 0.5, 0.05]);
        // Legs
    for(x=[0.025, 0.475])
    for(y=[0.025, 0.475])
    translate([x, y, 0])
    cylinder(0.25, r=0.025, $fn=10);

}
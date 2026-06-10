color([0.3, 0.3, 0.5])
difference() {
    translate([0.1,-0.1,0.5])
    cube([0.8, .2, 0.4], false);

    
    rotate([90, 0, 0])
    {
        translate([.5, .4, 0]) 
        cylinder(.3, .4, .4, center = true,
                 $fn=30);
        translate([.5, 0, 0])
        cube([.8, .8, .3], center = true);
        
    }

}

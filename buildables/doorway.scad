color([0.3, 0.3, 0.5])
difference() {
    translate([-0.1,-0.1,-0.1])
    cube([1.2, .2, 1.2], false);

    
    rotate([90, 0, 0])
    {
        translate([.5, .4, 0]) 
        cylinder(.3, .4, .4, center = true,
                 $fn=30);
        translate([.5, 0, 0])
        cube([.8, .8, .3], center = true);
        
    }

}

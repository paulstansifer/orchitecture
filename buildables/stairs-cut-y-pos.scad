intersection() {
    color([0.3, 0.3, 0.5])
    for (i = [0.1:0.1:1]) {
        translate([0,i-.2,0])
        cube([1,.1, i]);
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

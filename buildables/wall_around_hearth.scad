color([0.3, 0.3, 0.5])
translate([0,0.1,0])
union() {
    rotate([90, 0, 0])
    linear_extrude(0.2) {
        polygon([[0,1], [0.333,1], [0,0.666]]);
    }
    
    rotate([90, 0, 0])
    linear_extrude(0.2) {
        polygon([[1,1], [0.666,1], [1,0.666]]);
    }
}

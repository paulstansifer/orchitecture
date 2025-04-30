color([0.3, 0.3, 0.5])
for (i = [0.1:0.1:1]) {
    translate([0,i-.2,0])
    cube([1,.1, i]);
}
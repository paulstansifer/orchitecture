
// Listen, this is just trial and error here.
intersection() {
    translate([0,1,0])
    rotate([90,0,0])
    linear_extrude(1.5) {
        polygon([[.8,0], [1, 0], [1.2,-.2], [.8, -.2], ]);
    };

    translate([1.5,1,0])
    rotate([90,0,-90])
    linear_extrude(1.5) {
        polygon([[.8,0], [1, 0], [1.2,-.2], [.8, -.2], ]);
    };
}
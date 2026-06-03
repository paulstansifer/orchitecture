
length=1;
height=.4;
post_radius=0.05;
rail_radius=0.03;
skew = [ [ 1  , 0  , 0  , 0   ],
         [ 0  , 1  , 0  , 0   ],
         [ 1  , 0  , 1  , 0   ],
         [ 0  , 0  , 0  , 1   ] ] ;


// Posts
for (i = [0:0.2:1]) {
    translate([i * length, 0, i * length]) {
        cylinder(h = height, r = post_radius, $fn = 20);
    }
}

translate([-.1, -.1, height-.15])
multmatrix(skew)
cube([1.2, .2, .1]);

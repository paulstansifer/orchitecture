
For development, you need to (in Ubuntu) do 
```
sudo apt install assimp-utils
```

Then you can do the following to bulk-export OpenSCAD files:
```
for f in buildables/*.scad; do dest="$(echo "$f" | sed 's/.scad/.gltf/')"; openscad "$f" -o /tmp/tmp.stl && assimp export /tmp/tmp.stl "$dest"; done; rm buildables/*.bin
```
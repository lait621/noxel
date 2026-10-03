# logo

The Noxel mark: an isometric cube in three tones on a dark tile, drawn from
`logo.py`.

```bash
python3 tools/logo/logo.py --out tools/logo
```

Reproducible and **crisp at every size**, by the same rule the engine's art
follows. The mark is authored on a 32x32 logical grid with integer vertices
only, and every icon is that grid scaled by a whole number — so 16, 32, ... 1024
are all exact, and none of them is a resample.

An earlier draft cut two cells out of the cube's roof to say "voxel". It read as
damage: at icon sizes the top face is a dozen pixels across, so a gap in it is
not a grid, it is a chip out of the shape.

`Noxel.icns` is the macOS icon set, built with `iconutil`. `scripts/build-dist.sh`
in the Noxel Valley repository installs it as the application icon.

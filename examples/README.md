# Synthetic assets

Both examples are generated entirely by
[`tools/generate_synthetic_asset.py`](../tools/generate_synthetic_asset.py).
They contain no trained model, captured subject or external dataset.

## `minimal-sh0`

Three static Gaussians exercise the explicit-v1 header, geometry record and
SH0-only loader path. This is a format smoke test, not a visual reference.

## `synthetic-motion-sh3`

This 4,096-Gaussian scene is an analytic **4D calibration target**, not a
decorative sample. At the fixed 640×360 camera and reference time
`t = 0.5084746` (declared as manifest `time.initial`), its structure is:

```text
cyan solid corner ┌─ 4DGS TEST ─────────────── SH + three probes
                  │
 RGBW swatches    │  RGB camera axes   overlapping F/M/N depth cards
                  │
                  │              T ─── cyan gate ─── yellow mover ───
                  └────────────────────────────── dashed / magenta cut
```

The regions are deliberately separated and asymmetric so a 640×360 frame is
readable without a UI overlay:

| Region | Expected observation | Renderer invariant |
| --- | --- | --- |
| Asymmetric frame | Solid cyan upper-left; dashed lower/right; magenta lower-right cut | Orientation, principal point and aspect-preserving fit |
| `4DGS TEST` | Small white glyphs remain distinct | Projected covariance and alpha footprint |
| RGB axes | Red `+X` points right, green `+Y` points down in the fixed camera, blue `+Z` recedes | Coordinate signs, quaternion rotation and covariance projection |
| RGBW swatches | Four rectangular blocks remain in red/green/blue/white order | SH0/RGB channel order and display working space |
| F/M/N cards | Orange `N` covers green `M`, which covers blue `F`, only where they overlap | Global depth sort and front-to-back compositing; record order is deliberately interleaved |
| Timeline | Yellow diamond travels left-to-right over `[0,1]`; magenta/cyan/violet rings peak at `t = 0.2/0.5/0.8` | Explicit velocity, temporal gating and culling |
| SH probes | Three neutral discs change color differently while orbiting | Non-constant SH coefficient order and real-basis signs across degrees 1, 2 and 3 |

The fixed camera uses a positive-Z, screen-Y-down convention. Orbiting releases
the fixed view: the blue Z stem and F/M/N layers should separate by parallax,
while the SH probes change color. Returning to the fixed camera makes golden
comparison meaningful again.

The exact region counts and intended invariants are duplicated in the
manifest's procedural provenance. Regenerate only by explicit replacement:

```bash
python3 tools/generate_synthetic_asset.py --force
python3 tools/generate_synthetic_asset.py --check
```

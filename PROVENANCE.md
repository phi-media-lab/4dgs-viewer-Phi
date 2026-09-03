# Provenance

Status: incomplete; publication is blocked until every row has a human sign-off.

| Area | Current origin | Publication rule |
| --- | --- | --- |
| Player Rust host | Project-workspace implementation | Confirm owner authorization and third-party notices |
| Player WGSL except `preprocess.wgsl` | Project-workspace implementation informed by published 3DGS/4DGS mathematics | Cite papers and complete expression review |
| `player/shaders/preprocess.wgsl` | Modified gsplat projection plus source-workspace WGSL/4D extensions | Apache-2.0 header and pinned upstream provenance; see `THIRD_PARTY.md` |
| Browser receiver | Project-workspace implementation | Confirm owner authorization and remove deployment identifiers |
| Asset format | Project explicit-linear interchange contract | Publish specification and deterministic conformance fixtures |
| Synthetic examples | Generated procedurally by this repository | No external model, image, or dataset input |
| WebGPU lessons | Project course implementation | May acknowledge WebGPU Unleashed as format inspiration; do not copy unlicensed code, text, or media |

Relevant public research and software references still need to be frozen in a
future `CITATION.cff`; its absence remains a release blocker rather than an
implied completed artifact.

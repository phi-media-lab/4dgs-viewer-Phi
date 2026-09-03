# Open-source packaging plan

## Decision

Publish one source repository with two independently runnable products and one
small conformance layer:

```text
                              explicit-v1 + fixtures
                               /                 \
                              /                   \
Remote Frame Mode Player (Linux)                    WebGPU lessons (browser)
Rust / wgpu / WGSL                                  JavaScript / WGSL / Vite
Vulkan → DMA-BUF → VA-API → WebRTC                  VS Code → HMR → WebGPU canvas
```

The products share terminology, specifications and tests, but not a runtime.
Neither product is a prerequisite for running the other.

## Public units

### Remote Frame Mode Player

Included:

- `player/` host, shaders, thin browser receiver and operational scripts;
- `asset-format/` as the input contract;
- `examples/` synthetic-only conformance assets;
- the asset validation tools and applicable license/provenance documents.

The first release should be source-only. A portable Linux binary bundle is a
separate deliverable because Vulkan, VA-API, GStreamer and driver redistribution
have different compatibility and licensing constraints.

### WebGPU lessons

Included:

- lesson source, explanations and exact WGSL files;
- the small shared WebGPU boundary in `lessons/infra/`;
- Vite development/build configuration and source-contract tests;
- a static-site build produced from the same tagged source.

VS Code is the code surface. The browser is only the execution, visualization
and interaction surface. The site must not embed an editor or imitate an IDE.
The initial staging slice contains the audited course shell and Lesson 00, not a
claim that the complete curriculum has already been extracted; see
`lessons/ROADMAP.md`.

### Shared conformance layer

`asset-format/`, `examples/`, `tools/` and `tests/` make implementation claims
checkable. They are part of the Player source release, while the lesson site may
link to the specification without importing the Player runtime.

## Release sequence

1. Freeze the copyright owner and top-level license. The public repository and
   namespace are `phi-media-lab/4dgs-viewer-Phi` and `phi.*`.
2. Close file-level provenance and third-party-license review.
3. Create the first clean Git commit; do not import the private workspace history.
4. Pass portable source, asset, schema and lesson build gates from a fresh clone.
5. Pass the real-browser Lesson 00 gate on the Apple GPU lane.
6. Pass the one-frame and end-to-end Player gates on the AMD Linux lane.
7. Produce release archives only from the reviewed Git tag, never from the
   working directory.
8. Scan both Git history and the exact archives, generate SBOMs, then publish.

## Evidence boundary

Portable CI proves source properties. Hardware lanes prove only the exact
adapter/driver/browser combinations recorded in their receipts. A result is not
called supported merely because it compiled, and a newly generated golden is
never compared against itself in the same validation run.

Raw hardware logs, machine paths, private addresses and unapproved model assets
remain CI artifacts or private evaluation inputs. Only deliberately reviewed,
synthetic evidence may be promoted into the source tree.

## Decisions still owned by the maintainer

The staging work deliberately cannot choose these on the maintainer's behalf:

- top-level OSI license compatible with the Apache-2.0-derived shader file;
- copyright owner and contributor authorization;
- whether a reviewed synthetic golden belongs in Git or in a versioned release
  artifact.

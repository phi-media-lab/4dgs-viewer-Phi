# Architecture

This repository contains two products that share first-principles GPU concepts,
not a single coupled application.

## Remote Frame Mode Player

```text
explicit-v1 manifest + geometry + optional SH3
                         │
                         ▼
                  Rust host control
                         │
        wgpu resource and command submission
                         │
                   WGSL compute rasterizer
                         │
                 Vulkan BGRA image (GPU)
                         │ external memory, linear DMA-BUF
                         ▼
              GStreamer / VA-API H.264
                         │
                      WebRTC
                         │
                         ▼
             Chrome video + input receiver
```

The Linux process owns the GPU device, Gaussian resources, frame scheduling, rasterization, encoder and command order. The browser owns video decode/presentation and camera/time input. Gaussian payloads are never sent to the browser in this mode.

The critical invariant is the frame boundary between Vulkan and VA-API: a
linear DMA-BUF is exported and consumed without a CPU pixel copy. If that
interop is unavailable, the player fails closed. Full-frame pixel readback
exists only in explicit one-frame validation mode; streaming maps small counter,
timestamp and flag buffers for telemetry.

The HTTP signaling server is single-peer and loopback-only. An SSH local port
forward can expose that HTTP endpoint to a LAN receiver, but it does not carry
WebRTC media or DataChannels. Those use the renderer's directly reachable UDP
host ICE candidate. Non-direct networks require ICE-server support and may
require TURN; neither is configured in v0.1. Public signaling, authentication
and multi-tenancy are also unsupported.

## WebGPU lessons

```text
VS Code Insiders                      Browser
----------------                     ----------------
JavaScript host code   ── Vite ──►   WebGPU execution
WGSL shader source    + live reload   canvas output
lesson text                           small PASS/FAIL surface
```

VS Code is the reading and editing surface. The browser is the execution,
visualization and interaction surface. The lesson site contains no embedded
editor, file tree or notebook UI.

The seven directly runnable lessons form this dependency chain:

```text
00 WebGPU environment
   └─ 01 analytic Gaussian footprint
       └─ 02 3D covariance → camera Jacobian → 2D conic
           └─ 03 transparent order + transmittance blend
               └─ 04 explicit motion + temporal opacity
                   └─ 05 active/visible compaction + indirect draw
                       └─ 06 validate → project → sort → render
```

Each lesson owns its JavaScript resource setup, WGSL stages and command
submission. `infra/` supplies only the adapter/device boundary, canvas sizing,
checked error scopes and the machine-readable `window.__LESSON_RESULT__`
surface. Mathematical stages remain visible in lesson-owned `reference.js` and
WGSL files.

Course inputs are source constants or deterministic procedural records. The
lesson tree contains no model or media payload. Lesson 06's external manifest
slot is `null`, so its complete pipeline executes without an asset request.

## Shared boundary

The two products share the first-principles vocabulary of host control, shader compilation, resource layout and validation. They do not share a runtime device or frontend framework. The course is browser WebGPU; the Player is Linux wgpu/Vulkan with a thin WebRTC receiver.

Training, model conversion and Client GS streaming are outside this repository.

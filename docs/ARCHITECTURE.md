# Architecture

This repository contains two products that share concepts and tests, not a single coupled application.

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

The critical invariant is the frame boundary between Vulkan and VA-API: a linear DMA-BUF is exported and consumed without a CPU pixel copy. If that interop is unavailable, the player fails closed. Readback exists only in explicit one-frame validation mode.

The HTTP signaling server is single-peer and loopback-only. An SSH local port
forward can expose that HTTP endpoint to a LAN receiver, but it does not carry
WebRTC media or DataChannels. Those use the renderer's directly reachable UDP
host ICE candidate. NAT/cloud traversal requires TURN; TURN, public signaling,
authentication and multi-tenancy are not part of v0.1.

## WebGPU lessons

```text
VS Code Insiders                      Browser
----------------                     ----------------
JavaScript host code   ── Vite ──►   WebGPU execution
WGSL shader source       + HMR        canvas output
lesson text                           small PASS/FAIL surface
```

VS Code is the reading and editing surface. The browser is the execution, visualization and interaction surface. The lesson site intentionally does not reproduce an editor, file tree or notebook UI.

Lesson 00 proves only the smallest WebGPU host/shader chain. Later lessons may build toward Gaussian projection, covariance, compositing, sorting and time, but each lesson must remain directly runnable and publish a machine-readable `window.__LESSON_RESULT__`.

## Shared boundary

The two products share the first-principles vocabulary of host control, shader compilation, resource layout and validation. They do not share a runtime device or frontend framework. The course is browser WebGPU; the Player is Linux wgpu/Vulkan with a thin WebRTC receiver.

Training, model conversion from third-party projects, Client GS streaming, real-person assets and deployment infrastructure remain outside this repository.

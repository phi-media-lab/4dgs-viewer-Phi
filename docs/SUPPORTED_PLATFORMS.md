# Supported platforms

## Remote Frame Mode Player v0.1

| Role | Supported | Status |
| --- | --- | --- |
| Renderer | Ubuntu 24.04 x86_64, AMD RADV Vulkan, linear DMA-BUF, radeonsi VA-API, GStreamer 1.24 | Hardware lane required |
| Receiver | Current Chrome/Chromium with H.264 WebRTC support | Real-browser lane required |
| Signaling | One receiver through loopback HTTP or an SSH local port forward | Supported scope |
| WebRTC media/control | Direct UDP host ICE candidate on a reachable LAN | Supported scope |
| NAT/cloud relay | TURN | Not implemented in v0.1 |
| macOS | Receiver only | No native renderer |
| Windows | Out of scope | No support claim |
| NVIDIA/Intel Linux | Unverified | Contributions require interop evidence |

The code may compile elsewhere; compilation is not a hardware support claim. A supported renderer must pass the Vulkan external-memory, linear modifier, VA-API roundtrip and reviewed image-reference gates.

## WebGPU lessons

Lesson source/build tests run on macOS and Linux. Runtime support requires a current WebGPU-capable browser and GPU adapter. A software build or DOM-only browser test does not certify WebGPU execution.

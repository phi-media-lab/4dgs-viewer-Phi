# 4DGS Viewer Phi

4DGS Viewer Phi 包含两个可以独立运行的部分：

- `player/`：基于 Rust、wgpu、WGSL、Vulkan、DMA-BUF、VA-API 和 WebRTC
  的 Linux Remote Frame Mode Player；
- `lessons/`：由七课组成的 WebGPU + WGSL 第一性原理课程，从 Device 创建
  一直推进到完整的 synthetic 4DGS 渲染链路。

Player 在 Linux GPU 上渲染 explicit 4D Gaussian asset，并把编码后的帧发送给
轻量浏览器接收端。课程直接呈现 JavaScript 宿主代码、WGSL 阶段、GPU 命令和
数值验证；VS Code 负责代码，浏览器负责运行和显示。训练、模型转换和客户端
Gaussian 串流不在本仓库范围内。

## WebGPU 课程

需要 Node.js `^20.19.0` 或 `>=22.12.0`，以及支持 WebGPU 的
Chrome/Chromium。

```bash
code-insiders lessons/4dgs-viewer-phi.code-workspace
cd lessons
npm ci
npm run dev:open
```

课程目录位于 `http://127.0.0.1:5173/`，包含：

```text
00 Environment       WebGPU Device、Shader、Pipeline 与命令提交
01 One Gaussian      解析 Gaussian footprint 与 CPU/WGSL 一致性
02 Projection        3D 协方差、相机 Jacobian 与 2D conic
03 Order and blend   正确透明顺序与刻意反转的错误结果
04 Explicit time     静态/运动 primitive 与时间 opacity
05 Active set        GPU active/visible 压缩与 indirect draw
06 Complete pipeline 验证、投影、排序和渲染的完整组合
```

每一课直接持有自己的 WebGPU pipeline，并给出可证伪的
`window.__LESSON_RESULT__`。课程输入是源码内常量或 JavaScript 程序化记录，
运行时不需要模型、媒体文件或外部资产请求。修改源码后，Vite 会自动更新浏览器，
无需手动刷新。课程说明与验证命令见
[`lessons/README.md`](lessons/README.md)。

## Remote Frame Mode Player

参考渲染端组合是 Ubuntu 24.04 x86_64、AMD RADV/VA-API 和 GStreamer
1.24。macOS 仅作为接收端；其他渲染端 GPU 尚未验证，Windows 不在支持范围内。

安装 [`player/README.md`](player/README.md) 中列出的系统依赖后运行：

```bash
cd player
cargo test --locked
./scripts/run.sh
```

服务监听 `127.0.0.1:4191`。另一台机器上的浏览器可以通过 SSH 转发信令：

```bash
ssh -L 4192:127.0.0.1:4191 user@renderer-host
```

然后用 Chrome 打开：

```text
http://127.0.0.1:4192/?jitter_buffer_ms=browser
```

SSH tunnel 只承载 HTTP 信令。WebRTC 媒体和控制仍要求浏览器能够通过 UDP
直达渲染端，详见 [`player/README.md`](player/README.md#run-the-webrtc-player)。

浏览器发送相机和时间控制；Linux 进程负责渲染、编码与帧调度。GPU 帧通过
linear DMA-BUF 跨越 Vulkan/VA-API 边界；如果互操作条件不成立，程序会直接
失败，不会悄悄切换到 CPU 像素拷贝。

## Asset 一致性验证

仓库包含严格的 explicit-4DGS 资产格式和两个确定性 synthetic 示例：

```bash
python3 tools/generate_synthetic_asset.py --check
python3 -m unittest discover -s tests -v
python3 tools/validate_asset.py \
  examples/minimal-sh0/manifest.json \
  examples/synthetic-motion-sh3/manifest.json
```

完整格式见 [`asset-format/explicit-v1.md`](asset-format/explicit-v1.md)，校准图形
及其预期视觉特征见 [`examples/README.md`](examples/README.md)。

## 目录结构

```text
player/        远端渲染器与轻量 WebRTC 浏览器接收端
lessons/       WebGPU 课程源码与开发环境
asset-format/  explicit 4DGS manifest 和二进制合同
examples/      确定性 synthetic 一致性资产
tools/         资产和 Schema 验证工具
evidence/      Native reference/comparison receipt Schema
docs/          架构、平台支持和验证模型
```

完整技术边界见 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)、
[`docs/SUPPORTED_PLATFORMS.md`](docs/SUPPORTED_PLATFORMS.md) 和
[`docs/VALIDATION.md`](docs/VALIDATION.md)。

# 4DGS Viewer Phi

4DGS Viewer Phi 包含两个可以独立运行的部分：

- `player/`：基于 Rust、wgpu、WGSL、Vulkan、DMA-BUF、VA-API 和 WebRTC
  的 Linux Remote Frame Mode Player；
- `lessons/`：从第一性原理讲解 WebGPU + WGSL 的课程，目前包含 Lesson 00。
  VS Code 负责代码，浏览器负责运行和显示。

Player 在 Linux GPU 上渲染 explicit 4D Gaussian asset，并把编码后的帧发送给
轻量浏览器接收端。Lesson 00 以 RGB 三角形直接展示最小 WebGPU 宿主控制链和
Shader 链。训练、模型转换和客户端 Gaussian 串流不在本仓库范围内。

## WebGPU Lesson 00

需要 Node.js `^20.19.0` 或 `>=22.12.0`，以及支持 WebGPU 的
Chrome/Chromium。

```bash
code-insiders lessons/4dgs-viewer-phi.code-workspace
cd lessons
npm ci
npm run dev
```

打开 `http://127.0.0.1:5173/00-environment/`。在 VS Code 中修改
[`lessons/00-environment/main.js`](lessons/00-environment/main.js) 或
[`lessons/00-environment/environment.wgsl`](lessons/00-environment/environment.wgsl)
后，Vite 会自动更新浏览器里的渲染结果。验证命令见
[`lessons/README.md`](lessons/README.md)，课程正文见
[`lessons/00-environment/LESSON.md`](lessons/00-environment/LESSON.md)。

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

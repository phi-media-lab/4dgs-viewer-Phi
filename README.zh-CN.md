# 4DGS Viewer Phi

> 这是预发布源码树。在 `docs/RELEASE_BLOCKERS.md` 中的许可与来源门关闭前，不得创建正式 release。

本仓库只抽取两个相互独立的产物：

- `player/`：Linux Vulkan/DMA-BUF/VA-API Remote Frame Mode Player；
- `lessons/`：使用原生 WebGPU + WGSL 的第一性原理课程，VS Code 阅读代码，浏览器显示渲染结果。

这里不会包含客户端 Gaussian 串流实验、训练代码、真实人物 checkpoint、私有网络 receipt 或机器配置。

当前第一阶段先建立干净仓库、严格资产合同、原创 synthetic SH0/SH3 示例、Player 离线验证入口，以及完全不依赖 Client GS 的 Lesson 00。此时还不是正式 release；公开项目身份已经确定为 `phi-media-lab/4dgs-viewer-Phi`，OSI 许可证和版权主体仍需 maintainer 确认。

两个产物的边界、发布单元和 gate 见
[`docs/OPEN_SOURCE_PACKAGING.md`](docs/OPEN_SOURCE_PACKAGING.md)。

## 生成并验证示例资产

```bash
python3 tools/generate_synthetic_asset.py --check
python3 tools/validate_asset.py examples/minimal-sh0/manifest.json
python3 tools/validate_asset.py examples/synthetic-motion-sh3/manifest.json
python3 -m pip install --require-hashes -r tools/requirements-schema.lock
python3 tools/check_json_schema.py examples/*/manifest.json
python3 tools/audit_public_tree.py
```

只有在有意替换已提交的 procedural fixtures 时才使用 `--force`；日常验证使用
`--check`，不会修改文件。

## 运行 Lesson 00

```bash
cd lessons
npm ci
npm run dev
```

浏览器只显示 canvas、最小状态和错误；代码与课程说明保留在 VS Code 中。

## Player 支持边界

v0.1 的目标渲染端是 Ubuntu 24.04 x86_64、AMD RADV/VA-API 与 GStreamer
1.24；抽取后的单帧 Vulkan/DMA-BUF/VA-API gate 已在该组合运行，重新完成
端到端 Chrome/Chromium session 仍是 release gate。其他 Linux GPU 尚未验证；
macOS 仅作为浏览器接收端，Windows 不在范围内。

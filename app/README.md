# LectureEdit 桌面应用

此目录包含 React 19 + TypeScript 界面、Tauri 2/Rust 桌面后端、SQLite 持久化、原生音频采集、本地 Qwen3-ASR 推理与可选 Soniox stt-rt-v5 WebSocket 转写。DeepSeek V4.1 Flash 提供转写轻度整理、选文翻译和图片公式识别。Motion 负责界面动效，Floating UI 负责就地菜单的定位与焦点交互。

课程分组、主题偏好与各次课堂记录保存在同一 SQLite 持久化状态中。完整文稿编辑器按 segment 写回发生变化的句子，保留 sample 音频锚点、搜索定位和人工修订历史。Soniox 使用最终词元的真实时间切分自然句；Qwen 最终窗口按自然句原子拆分。界面还会为历史长段生成不修改持久化数据的句内播放锚点。跨课堂搜索使用当前修订正文和笔记。macOS 的精排 PDF 导出生成适合打印和分享的 A4 矢量页面，支持中英文、KaTeX 公式和本地图片。

用户词表保存在设置中。Qwen 请求把词表写入 system context，Soniox WebSocket 配置把标准词写入 `context.terms`；`常见误写 → 标准拼写` 条目还会在 domain 层按整词边界统一落稿，因此本地、云端和重试任务共享同一拼写规则。

自动轻度整理只在 Qwen 或 Soniox 确认句段边界后触发。后台请求使用当前 `displayText`，允许继续整理已有人工修订；`machineRevision`、`userSeq` 和输入快照共同阻止过期响应覆盖后续编辑。结果作为新的 `Correction` 写入可撤销历史，`machineText` 始终保留原始机器识别内容。

## 开发环境

- Node.js 20.19+ 与 npm
- Rust stable
- macOS 13+：Xcode Command Line Tools、CMake、Git
- Windows：Visual Studio 2022 C++ Build Tools 与 Windows SDK

模型权重保存在应用数据目录，独立于源码和应用包。用户选择本地识别后，可按需下载约 1.019 GB 的 Qwen3-ASR-0.6B Q8 decoder 和 mmproj 文件；界面显示实际下载进度，中断后可继续，完整性校验通过后加载。

## 安装依赖并运行

macOS：

```bash
cd app
npm ci
./scripts/build-native.sh
npm run tauri dev
```

Windows（PowerShell）：

```powershell
cd app
npm ci
./scripts/build-native.ps1
npm run tauri dev
```

浏览器中的 Vite 页面使用固定演示数据。录音、文件导入、回放、持久化、本地推理和导出需要 Tauri 桌面进程。

在 macOS 桌面应用中打开一节课堂，从“导出”菜单选择“精排 PDF”，再指定以 `.pdf` 结尾的保存位置。导出使用保存对话框关闭后的最新课堂内容，单份文档最多 500 页。精排 PDF 当前仅支持 macOS。Windows 版尚未完成真实运行验证；浏览器预览不执行文件导出。

## 构建原生组件与应用

macOS 构建脚本会编译 ScreenCaptureKit 系统音频助手，检出固定提交的 llama.cpp，构建 `llama-server`，并把可移植的原生文件复制到 Tauri resources：

```bash
cd app
./scripts/build-native.sh
npm run tauri build
```

Windows 原生组件的构建说明见 [native/README.md](native/README.md)。发布前需在目标平台完成真实麦克风、系统声音和打包应用验证。

## 验证

```bash
cd app
npm test
npm run build
cd src-tauri
cargo test --lib
cargo check --bin lectureedit-probe
```

生产状态使用 camelCase JSON 快照保存在 SQLite 中。音频按 16 kHz 单声道 16-bit PCM 分 run 保存；本地识别的最终窗口单独持久化，重启后继续。Soniox 逐次读取已保存的音频，云端任务通过用户主动操作启动，密钥保存在进程内存中。

打包时 `scripts/collect-licenses.mjs` 会收集依赖的版权与许可证通知。集成命令与状态结构见 [API.md](API.md)。

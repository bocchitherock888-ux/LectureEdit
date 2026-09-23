# LectureEdit 0.1.1 · Windows x64 构建执行手册

日期：2026-09-20。输入为用户提供的 `LectureEdit-0.1.0-source.zip`。本目录已经包含修改后的完整源码。

**目标交付：Intel/AMD Windows x64 的 NSIS 安装器。当前包交付源码、配置、脚本和本次验证记录；Windows 安装器由以下流程生成。**

## 0. 固定决策

| 项目 | 本次选择 |
|---|---|
| 构建主机 | 优先 GitHub Actions `windows-2022`；也支持本地 Intel/AMD Windows x64 |
| 用户安装验收 | Windows 11 x64 实机；其他 Windows 版本单独测试 |
| Rust 目标 | `x86_64-pc-windows-msvc` |
| Rust 工具链 | `1.92.0-x86_64-pc-windows-msvc`，沿用源项目记录的 Rust 版本 |
| Node | 24.x x64；预检也接受 22.x 中的 22.12+ |
| C++ | Visual Studio 2022，MSVC v143，Windows SDK，CMake |
| 依赖安装 | `npm ci`；所有 Cargo 构建使用 `--locked` |
| 安装格式 | NSIS `.exe`；当前用户安装；英文/简体中文安装界面 |
| 本地推理 | 原项目固定 commit 的 llama.cpp；Qwen3-ASR-0.6B；CPU 构建 |
| CPU 默认档 | `multi`，包含基础 x64 与优化 CPU 后端，由运行时选择 |
| CPU 备用档 | `baseline`，明确关闭扩展指令集，单独标记产物 |
| 系统声音 | 原项目 Rust WASAPI loopback 助手，单独编成 Windows x64 |
| WebView2 | 默认按需联网安装；可选离线 WebView2 安装组件 |
| 代码签名 | 本次生成未签名测试包，签名和公开发布另行安排 |
| 模型权重 | 安装后由应用下载/验证；源码包及安装包均不捆绑权重 |

x64 的目标是 Intel/AMD 的 64 位架构。Windows ARM64 是独立目标，本轮保持 x64；实际 CPU 速度仍需用音频实测。Tauri 的 Windows 原生构建和 NSIS 路径见 [Tauri installer](https://v2.tauri.app/distribute/windows-installer/)。

## A. 在 Mac 上操作，交给 GitHub Windows 编译（推荐）

Mac 负责保存和提交源码，Windows runner 负责 C/C++、Rust、录音助手和安装器构建。该路线沿用原项目技术栈，避免在 Mac 配置一整套 Windows 交叉工具链。[官方构建说明](https://v2.tauri.app/distribute/windows-installer/)

### A1. 先核对目录

解压后找到本文件所在的 `LectureEdit/`。**Git 仓库根目录应直接包含 `app/`、`.cargo/` 和 `.github/`。**

```text
LectureEdit/
  .github/workflows/windows-x64.yml
  .cargo/config.toml
  app/package.json
  app/scripts/windows/Build-Windows.ps1
  app/src-tauri/tauri.windows.conf.json
  WINDOWS_EXECUTOR_PROMPT.md
```

完整交付包外层还有中文入口、测试日志和补丁。上传源码时，选内部的 `LectureEdit` 目录。Finder 对隐藏文件的显示设置与 Git 是否提交它们是两回事；用终端 `ls -la` 核对。

### A2. 使用已获授权的代码仓库

已有仓库：在它的独立工作副本或新分支上应用本包。工作副本存在未提交变更时，先保存这些变更。不要使用 `reset --hard`、`clean -fdx`、强制推送或覆盖主分支。

新仓库：先由用户确认名称与可见性，建议使用私有仓库。源码中可以包含产品实现；真实课堂录音、数据库、API Key 和约 1 GB 的模型文件应保留本地。GitHub Actions 的配额、组织策略和可能费用以该账号当时的设置为准。

从准确的仓库根目录执行以下步骤。已有 `windows-x64` 分支时，切换到它；新建分支时使用：

```bash
git status --short
git switch -c windows-x64
# 将本包中的源码变更放入该工作副本，或从交付包内层目录开始。
git diff --stat
git add .github .cargo AGENTS.md app WINDOWS_*.md SOURCE_EXPORT_ORIGINAL_MANIFEST.sha256
# 首次创建的仓库还应加入原有 tests/、scripts/、reference/、spikes/ 等完整源码。
# 提交前查看 git diff --cached --stat，确认没有录音、模型或密钥。
git commit -m "Prepare Windows x64 native build and installer"
git push -u origin windows-x64
```

对首次新建仓库，用 `git add .` 前检查 `.gitignore` 和 `git status --short`，确认加入整个项目。上面特定路径的 add 命令适合已有项目升级，并不涵盖新仓库的所有原始源码。

`windows-x64` 分支的这次推送会触发 **Windows x64 installer** 工作流。默认采用 `multi` CPU 档和联网 WebView2 引导程序。

### A3. 查看构建与取回文件

在 GitHub 仓库页面打开 **Actions → Windows x64 installer → 本次 commit 对应的运行**。成功后在页面底部 Artifacts 下载名称类似：

```text
LectureEdit-windows-x64-multi-运行编号
```

解压其中的最新构建目录，应看到：

```text
LectureEdit_0.1.1_windows-x64_multi_setup.exe
BUILD-INFO.json
BINARY-AUDIT.json
native-audit.json
build.log
SHA256SUMS.txt
```

也可以让执行模型在已登录 GitHub CLI 的终端中读取准确运行 ID：

```bash
gh auth status
gh run list --branch windows-x64 --commit "$(git rev-parse HEAD)" \
  --json databaseId,workflowName,status,conclusion
# 选择 workflowName 为 Windows x64 installer 的这一条运行。
gh run watch <上一步返回的databaseId> --exit-status
gh run download <同一个databaseId> --dir windows-artifacts
```

`databaseId` 必须来自真实工具输出。Artifacts 中 diagnostics 包只含日志/报告；应将成功的 installer artifact 交给用户。

构建失败时打开日志中最早失败的阶段，下载 `LectureEdit-windows-x64-diagnostics-运行编号`。该阶段通过前，保持后续阶段为 `not_run`。

### A4. 手动选择备用档或离线组件

工作流文件位于默认分支后，Actions 页面可以显示 **Run workflow**。选需要构建的分支，选择 `cpu_profile=baseline` 或勾选 `offline_webview2`。GitHub 对手动触发的默认分支要求见 [GitHub manual workflow](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow)。

文件暂时仅在 `windows-x64` 分支时，默认档先通过 push 自动构建。执行模型不应为了显示按钮擅自推送或合并到主分支。备用档也可以在该分支调整工作流的默认值后经审核提交，再触发 push；变更必须记录。

## B. 在本地 Windows x64 编译

### B1. 安装构建工具

以下命令由执行模型在 Windows 执行。软件安装可能显示 UAC 或需要用户同意。优先使用已有工具，缺少的再安装。无需关闭 Defender、SmartScreen 或系统安全设置。

在 Windows Terminal 的 PowerShell 中按需执行：

```powershell
winget install --id Microsoft.PowerShell --exact
winget install --id Git.Git --exact
winget install --id OpenJS.NodeJS.LTS --exact
winget install --id Kitware.CMake --exact
winget install --id Rustlang.Rustup --exact
winget install --id Python.Python.3.12 --exact
winget install --id Microsoft.VisualStudio.2022.BuildTools --exact --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

安装完成后关闭旧终端，重新打开 **PowerShell 7 x64（pwsh）**。若 VS 安装器要求重启，先完成重启。`winget` 缺失时使用各软件的官方安装器；具体链接见本手册“来源”。VS 安装器里选择 Desktop development with C++，包含 MSVC v143 x64/x86 和 Windows SDK。[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

`NodeJS.LTS` 是安装渠道。脚本仍会检查实际安装的主版本。将来该渠道切换主版本时，使用官方 24.x x64 安装器，保持本方案选定版本系列。禁止用修改锁文件来绕过版本检查。

CMake 使用 VS2022 generator，故无需手动配置 Ninja。GitHub Windows runner 已有主要工具，流程会准备固定 Rust 工具链。构建使用的 Python 仅用于参考测试，终端用户安装应用无需 Python。

### B2. 解压到一个简短路径

建议 `C:\src\LectureEdit`。此目录下应能找到 `app\package.json`，保留隐藏目录 `.cargo`、`.github`。构建路径先采用简短英文目录；安装后的中文用户名、中文课程名和含空格路径在验收阶段测试。

先执行预检：

```powershell
cd C:\src\LectureEdit
pwsh -NoProfile -ExecutionPolicy Bypass -File .\app\scripts\windows\Test-WindowsBuildEnvironment.ps1 -InstallToolchain
```

这里的执行策略参数只作用于启动的 PowerShell 进程，脚本不修改系统执行策略。预检会检查脚本语法、x64 主机/进程、Node、Python、VS2022、Windows SDK 和 Rust；它只为当前进程及其子进程选择 Rust 工具链和编译环境。

### B3. 执行完整构建

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\app\scripts\windows\Build-Windows.ps1 -CpuProfile multi -Jobs 4
```

同一源码目录每次运行一个构建任务。资源较少的构建机用 `-Jobs 2`。脚本按以下顺序执行，任何阶段失败立即报告：

1. PowerShell 语法和构建环境预检。
2. `npm ci`，保留现有依赖锁文件。
3. Node 二进制检查工具测试、原项目 Python 测试、Vitest 测试、完整 TypeScript/Vite 构建。
4. 编译 WASAPI 助手；获取固定 llama.cpp commit；构建 Windows CPU 引擎和全部所需 DLL。
5. 检查 PE x64 文件头、DLL 依赖闭包和基础/优化 CPU 后端；只运行 `llama-server --version` 做启动检查。
6. 收集依赖许可资源，再执行主程序、WASAPI crate、合并 crate 的 Rust 测试。
7. Tauri release 构建、许可文件收集和 NSIS 打包。
8. 检查主程序及原生资源、收集安装器、SHA-256、日志和 JSON 报告。

备用基础指令集档：

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\app\scripts\windows\Build-Windows.ps1 -CpuProfile baseline -Jobs 2
```

捆绑 WebView2 离线安装组件：

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\app\scripts\windows\Build-Windows.ps1 -CpuProfile multi -OfflineWebView2 -Jobs 4
```

`OfflineWebView2` 解决的是目标机器安装 WebView2 时的联网需求。构建机仍需获取依赖；Qwen 权重仍需安装后下载，或通过应用模型设置指定经过哈希核对的本地文件。[WebView2 packaging](https://v2.tauri.app/distribute/windows-installer/#webview2-installation-options)

### B4. 产物路径

主要交付目录：

```text
LectureEdit\.windows-output\年月日-时分秒-multi\
```

Tauri 原始安装器目录：

```text
LectureEdit\app\src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\
```

应用主程序：

```text
LectureEdit\app\src-tauri\target\x86_64-pc-windows-msvc\release\lectureedit.exe
```

将 `.windows-output` 这次构建的完整子目录压缩后交付。只复制主程序 exe 会丢失旁边的原生推理、录音和许可资源；一般用户应使用 setup 安装器。

校验安装器：

```powershell
Get-FileHash .\LectureEdit_0.1.1_windows-x64_multi_setup.exe -Algorithm SHA256
```

与同目录 `SHA256SUMS.txt` 对照。哈希用于核对文件，数字签名用于验证发布者，两项应分别记录。NSIS 安装器的引导程序架构与安装进去的主程序架构可以不同；本包对实际 app、helper 和 DLL 检查 `0x8664`。

## C. 安装后的资源布局

`tauri.conf.json` 已经使用资源目录映射，这次保持该约定。安装器应保留：

```text
<安装目录>/
  lectureedit.exe
  native/
    lectureedit-loopback.exe
    qwen/
      llama-server.exe
      [llama/ggml 相关 DLL]
      ggml-cpu-x64.dll          # multi 档
      ggml-cpu-haswell.dll      # multi 档中的优化后端之一
      [其他已构建 CPU 变体]
  licenses/
    DEPENDENCIES.json
    [依赖许可证与声明]
```

实际 DLL 集合来自该 commit 的构建结果；脚本检查文件头和 PE import/delay-import 表。系统 API DLL 来自 Windows，项目私有 DLL 从同目录解析。动态 `LoadLibrary` 的一切路径无法仅凭 import 表证明；因此还需要干净机器安装和真实模型加载验收。

本次给 C++ 配置静态 CRT，Rust 目标也配置 `+crt-static`，并关闭 OpenMP。检查工具发现仍然依赖额外 VCRUNTIME/MSVCP/OpenMP DLL 时会失败，要求修复构建配置。维护依据见 [Rust linkage](https://doc.rust-lang.org/reference/linkage.html)、[CMake MSVC runtime](https://cmake.org/cmake/help/latest/variable/CMAKE_MSVC_RUNTIME_LIBRARY.html)。

## D. 模型、性能与现有功能

模型及音频处理继续使用原项目。下载 manifest 在 `app/src-tauri/src/models.rs`：

| 文件 | 字节数 | SHA-256 |
|---|---:|---|
| Qwen3-ASR-0.6B-Q8_0.gguf | 804749248 | bca259818b50ca7c4c05e9bdb35a5dc04fa039653a6d6f3f0f331f96f6aa1971 |
| mmproj-Qwen3-ASR-0.6B-Q8_0.gguf | 214392480 | 41a342b5e4c514e968cb756de6cd1b7be39eff43c44c57a2ef5fc6522e36603d |

总下载量约 1.02 GB，来源是输入源码的固定清单；本次没有重新下载权重验证远端内容。首次使用需要完成应用内模型准备；已有文件可在设置中指定。模型安装完毕后，选择本地 Qwen 做断网识别测试。

Windows 首包走 CPU；macOS 保留现有 Metal 配置。Windows 实际实时能力、发热和电池表现待硬件实测，不能从 Apple Silicon 表现推算。`multi` 提供 CPU 指令集适配；`baseline` 提供兼容性对照。CUDA/Vulkan 加速作为独立后续工作，当前保持原有接口和依赖稳定。

**原生精排 PDF 是现有源码的 macOS 专用实现。** 本次在 Windows 清楚标明该限制并禁用对应入口，提供“导出课程阅读 HTML → 在 Edge 打开 → 打印为 PDF”的流程；版式须实际核对。Markdown、HTML、WAV、课程包保留原实现，待 Windows 验收。处理 PDF 功能对齐应单列后续任务，避免与首个 Windows 构建同时改动导出架构。

## E. 出错处理顺序

| 错误/现象 | 指定处理 |
|---|---|
| 找不到 `pwsh` | 安装 PowerShell 7 x64，关闭旧终端后重开 |
| VS/`link.exe`/`rc.exe` 缺失 | 修改 VS2022 安装，补 C++ 桌面工作负载及 Windows SDK，重开终端 |
| 缺 Node/Python 或主版本不匹配 | 安装手册中的版本；保留 npm/Cargo 锁文件 |
| 锁文件需要更新 | 停止；记录哪个 manifest/工具链触发它。先检查路径、版本和依赖是否被改动 |
| 外部下载超时 | 记录准确域名/URL/退出码；恢复网络后重跑同一脚本；保持 commit 和证书验证 |
| CMake generator/cache 冲突 | 关闭应用；只移动相应 `app/.native-build/windows-x64-<档位>` 生成目录后重跑 |
| C++ 编译资源不足 | 重跑同档位，使用 `-Jobs 2` 或 `-Jobs 1` |
| `ggml-cpu-x64.dll` 缺失 | 核查 multi 档的 BACKEND_DL 和 CPU_ALL_VARIANTS；保留完整 bin/Release DLL |
| PE 检查发现 ARM64/x86 | 清理错误档位生成目录，在真正 x64 Windows runner 重新构建全部组件 |
| 仍依赖 MSVCP/VCRUNTIME | 检查目标 .cargo 配置、CMAKE_MSVC_RUNTIME_LIBRARY 和旧 CMake 缓存；保持检查开启 |
| 应用启动缺 DLL | 核对安装目录结构、BINARY-AUDIT.json；从本次固定源码重建；不要随意下载同名 DLL |
| 启动引擎非法指令 | 记录 CPU 型号、所选 ggml backend 和日志；构建 baseline 对照；避免宣称所有 x64 已兼容 |
| 空白窗口/WebView2 失败 | 完成 WebView2 安装；检查目标机器环境；可改用 OfflineWebView2 安装包 |
| 麦克风无声 | 确认默认输入设备以及 Windows 桌面应用麦克风权限，再查看应用日志 |
| 系统音频无声 | 在当前默认输出设备播放自有测试音频；核对 WASAPI helper stderr；先用扬声器/有线设备对照 |
| PDF 按钮灰色 | 按当前 Windows 功能说明导出 HTML 后打印；该项为已知功能边界 |
| SmartScreen/发布者提示 | 核对源码、构建记录和哈希；公开发行另做签名。保持系统防护开启 |
| GitHub 找不到 Run workflow | 确认文件所在分支；先使用 windows-x64 分支 push 的自动触发 |
| 下载到日志包但没有 setup | 下载成功运行的 installer artifact；检查 BUILD-INFO.status 与 bundle gate |

任何编译错误都先保留“第一条实际报错及其上下文”。执行模型只修改和错误直接相关的小范围文件，重新运行被影响的测试及完整构建。多处错误同时出现时，优先解决第一个根因。

## F. 完成判定

`BUILD-INFO.json` 中 `status=build_passed_runtime_pending` 只代表指定构建/测试通过。用户可试用状态还要求完成 `WINDOWS_ACCEPTANCE.md` 的实机项目。公开发行还需要记录签名/安装升级策略、系统覆盖、许可和隐私检查。

构建脚本不启动实际录音，也不自动调用付费 API。麦克风、系统音频、长录音和 API 测试须由用户同意后执行，并使用可公开的测试素材。

## 来源（2026-09-20 核对）

- 原项目源码及锁文件是版本和现有功能判断的依据。
- Tauri prerequisites：https://v2.tauri.app/start/prerequisites/
- Tauri Windows installer：https://v2.tauri.app/distribute/windows-installer/
- Tauri CLI/config：https://v2.tauri.app/reference/cli/ ，https://v2.tauri.app/reference/config/
- Windows runner 工具清单：https://github.com/actions/runner-images/blob/main/images/windows/Windows2022-Readme.md
- GitHub 手动运行：https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow
- 固定 llama.cpp 代码：https://github.com/ggml-org/llama.cpp/tree/391fac16460f15233a7740550d858ac96df3419d
- CPU variants 实现：https://github.com/ggml-org/llama.cpp/blob/391fac16460f15233a7740550d858ac96df3419d/ggml/src/CMakeLists.txt
- CMake runtime：https://cmake.org/cmake/help/latest/variable/CMAKE_MSVC_RUNTIME_LIBRARY.html
- Rust static CRT：https://doc.rust-lang.org/reference/linkage.html
- Windows 创建进程标志：https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags
- Windows console handler：https://learn.microsoft.com/en-us/windows/console/setconsolectrlhandler
- 工具官方入口：https://nodejs.org/ ，https://www.rust-lang.org/tools/install ，https://cmake.org/download/ ，https://visualstudio.microsoft.com/downloads/ ，https://learn.microsoft.com/en-us/powershell/scripting/install/installing-powershell-on-windows

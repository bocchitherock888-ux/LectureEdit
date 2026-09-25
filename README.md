<div align="right"><a href="README.en.md">English</a></div>

<img src="app/public/app-icon.png" width="72" height="72" alt="随堂 图标">

# 随堂

[![最新版本](https://img.shields.io/github/v/release/bocchitherock888-ux/LectureEdit?label=release&color=356d73)](https://github.com/bocchitherock888-ux/LectureEdit/releases/latest)
[![MIT License](https://img.shields.io/github/license/bocchitherock888-ux/LectureEdit?color=356d73)](LICENSE)
[![下载量](https://img.shields.io/github/downloads/bocchitherock888-ux/LectureEdit/total?label=downloads&color=356d73)](https://github.com/bocchitherock888-ux/LectureEdit/releases)
[![构建](https://github.com/bocchitherock888-ux/LectureEdit/actions/workflows/verify.yml/badge.svg)](https://github.com/bocchitherock888-ux/LectureEdit/actions/workflows/verify.yml)
![macOS 13+](https://img.shields.io/badge/macOS-13%2B%20Apple%20Silicon-555555?logo=apple)
![Windows x64](https://img.shields.io/badge/Windows-x64-555555?logo=windows)
[![主要语言](https://img.shields.io/github/languages/top/bocchitherock888-ux/LectureEdit?color=356d73)](https://github.com/bocchitherock888-ux/LectureEdit)
[![语言数](https://img.shields.io/github/languages/count/bocchitherock888-ux/LectureEdit?label=languages&color=356d73)](https://github.com/bocchitherock888-ux/LectureEdit)

**边录音，边改转写，边补充课堂笔记。**

随堂（原名 LectureEdit）是一款以本地语音识别为主的课堂录音与转写桌面应用。听课时，可以把需要修正的句子取到左侧编辑，右侧继续呈现新转写。保存后，修订回到原来的位置，相关录音和笔记继续保留。

例如，老师提到的学者名字被识别错了，你可以当场改好；讲到一个公式时，可以补上 LaTeX，或贴入一张课件截图。课后再按关键词找到这一段，回放录音，并继续整理笔记。

[下载与版本](https://github.com/bocchitherock888-ux/LectureEdit/releases) · [开始使用](#开始使用) · [反馈问题](https://github.com/bocchitherock888-ux/LectureEdit/issues)

<!-- 可在这里插入真实桌面应用的截图或动图，展示左侧修订、右侧持续转写的状态。 -->

## 下载与平台

当前版本为 **0.1.6 测试版**。

| 平台 | 当前状态 |
|---|---|
| macOS 13 及以上，Apple Silicon | 提供 DMG / ZIP 测试版安装包。 |
| Windows x64，Intel / AMD 64 位电脑 | 提供未签名 NSIS 测试安装包。真实设备录音与转写验收待完成。 |

Mac 用户从 [Releases](https://github.com/bocchitherock888-ux/LectureEdit/releases) 下载对应安装包，将“随堂”放入“应用程序”后打开。从 0.1.3 及更早版本升级时，“应用程序”里旧的 LectureEdit 可以直接删除，课程数据不受影响。当前 Mac 构建采用临时签名，尚未经过 Apple 公证；确认下载来源后，可按系统提示在“系统设置 → 隐私与安全性”中允许打开。

Windows 用户下载 `Suitang_*_windows-x64_multi_setup.exe` 后运行安装。已装旧版时直接运行新安装包即可：保持默认的“安装前卸载”，安装器会先卸载旧版再装到原位置；如果装的是改名前的 LectureEdit，安装器也会自动把它卸掉。卸载确认页上不要勾选“删除应用程序数据”，课程、录音和已下载的模型就都会保留。

本地模型首次使用时单独下载。安装包用户可以直接在应用内完成配置。

## 开始使用

1. 打开“模型与服务”，选择 Qwen3-ASR 本地识别，点击“下载模型”。当前模型为 Qwen3-ASR-0.6B Q8，模型与配套音频投影文件合计约 1.02 GB。下载显示进度，支持中断后继续，并在完成后校验文件。
2. 等待显示“本地模型已就绪”。新建一个课程分组，再为本次课堂创建记录。英文课程可将识别语言设为“英语”，也可以选择中文或自动检测。
3. 选择音频来源并开始录音。麦克风用于现场课堂，系统声音用于电脑播放的课程；系统声音采集需要相应系统权限，当前仍需补充实机验证。已有录音可通过“更多操作”导入 WAV 文件。
4. 看到需要修改的内容时，点击“修订当前句”或按 `⌘E`。在左侧改好并保存，右侧继续转写。Windows 对应快捷键为 `Ctrl+E`。

录音前请取得相关人员和场所的许可，先用一段短录音检查输入来源和转写效果。

## 当场修订，继续听课

### 取出一句编辑，其他内容继续更新

“修订当前句”会打开独立修订区，草稿自动保存。历史内容也可以通过“取出编辑”修改。你编辑时，录音和后续识别继续运行；保存后，文字合回正文并恢复实时跟随。

人工修订和原始机器识别分别保留。后续识别与人工修改发生冲突时，应用会展示待核对版本，供你选择保留当前文字、采用识别结果或手动合并。修订支持撤销与重做。

翻看前文时，页面保留当前阅读位置。点击“回到实时”可以跟随最新内容。工具栏另有独立的“暂停录音”和“继续录音”按钮。

### 在讲到的位置补充公式、图片和例子

选中一句转写，点击“添加资料”，可以补充文字备注、例子、LaTeX 公式或图片。图片支持选择文件和粘贴；公式会显示为排版后的数学表达式。

资料附在对应的转写位置，并保留音频时间关联。听到一个重要例子时，可以直接补在这一段下面，日后一起查看。

### 为课程设置常用词和标准拼写

在“模型与服务”的“常用词与标准拼写”中加入人名、术语和缩写，每行一条。也可以写出固定纠错规则：

```text
Amartya Sen
Kahneman
con yard → Cournot
```

词表可以分别设置为“所有课程”“当前课程”和“本堂课”，三层叠加生效。同一误写存在不同规则时，优先使用本堂课的设置，再使用当前课程和全局设置。

标准词会作为识别提示，箭头形式的规则用于落稿时的拼写修正。词表由你维护，可随课程内容调整。

## 课后整理与复习

录音结束后，点击“编辑全文”，可以在连续文稿中修改整篇转写。保存时保留原有录音时间关联、资料和修订历史。

通过课程分组管理不同科目，每次课堂记录有独立的名称和日期，也可以重命名或移到其他分组。点击“搜索所有课堂”，或按 `⌘K` / `Ctrl+K`，查找课程名称、转写和笔记；搜索使用已经修订的正文。

搜索结果可以定位到原文，也可以回放相关录音。正文中点击“回放这句”即可复听所选句子。Qwen 和历史长段的句内播放位置采用估算，适合快速复听；具体范围会受识别分段影响。

界面提供浅色、深色和跟随系统三种外观，可收起课程栏，留出更多阅读空间。当前应用界面使用简体中文。

## 本地识别与可选云端工具

默认使用 Qwen3-ASR 在电脑上识别音频。模型准备完成后，本地转写、手动修订、笔记、搜索和导出可以离线使用。纯本地使用时，保持本地识别，并关闭云端辅助功能。

需要云端工具时，可在设置中配置自己的 API Key，每家只需填一个 Key。设置里有每家获取 Key 的分步说明。

云端实时语音识别可以选择以下服务（价格为 2026 年 9 月各家官网标价）：

| 服务 | 模型 | 参考价格 | 适合 |
|---|---|---|---|
| 豆包语音（火山引擎） | 流式语音识别模型 2.0 | 约 ¥1 / 小时 | 中文与中英混说，国内首选 |
| 阿里云百炼 | Qwen-Audio 3.1 实时识别 | 按量计费，新用户有免费额度 | 中英双语提示、大量术语；北京或新加坡地域 |
| ElevenLabs | Scribe v2 Realtime | 约 $0.39 / 小时 | 海外使用、英文课堂 |
| Soniox | stt-rt-v5 | 约 $0.12 / 小时 | 价格最低，海外使用 |

豆包、百炼和 ElevenLabs 按各自最新官方文档接入，已通过协议级测试，尚未用真实账户完成验收。常用词会作为热词传给所选服务。网络中断或服务端到达单次连接时长上限时，应用会自动重连，并从上一句接着转写。

| 工具 | 用途 | 发送到服务商的内容 |
|---|---|---|
| 云端实时语音识别 | 录音时实时转写 | 主动开始、导入或重试的对应音频，以及配置的词表提示。 |
| DeepSeek 轻度整理 | 整理明显误断句、孤立口水词、标点和大小写 | 启用后自动发送已确认的转写段落。 |
| DeepSeek 选文翻译 | 将选中文字译为简体中文或英语，译文可保存为备注 | 主动调用时选中的文字。 |
| DeepSeek 图片公式识别 | 从图片生成可编辑的 LaTeX，预览修改后作为公式保存 | 点击识别时选择的图片。 |

轻度整理保留原始识别稿和可撤销的修订历史。翻译与公式识别结果可以先检查再保存，重要术语和公式请结合原文核对。

选择云端识别后，原始录音仍保存在本机。“暂停云端转写”会停止发送后续音频，本地录音继续保存；恢复连接后可主动重试。通过云端导入的 WAV 按正常播放速度发送，处理时长与录音长度相关。

云端服务费用计入你自己的账户。API Key 使用系统凭据存储，macOS 对应 Keychain，Windows 对应 Credential Manager；课程包和导出文稿排除密钥。

## 导出、备份与继续编辑

从“导出”菜单选择需要的格式：

| 格式 | 用途 |
|---|---|
| Markdown | 导出转写与文字资料，公式保留为 LaTeX 文本，方便继续编辑。 |
| 课程阅读 HTML | 导出可在浏览器中阅读的文字版，包含转写与文字资料。 |
| 精排 PDF | 导出适合阅读、分享和打印的页面，包含公式、图片与页码。当前支持 macOS。 |
| WAV 音频 | 保存课堂录音。 |
| `.lecture` 课程包 | 保留该次课堂的录音、转写、修订和附加资料，可重新导入“随堂”。 |

**完整备份一节课堂，请选择 `.lecture` 课程包。** 图片附件随课程包保存；Markdown 和 HTML 当前以文字内容为主。课程包重新导入后，可以继续编辑并放入所需课程分组。

课堂数据保存在本机应用数据目录。课程包包含录音和附加资料，分享前请核对内容，重要记录请定期另存备份。

## 测试版说明

0.1.6 适合先用短课程记录体验完整流程。Windows 打包与实机运行、系统声音采集、云端服务和 120 分钟连续录音的验收待完成。实际识别速度与效果取决于电脑配置、音频质量和所选服务。

当前文件导入支持 WAV。当前 Mac 安装包面向 Apple Silicon。具体安装文件与后续验证进展以每次 Release 的说明为准。

## 开发与反馈

项目使用 Tauri 2、Rust、React、TypeScript 和 SQLite。本地识别通过 llama.cpp 运行 Qwen3-ASR，数学公式使用 KaTeX 排版。

开发环境和运行命令见 [app/README.md](app/README.md)，Windows x64 构建见 [WINDOWS_BUILD.md](WINDOWS_BUILD.md)。

作者：**醉步羊**（Tipram）。原创代码采用 [MIT](LICENSE)。第三方组件及模型权重见 [NOTICE.md](NOTICE.md)。

[反馈问题](https://github.com/bocchitherock888-ux/LectureEdit/issues)时，请附上应用版本、系统与芯片型号、音频来源、识别引擎、操作步骤，以及实际出现的情况。涉及录音时，优先提供经过许可、去除个人信息的短片段。
====================================================================

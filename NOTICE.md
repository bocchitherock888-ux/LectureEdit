# 许可范围

随堂（原名 LectureEdit）由醉步羊（Tipram）提供的原创代码与文档采用 MIT License，全文见同目录的 [LICENSE](LICENSE)。可在保留版权声明与许可文本的条件下使用、复制、修改和分发；软件按许可中的原样条款提供。

第三方组件保留各自的版权及许可。桌面应用构建时会收集依赖声明，并写入安装包内的 `licenses/` 目录。重新分发安装包时，应一并保留这些声明。

本地语音识别使用 llama.cpp 运行 Qwen3-ASR 模型。模型权重不包含在本仓库或安装包中，由用户在应用内下载，并遵循模型发布方的许可。

Windows 安装包在 `native/qwen/` 中随附 Microsoft Visual C++ 运行库（`vcruntime140*.dll`、`msvcp140*.dll` 等），取自 Visual Studio 2022 的可再发行组件目录，按 Microsoft Visual C++ 可再发行组件的许可条款分发。

云端转写（豆包语音、阿里云百炼、ElevenLabs、Soniox）与可选的 DeepSeek 功能需要你自己的 API 密钥。密钥保存在本机的系统钥匙串（Windows 为凭据管理器）中，不会进入本仓库、课程文件或导出内容。

## 云端服务标识

设置界面在云端识别服务的选项旁显示各家的标识，用于区分服务。这些标识是其所有者的商标：豆包（字节跳动 / 火山引擎）、阿里云百炼（阿里云）、ElevenLabs 和 Soniox。豆包、阿里云百炼、ElevenLabs 的矢量图取自 [LobeHub Icons](https://github.com/lobehub/lobe-icons)（MIT 许可），Soniox 图标取自 soniox.com。

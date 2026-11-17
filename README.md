# VBox

**桌面端 TVBox 兼容播放器 —— Rust + Tauri 2 + Vue 3，自备订阅，装完即用。**

[![Build](https://github.com/q310550690/VBox/actions/workflows/build.yml/badge.svg)](https://github.com/q310550690/VBox/actions/workflows/build.yml)
[![Release](https://img.shields.io/github/v/release/q310550690/VBox)](https://github.com/q310550690/VBox/releases)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)

## 简介

VBox 是一个跨平台（Windows / macOS）的桌面视频播放客户端，兼容 TVBox 的订阅协议与配置格式。
它**不内置任何影视源或内容**，只负责解析用户自己提供的订阅/配置，并把 PC 桌面端缺的那部分体验补齐：
内嵌 mpv 播放、边下边播、全源搜索、直播与 EPG、下载管理、收藏与历史。

项目以「可分发给普通用户」为目标：libmpv、FFmpeg、Python、Node 等原生运行时都可以随安装包内置，
无需用户自己装环境。

## 功能特点

- **订阅与线路**：支持明文 / Base64 / `2423` AES-CBC / `;pk;` AES-ECB 配置与 URL 归一化；多仓配置按「线路切换器」处理，重启恢复上次线路；配置本地缓存 + 离线回退。
- **多源适配**：type=0 苹果CMS XML、type=1 苹果CMS JSON、type=4 HTTP 远程接口、type=3 JS（catvod 子集）/ Python（CPython sidecar）/ drpy2（Node sidecar）/ 原生规则引擎（`csp_XYQHiker`、`csp_XBPQ`）。
- **搜索与解析**：当前源 / 全源并发搜索；type1 解析接口、type4 超级解析、type0 WebView 嗅探的解析链与进度提示。
- **内嵌播放器**：libmpv 的视频画面 + HTML 控制条，支持进度、音量、倍速、全屏、小窗、循环、自动下一集，音轨/字幕轨切换、本地与在线字幕，以及 **Anime4K 实时超分**（播放器页面「画质」开关，三档可选）。
- **边下边播与下载**：HLS 本地代理按需拉段、看过即缓存；「保存本集」支持暂停/继续；下载页支持直链 + HLS 合并、暂停/续传/重试、剧集分组与本地播放。
- **直播与 EPG**：txt / m3u / JSON 直播源、多源换源、密码分组、XMLTV 节目单浏览（直播播放暂未接入）。
- **收藏与历史**：SQLite 持久化，收藏页 / 历史页统一卡片网格与时间筛选，支持继续观看。
- **自包含运行时**：libmpv / FFmpeg / Python / Node 可随包内置（打包时选择），安装包开箱即用。
- **自动更新**：基于 Tauri updater，更新包验签后一键安装并自动重启。
- **调试能力**：网络请求日志与抓包面板、隐藏 WebView 媒体嗅探、本地 `/file` 与 `/proxy` 服务。
- **跨平台**：Windows 全功能；macOS 已支持（依赖系统 / Homebrew 的 libmpv）。

## 技术栈

Rust · Tauri 2 · Vue 3 + Vite + TypeScript + Pinia · Element Plus · libmpv

## 快速开始

环境要求：Node.js 18+、Rust stable；Windows 需 WebView2 + MSVC 构建工具，macOS 需 Xcode Command Line Tools。

```bash
# 1) 安装前端依赖
npm install

# 2) 拉取原生依赖（首次 / 发布前各一次，Windows 全量约 343 MB）
npm run fetch:tools

# 3) 开发运行
npm run tauri dev

# 4) 打包（默认全内置；--bundle none 为极简包）
npm run tauri:build
```

随包依赖、打包参数、macOS 构建与 CI 发布流程见仓库内 `package.json` 脚本与 `.github/workflows/` 配置。

> **安装提示**：Windows 安装包目前**未做代码签名**，SmartScreen 提示"Windows 已保护你的电脑"时
> 选择「更多信息 → 仍要运行」即可；macOS 未公证，首次打开如被拦截请右键 App → 打开。
> 自动更新不受影响（更新包有独立签名校验）。

## 开源协议

本项目采用 **GNU Affero 通用公共许可证第 3 版（AGPL-3.0-only）** 发布，详见 [LICENSE](LICENSE)。

随安装包分发的第三方组件（libmpv / mpv、FFmpeg、CPython、Node.js 等）分别遵循其自身的开源协议，
与 VBox 自身的 AGPL-3.0 许可相互独立；分发或二次打包时请一并遵守。
内置的 Anime4K 超分着色器遵循其 MIT 许可（见 `src-tauri/resources/shaders/anime4k/LICENSE-Anime4K.txt`）。

## 免责声明

VBox 不内置、不提供、不分发任何影视内容或订阅源，仅作为用户自备配置的解析与播放客户端。
请只使用你拥有合法访问权的订阅与内容，并遵守当地法律法规及内容服务条款。

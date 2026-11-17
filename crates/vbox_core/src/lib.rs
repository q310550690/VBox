//! vbox_core：VBox 核心引擎（兼容 TVBox 源格式，纯 Rust，无 Tauri 依赖）
//!
//! 模块划分：
//! - `crypto` / `url_norm` / `config`：订阅链接归一化、四格式解密、线路合集、缓存
//! - `sources`：type0 XML / type1 CMS / type3 Spider / type4 远程接口
//! - `parse`：parses 解析链（type1 接口、type4 超解析并发）
//! - `spider`：type3 js（catvod/drpy 子集）；jar/py 返回明确不支持错误
//! - `live` / `epg`：直播 txt/m3u、多源、EPG
//! - `capture`：网络请求观测（供桌面端「抓包」面板使用）

// ---- 基础层 ----
pub mod crypto; // 配置解密（2423 / ;pk; / Base64 包装）
pub mod error; // 统一错误类型
pub mod http; // 带 rules 的客户端
pub mod model; // 所有数据结构（与前端/TVBox JSON 对齐）
pub mod normalize; // 脏 JSON 容错
pub mod pan; // 网盘引擎（W9：Quark 等分享协议客户端）
pub mod probe; // 直链可用性探测（HEAD + Range GET 兜底）
pub mod progress; // 解析/抓包进度事件（桌面端进度面板数据源）
pub mod url_norm; // 订阅地址归一化 / 相对路径修正 / URL 编码

// ---- 订阅与源适配 ----
pub mod config; // 订阅加载、线路合集、缓存
pub mod html_rule;
pub mod parse; // parses 解析链
pub mod search; // 跨源搜索 / 单源搜索
pub mod source; // 按 kind 分发（type0/1/3/4）
pub mod sources; // 各类型源的具体实现（cms/xml/spider/remote） // Jsoup 风格 pdfh/pdfa/pd/pdfla 规则引擎

// ---- 高级能力 ----
pub mod cache; // 本地缓存
pub mod capture;
pub mod douban; // 豆瓣首页数据（点播首页：集合 + 轮播详情，进程内缓存 10 分钟）
pub mod epg; // EPG 节目单
pub mod live; // 直播源解析
pub mod node; // Node/drpy sidecar
pub mod python; // Python spider（CPython sidecar）
pub mod spider; // type3 js 运行时 // 网络请求日志（供抓包面板）
pub mod subtitle_online; // 在线字幕搜索（SubHD）与下载直链解析

// 对外统一 API
pub use config::{inspect_config, load_config, load_config_shallow, load_config_with_options};
pub use search::{search_all, search_site};
pub use source::{get_detail, list_categories, list_videos, resolve_play_url};

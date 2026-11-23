//! catvod 契约层的公共类型。
//!
//! 这一层只描述「type3 `csp_*.jar` 站点在桌面端怎么被处理」，不做任何网络或
//! 解析动作：
//! - [`Strategy`]：实现载体（规则包 / 原生 Rust / 脚本 / 明确不支持）
//! - [`Difficulty`]：移植难度（对应计划 §7.1 的 L1–L4）
//! - [`ErrorClass`]：失败分类（计划 G3，所有未覆盖路径必须可读、可分类）
//! - [`JarSpec`]：站点 `jar` 字段的解析结果（`;md5;` 校验、`img+` 伪装）

use serde::Deserialize;

use crate::error::{CoreError, Result};
use crate::model::Site;

/// 实现载体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    /// 规则包：HTML/JSON 路径 + 简单加密，复用 `html_rule`
    Rulepack,
    /// 已画像、尚未移植
    Unported,
    /// js/py 脚本家族（走既有运行时，不属于 jar lane）
    Script,
    /// 明确不支持（受保护 jar / 需要 Android 能力）
    Unsupported,
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::Rulepack => "rulepack",
            Strategy::Unported => "unported",
            Strategy::Script => "script",
            Strategy::Unsupported => "unsupported",
        }
    }
}

/// 移植难度（计划 §7.1 L1–L4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Difficulty {
    L1,
    L2,
    L3,
    L4,
}

impl Difficulty {
    pub fn as_str(self) -> &'static str {
        match self {
            Difficulty::L1 => "L1",
            Difficulty::L2 => "L2",
            Difficulty::L3 => "L3",
            Difficulty::L4 => "L4",
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            Difficulty::L1 => 1,
            Difficulty::L2 => 2,
            Difficulty::L3 => 3,
            Difficulty::L4 => 4,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "L1" => Some(Difficulty::L1),
            "L2" => Some(Difficulty::L2),
            "L3" => Some(Difficulty::L3),
            "L4" => Some(Difficulty::L4),
            _ => None,
        }
    }
}

/// 失败分类。每个分类都有稳定的错误码，供前端/日志聚合使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// 配置里的类在任何已下载 jar 中都找不到
    MissingJar,
    /// 受保护 jar（`BaseSpiderGuard`/`ftyguard`/加密 dex）
    ProtectedGuard,
    /// 类已画像，但尚未移植
    NotPorted,
    /// 该站点其实是 js/py 脚本家族
    ScriptFamily,
    /// api 形态无法识别
    UnknownApi,
    /// jar 下载失败
    JarFetch,
    /// jar 完整性校验失败
    JarIntegrity,
    /// jar 布局无法识别（assets://、非 zip 等）
    JarLayout,
    /// spider 返回结果无法归一化
    Parse,
    /// 需要桌面端不提供的宿主能力（WebView/native/Android API）
    HostApi,
    /// JVM 宿主：jar 里没有配置的蜘蛛类
    JvmClassNotFound,
    /// JVM 宿主：蜘蛛 init 抛异常
    JvmInitFailed,
    /// JVM 宿主：蜘蛛未实现该入口
    JvmMethodMissing,
    /// JVM 宿主：调用参数/扩展无法解析
    JvmBadArgs,
    /// JVM 宿主：蜘蛛内部异常（站点改版/风控）
    JvmSpiderThrew,
    /// JVM 宿主：本地取流代理端口绑定失败
    JvmProxyBind,
    /// JVM 宿主：dex 转 jar 失败
    JvmDex2Jar,
    /// JVM 宿主：进程崩溃或超时退出
    JvmCrashed,
    /// 加固壳：需要 ARM native 解密载荷，当前没有可用载荷。
    JvmShellUnsupported,
    /// JVM 运行时未就位（缺 JRE/stubs.jar）
    JvmUnavailable,
    /// 网络层失败（未细分的兜底）
    Network,
    /// DNS 解析失败
    NetworkDns,
    /// 连接建立失败（拒绝/不可达/连接超时外的连接错误）
    NetworkConnect,
    /// 请求超时
    NetworkTimeout,
    /// TLS/证书失败
    NetworkTls,
    /// 网盘分享失效/不可访问（已删除、过期、分享码错误）
    PanShareUnavailable,
    /// 网盘操作需要 Cookie/登录态
    PanCookieRequired,
    /// 直播源加载/解析失败
    LiveFetch,
    /// 解析链（parses）未能产出可播放地址
    PlayParse,
    /// 辅助资源（EPG/字幕等）拉取或解析失败
    AuxFetch,
}

impl ErrorClass {
    pub fn code(self) -> &'static str {
        match self {
            ErrorClass::MissingJar => "catvod.jar.missing",
            ErrorClass::ProtectedGuard => "catvod.jar.protected",
            ErrorClass::NotPorted => "catvod.spider.not_ported",
            ErrorClass::ScriptFamily => "catvod.spider.script",
            ErrorClass::UnknownApi => "catvod.spider.unknown_api",
            ErrorClass::JarFetch => "catvod.jar.fetch",
            ErrorClass::JarIntegrity => "catvod.jar.integrity",
            ErrorClass::JarLayout => "catvod.jar.layout",
            ErrorClass::Parse => "catvod.result.parse",
            ErrorClass::HostApi => "catvod.host.unsupported",
            ErrorClass::JvmClassNotFound => "catvod.host.class_not_found",
            ErrorClass::JvmInitFailed => "catvod.host.init_failed",
            ErrorClass::JvmMethodMissing => "catvod.host.method_missing",
            ErrorClass::JvmBadArgs => "catvod.host.bad_args",
            ErrorClass::JvmSpiderThrew => "catvod.host.spider_threw",
            ErrorClass::JvmProxyBind => "catvod.host.proxy_bind",
            ErrorClass::JvmDex2Jar => "catvod.host.dex2jar",
            ErrorClass::JvmCrashed => "catvod.host.crashed",
            ErrorClass::JvmShellUnsupported => "catvod.host.shell_unsupported",
            ErrorClass::JvmUnavailable => "catvod.host.unavailable",
            ErrorClass::Network => "catvod.net.http",
            ErrorClass::NetworkDns => "catvod.net.dns",
            ErrorClass::NetworkConnect => "catvod.net.connect",
            ErrorClass::NetworkTimeout => "catvod.net.timeout",
            ErrorClass::NetworkTls => "catvod.net.tls",
            ErrorClass::PanShareUnavailable => "catvod.pan.share_unavailable",
            ErrorClass::PanCookieRequired => "catvod.pan.cookie_required",
            ErrorClass::LiveFetch => "catvod.live.fetch",
            ErrorClass::PlayParse => "catvod.play.parse",
            ErrorClass::AuxFetch => "catvod.aux.fetch",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            ErrorClass::MissingJar => "配置引用的 jar 不在已分析样本中；补下载后重新画像",
            ErrorClass::ProtectedGuard => "jar 使用 ftyguard 加固，dex 仅剩壳，静态分析无法还原",
            ErrorClass::NotPorted => "已有 spec/画像，等待按计划 P2 移植",
            ErrorClass::ScriptFamily => "该站点属于 js/py 家族，应走脚本运行时",
            ErrorClass::UnknownApi => "api 既不是 csp_* 也不是脚本，无法分派",
            ErrorClass::JarFetch => "jar 源站不可达或超时，可稍后重试或换镜像",
            ErrorClass::JarIntegrity => "jar 内容与配置 md5 不符（可能被替换或源站返回错误页）",
            ErrorClass::JarLayout => "jar 不是标准 dex-in-zip，或使用了桌面端不支持的 assets://",
            ErrorClass::Parse => "spider 返回结构与 catvod 契约不符",
            ErrorClass::HostApi => "依赖 WebView/native/Android 系统能力，桌面端不实现",
            ErrorClass::JvmClassNotFound => "宿主在 jar 里找不到配置的蜘蛛类：检查 api/class 拼写，或该 jar 版本已改名",
            ErrorClass::JvmInitFailed => "蜘蛛 init 抛异常：ext 形态不符或依赖能力缺失，看日志 stderr 尾部",
            ErrorClass::JvmMethodMissing => "该蜘蛛没有实现这个入口（例如只支持搜索或只支持详情）",
            ErrorClass::JvmBadArgs => "调用参数或 ext 无法被蜘蛛解析：检查站点配置",
            ErrorClass::JvmSpiderThrew => "蜘蛛内部异常，多为站点改版或风控；可稍后重试或换镜像",
            ErrorClass::JvmProxyBind => "宿主本地取流代理端口绑定失败：多为端口被占用，可重试",
            ErrorClass::JvmDex2Jar => "dex 转 jar 失败：转换器缺失或输出为空，可重试或换样本",
            ErrorClass::JvmCrashed => "JVM 宿主崩溃或超时退出：看日志里的 stderr 尾部与堆使用",
            ErrorClass::JvmShellUnsupported => {
                "该 jar 是加固壳，init 阶段要 ARM native 解密；当前没有可用载荷。可改用原生家族，或把解密产物放到 payloads/<jar sha>/ 并建 index.json"
            }
            ErrorClass::JvmUnavailable => "JVM 运行时未就位：先跑 npm run build:jvm 与 npm run fetch:jre",
            ErrorClass::Network => "网络层失败（DNS/连接/超时/TLS），可稍后重试、换镜像或检查代理",
            ErrorClass::NetworkDns => "域名解析失败：检查域名拼写、DNS 或被墙情况",
            ErrorClass::NetworkConnect => "连接失败：目标不可达或被拒绝，可换镜像/代理后重试",
            ErrorClass::NetworkTimeout => "请求超时：站点响应过慢，可稍后重试或换镜像",
            ErrorClass::NetworkTls => "TLS/证书失败：站点证书过期或不受信（桌面端默认严格校验）",
            ErrorClass::PanShareUnavailable => "网盘分享已失效：分享可能被删除/过期，或分享码不正确",
            ErrorClass::PanCookieRequired => "该网盘操作需要 Cookie/登录态：请在「云盘 → 右上角账号管理」登录，或在站点 ext 中配置 cookie",
            ErrorClass::LiveFetch => "直播源加载失败：检查订阅地址、格式与网络",
            ErrorClass::PlayParse => "解析链未产出可播放地址：可换解析器/换线路/检查解析器配置",
            ErrorClass::AuxFetch => "EPG/字幕等辅助资源拉取失败：检查地址与网络后重试",
        }
    }
}

impl ErrorClass {
    /// 根据 reqwest 错误特征分类网络失败。
    ///
    /// 说明：`reqwest` 未暴露独立的 DNS/TLS 分类，因此用
    /// `is_timeout()/is_connect()` + 错误文本关键词做启发式判断；
    /// 无法判定时退回 `Network`（`catvod.net.http`）。
    /// 把网盘引擎抛出的 `CoreError` 归到稳定子码；无法识别时返回 `None`。
    ///
    /// - Cookie/登录态问题 → `PanCookieRequired`
    /// - 分享不存在/过期/分享码错误（41006 等）→ `PanShareUnavailable`
    pub fn classify_pan_error(err: &crate::error::CoreError) -> Option<ErrorClass> {
        use crate::error::CoreError;
        let msg = err.to_string();
        let lower = msg.to_ascii_lowercase();
        if lower.contains("cookie")
            || lower.contains("guest user is disabled")
            || lower.contains("login")
            || msg.contains("登录")
        {
            return Some(ErrorClass::PanCookieRequired);
        }
        match err {
            CoreError::Api { code, .. }
                if matches!(*code, 41001 | 41002 | 41004 | 41006)
                    || msg.contains("分享不存在")
                    || msg.contains("分享已失效")
                    || lower.contains("share_link") =>
            {
                Some(ErrorClass::PanShareUnavailable)
            }
            _ => None,
        }
    }

    /// 用 `reqwest::Error` 及其 source 链文本做网络失败分类。
    pub fn classify_reqwest_error(err: &reqwest::Error) -> ErrorClass {
        use std::error::Error as _;
        let mut text = err.to_string();
        let mut source = err.source();
        while let Some(cause) = source {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        Self::classify_network(err.is_timeout(), err.is_connect(), &text)
    }

    pub fn classify_network(is_timeout: bool, is_connect: bool, text: &str) -> ErrorClass {
        let lower = text.to_ascii_lowercase();
        if is_timeout || lower.contains("timed out") {
            return ErrorClass::NetworkTimeout;
        }
        if lower.contains("dns error")
            || lower.contains("failed to lookup address")
            || lower.contains("name or service not known")
            || lower.contains("nodename nor servname")
        {
            return ErrorClass::NetworkDns;
        }
        if lower.contains("certificate")
            || lower.contains("tls")
            || lower.contains("handshake")
            || lower.contains("invalid peer")
        {
            return ErrorClass::NetworkTls;
        }
        if is_connect {
            ErrorClass::NetworkConnect
        } else {
            ErrorClass::Network
        }
    }
}

/// 面向用户展示的错误文案（标题 + 建议动作 + 是否可重试）。
///
/// 宿主 UI 不应直接暴露 `catvod.*` 码；用 [`describe_error`] 把家族/源层
/// 抛出的消息翻译成「标题 + 细节 + 建议动作」，码仍保留在 `code` 里供日志聚合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorText {
    pub code: &'static str,
    pub title: &'static str,
    pub detail: String,
    pub action: &'static str,
    pub retryable: bool,
}

impl ErrorClass {
    /// 全部稳定分类（顺序与 UI 文案表一致）。
    pub const ALL: [ErrorClass; 30] = [
        ErrorClass::MissingJar,
        ErrorClass::ProtectedGuard,
        ErrorClass::NotPorted,
        ErrorClass::ScriptFamily,
        ErrorClass::UnknownApi,
        ErrorClass::JarFetch,
        ErrorClass::JarIntegrity,
        ErrorClass::JarLayout,
        ErrorClass::Parse,
        ErrorClass::HostApi,
        ErrorClass::JvmClassNotFound,
        ErrorClass::JvmInitFailed,
        ErrorClass::JvmMethodMissing,
        ErrorClass::JvmBadArgs,
        ErrorClass::JvmSpiderThrew,
        ErrorClass::JvmProxyBind,
        ErrorClass::JvmDex2Jar,
        ErrorClass::JvmCrashed,
        ErrorClass::JvmShellUnsupported,
        ErrorClass::JvmUnavailable,
        ErrorClass::Network,
        ErrorClass::NetworkDns,
        ErrorClass::NetworkConnect,
        ErrorClass::NetworkTimeout,
        ErrorClass::NetworkTls,
        ErrorClass::PanShareUnavailable,
        ErrorClass::PanCookieRequired,
        ErrorClass::LiveFetch,
        ErrorClass::PlayParse,
        ErrorClass::AuxFetch,
    ];

    /// 面向用户的一句话标题（不含技术码）。
    pub fn title(self) -> &'static str {
        match self {
            ErrorClass::MissingJar => "缺少站点组件",
            ErrorClass::ProtectedGuard => "站点组件受保护",
            ErrorClass::NotPorted => "该站点暂未支持",
            ErrorClass::ScriptFamily => "脚本类站点",
            ErrorClass::UnknownApi => "无法识别的站点类型",
            ErrorClass::JarFetch => "组件下载失败",
            ErrorClass::JarIntegrity => "组件校验失败",
            ErrorClass::JarLayout => "组件格式不支持",
            ErrorClass::Parse => "站点数据解析失败",
            ErrorClass::HostApi => "需要桌面端不支持的能力",
            ErrorClass::JvmClassNotFound => "站点类未找到",
            ErrorClass::JvmInitFailed => "站点初始化失败",
            ErrorClass::JvmMethodMissing => "该站点不支持此操作",
            ErrorClass::JvmBadArgs => "站点参数不匹配",
            ErrorClass::JvmSpiderThrew => "站点脚本出错",
            ErrorClass::JvmProxyBind => "本地代理端口被占用",
            ErrorClass::JvmDex2Jar => "组件转换失败",
            ErrorClass::JvmCrashed => "JVM 宿主崩溃",
            ErrorClass::JvmShellUnsupported => "加固壳暂不支持",
            ErrorClass::JvmUnavailable => "JVM 运行时未就绪",
            ErrorClass::Network => "网络请求失败",
            ErrorClass::NetworkDns => "域名解析失败",
            ErrorClass::NetworkConnect => "无法连接站点",
            ErrorClass::NetworkTimeout => "请求超时",
            ErrorClass::NetworkTls => "HTTPS 证书错误",
            ErrorClass::PanShareUnavailable => "网盘分享已失效",
            ErrorClass::PanCookieRequired => "网盘需要登录 Cookie",
            ErrorClass::LiveFetch => "直播源加载失败",
            ErrorClass::PlayParse => "解析播放地址失败",
            ErrorClass::AuxFetch => "EPG/字幕加载失败",        }
    }

    /// 面向用户的建议动作。
    pub fn action(self) -> &'static str {
        match self {
            ErrorClass::MissingJar | ErrorClass::JarFetch => "稍后重试，或在设置里更新订阅/镜像",
            ErrorClass::ProtectedGuard => "该站点无法在桌面端还原，建议在配置中禁用",
            ErrorClass::NotPorted => "关注后续版本更新；可先切换到其他站点",
            ErrorClass::ScriptFamily => "检查脚本运行时是否可用，或换用其他站点",
            ErrorClass::UnknownApi => "检查订阅里的 api 写法是否正确",
            ErrorClass::JarIntegrity => "删除本地缓存后重新加载订阅",
            ErrorClass::JarLayout => "该组件依赖 Android 布局，建议禁用该站点",
            ErrorClass::Parse => "站点可能已改版，稍后重试或换线路/换站点",
            ErrorClass::HostApi => "换用其他站点；该能力需要 Android/WebView 宿主",
            ErrorClass::Network => "检查网络/代理后重试，或换镜像",
            ErrorClass::NetworkDns => "检查 DNS 或代理设置后重试",
            ErrorClass::NetworkConnect => "换镜像/代理后重试",
            ErrorClass::NetworkTimeout => "稍后重试或换镜像",
            ErrorClass::NetworkTls => "检查系统时间/证书，或换镜像",
            ErrorClass::PanShareUnavailable => "换一个分享链接/线路，或联系分享者更新",
            ErrorClass::PanCookieRequired => "在「云盘 → 右上角账号管理」登录，或在站点 ext 中配置对应网盘的 Cookie 后重试",
            ErrorClass::LiveFetch => "检查订阅地址/格式与网络后重试",
            ErrorClass::PlayParse => "换解析器/换线路，或检查解析器配置",
            ErrorClass::AuxFetch => "检查地址与网络后重试",
            ErrorClass::JvmClassNotFound => "换用其他站点，或在设置里更新订阅（该 jar 可能已改类名）",
            ErrorClass::JvmInitFailed => "检查站点 ext 配置；仍失败则换用其他站点",
            ErrorClass::JvmMethodMissing => "该站点不支持此操作，换用其他站点",
            ErrorClass::JvmBadArgs => "检查站点配置后重试，或换用其他站点",
            ErrorClass::JvmSpiderThrew => "稍后重试；持续失败则换用其他站点",
            ErrorClass::JvmProxyBind => "稍后重试；持续失败则重启应用",
            ErrorClass::JvmDex2Jar => "重试一次；仍失败则在设置里重建组件缓存",
            ErrorClass::JvmCrashed => "重试；仍失败则查看日志并反馈",
            ErrorClass::JvmShellUnsupported => "换用非加固 jar，或在设置里切换到原生家族",
            ErrorClass::JvmUnavailable => "运行 npm run build:jvm 与 npm run fetch:jre 后重启应用",
        }
    }

    /// 该错误是否值得「原样重试」。
    pub fn retryable(self) -> bool {
        matches!(
            self,
            ErrorClass::JarFetch
                | ErrorClass::MissingJar
                | ErrorClass::Network
                | ErrorClass::NetworkDns
                | ErrorClass::NetworkConnect
                | ErrorClass::NetworkTimeout
                | ErrorClass::NetworkTls
                | ErrorClass::LiveFetch
                | ErrorClass::Parse
                | ErrorClass::PlayParse
                | ErrorClass::AuxFetch
                | ErrorClass::JvmSpiderThrew
                | ErrorClass::JvmProxyBind
                | ErrorClass::JvmDex2Jar
                | ErrorClass::JvmCrashed
        )
    }

    /// 从消息里识别稳定码（长码优先，避免前缀误判）。
    pub fn detect(message: &str) -> Option<ErrorClass> {
        let mut best: Option<ErrorClass> = None;
        for class in ErrorClass::ALL {
            if message.contains(class.code()) {
                match best {
                    Some(prev) if prev.code().len() >= class.code().len() => {}
                    _ => best = Some(class),
                }
            }
        }
        best
    }
}

/// 把带 `catvod.*` 码的消息翻译成面向用户的文案；无码返回 `None`。
///
/// 输入形如 `"catvod.result.parse：详情页缺少标题"`；`detail` 会剥掉码与前导
/// 分隔符，保留技术细节供「展开详情」使用。
pub fn describe_error(message: &str) -> Option<ErrorText> {
    let class = ErrorClass::detect(message)?;
    let code = class.code();
    let rest = message.split_once(code).map(|(_, after)| after).unwrap_or("");
    let detail = rest
        .trim_start_matches([':', '：', ' ', '\t', '-', '—'])
        .trim()
        .to_string();
    Some(ErrorText {
        code,
        title: class.title(),
        detail,
        action: class.action(),
        retryable: class.retryable(),
    })
}

/// catvod 侧的分类错误（最终转换为 `CoreError::Config`）。
#[derive(Debug, Clone)]
pub struct CatvodError {
    pub class: ErrorClass,
    pub site_key: String,
    pub site_name: String,
    pub api: String,
    /// 触发错误的 jar 字段原文（无 jar 上下文时为 `-`）
    pub jar: String,
    pub detail: String,
}

impl CatvodError {
    pub fn new(class: ErrorClass, site: &Site, detail: impl Into<String>) -> Self {
        Self {
            class,
            site_key: site.key.clone(),
            site_name: if site.name.is_empty() {
                site.key.clone()
            } else {
                site.name.clone()
            },
            api: site.api.clone(),
            jar: "-".into(),
            detail: detail.into(),
        }
    }


    /// 附带触发错误的 jar 字段原文（loader 层使用）。
    pub fn with_jar(mut self, jar: impl Into<String>) -> Self {
        self.jar = jar.into();
        self
    }

    pub fn message(&self) -> String {
        format!(
            "站点「{}」({})不可用：{}（api={}，jar={}）。{}",
            self.site_name,
            self.site_key,
            self.class.code(),
            if self.api.is_empty() { "-" } else { &self.api },
            &self.jar,
            if self.detail.is_empty() {
                self.class.hint().to_string()
            } else {
                format!("{}；{}", self.detail, self.class.hint())
            }
        )
    }

    pub fn into_core(self) -> CoreError {
        CoreError::Config(self.message())
    }
}

/// 站点 `jar` 字段的解析结果。
///
/// catvod 的写法：
/// - `http(s)://...jar`、`file://`、`assets://`、`clan://`
/// - `...jar;md5;<32 位 hex>`：下载后必须校验 md5
/// - `img+...`：伪装成图片的 jar（前若干字节是图片头，需裁掉）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JarSpec {
    /// 原始字段（排查问题时原样打印）
    pub raw: String,
    /// 去掉 `;md5;…` 与 `img+` 后的地址
    pub url: String,
    /// 期望的 md5（小写）
    pub md5: Option<String>,
    /// 是否带 `img+` 前缀
    pub disguised: bool,
}

impl JarSpec {
    /// 解析站点 `jar` 字段；空字段返回 `Ok(None)`。
    pub fn parse(raw: &str) -> Result<Option<JarSpec>> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        let mut rest = raw.to_string();
        let mut md5 = None;
        if let Some(idx) = rest.rfind(";md5;") {
            let candidate = rest[idx + 5..].trim().to_ascii_lowercase();
            if candidate.len() == 32 && candidate.chars().all(|c| c.is_ascii_hexdigit()) {
                md5 = Some(candidate);
                rest.truncate(idx);
            } else {
                // 旧逻辑会带着「;md5;非法值」去请求，用户看到的是 URL 解析/404，
                // 而不是「配置里的 md5 段写错了」。这里显式拒绝并保留原始字段。
                return Err(CoreError::Config(format!(
                    "站点 jar 字段 md5 非法（应为 32 位十六进制）: {raw}"
                )));
            }
        }
        let mut disguised = false;
        if let Some(stripped) = rest.strip_prefix("img+") {
            disguised = true;
            rest = stripped.trim().to_string();
        }
        if rest.is_empty() {
            return Err(CoreError::Config(format!("站点 jar 字段无有效地址: {raw}")));
        }
        Ok(Some(JarSpec {
            raw: raw.to_string(),
            url: rest,
            md5,
            disguised,
        }))
    }

    /// `file://` 的本地路径（仅当确实是 file 协议时返回）。
    pub fn local_path(&self) -> Option<String> {
        let url = &self.url;
        if let Some(rest) = url.strip_prefix("file://") {
            let decoded = percent_encoding::percent_decode_str(rest)
                .decode_utf8_lossy()
                .to_string();
            // file:///C:/a.jar / file://C:/a.jar 都可能出现
            let trimmed = decoded.trim_start_matches('/');
            if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
                return Some(trimmed.to_string());
            }
            return Some(decoded);
        }
        None
    }

    /// 缓存文件名（无 md5 时用 URL 的 md5 兜底）。
    pub fn cache_key(&self) -> String {
        if let Some(md5) = &self.md5 {
            return format!("{md5}.jar");
        }
        let digest = md5::compute(self.url.as_bytes());
        format!("{digest:x}.jar")
    }

    /// 是否 http(s) 远程地址。
    pub fn is_remote(&self) -> bool {
        self.url.starts_with("http://") || self.url.starts_with("https://")
    }

    /// `assets://` 在桌面端无对应资源包。
    pub fn is_assets(&self) -> bool {
        self.url.starts_with("assets://")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jar_spec_strips_md5_and_img_prefix() {
        let spec = JarSpec::parse("img+http://a.example/x.jar;md5;ABCDEF0123456789ABCDEF0123456789")
            .unwrap()
            .unwrap();
        assert_eq!(spec.url, "http://a.example/x.jar");
        assert!(spec.disguised);
        assert_eq!(
            spec.md5.as_deref(),
            Some("abcdef0123456789abcdef0123456789")
        );
        assert!(spec.is_remote());
    }

    #[test]
    fn jar_spec_rejects_bad_md5() {
        // 旧行为把「;md5;notahash」留在 URL 里，最终表现为下游 404；
        // 新契约要求在解析阶段直接报配置错误（集成测试同时覆盖错误文案）。
        let err = JarSpec::parse("http://a/x.jar;md5;notahash")
            .expect_err("非法 md5 段必须报错");
        assert!(err.to_string().contains("md5 非法"), "{err}");
    }

    #[test]
    fn jar_spec_file_url() {
        let spec = JarSpec::parse("file:///D:/tmp/a.jar").unwrap().unwrap();
        assert_eq!(spec.local_path().as_deref(), Some("D:/tmp/a.jar"));
    }

    #[test]
    fn error_class_codes_are_stable() {
        assert_eq!(ErrorClass::ProtectedGuard.code(), "catvod.jar.protected");
        assert_eq!(ErrorClass::NotPorted.code(), "catvod.spider.not_ported");
        assert_eq!(ErrorClass::MissingJar.code(), "catvod.jar.missing");
    }

    #[test]
    fn network_subcodes_are_stable() {
        assert_eq!(ErrorClass::Network.code(), "catvod.net.http");
        assert_eq!(ErrorClass::NetworkDns.code(), "catvod.net.dns");
        assert_eq!(ErrorClass::NetworkConnect.code(), "catvod.net.connect");
        assert_eq!(ErrorClass::NetworkTimeout.code(), "catvod.net.timeout");
        assert_eq!(ErrorClass::NetworkTls.code(), "catvod.net.tls");
        for c in [
            ErrorClass::Network,
            ErrorClass::NetworkDns,
            ErrorClass::NetworkConnect,
            ErrorClass::NetworkTimeout,
            ErrorClass::NetworkTls,
        ] {
            assert!(c.code().starts_with("catvod.net."));
            assert!(!c.hint().is_empty());
        }
    }

    #[test]
    fn classify_pan_error_splits_cookie_and_share() {
        use crate::error::CoreError;
        let cookie = CoreError::Config("quark 网盘需要 Cookie：请在站点 ext 中配置".into());
        assert_eq!(
            ErrorClass::classify_pan_error(&cookie),
            Some(ErrorClass::PanCookieRequired)
        );
        let gone = CoreError::Api { code: 41006, msg: "分享不存在".into() };
        assert_eq!(
            ErrorClass::classify_pan_error(&gone),
            Some(ErrorClass::PanShareUnavailable)
        );
        let other = CoreError::Api { code: 500, msg: "internal".into() };
        assert_eq!(ErrorClass::classify_pan_error(&other), None);
    }

    #[test]
    fn classify_network_uses_error_features() {
        // 超时优先
        assert_eq!(
            ErrorClass::classify_network(true, true, "connection timed out"),
            ErrorClass::NetworkTimeout
        );
        // DNS
        assert_eq!(
            ErrorClass::classify_network(false, true, "dns error: failed to lookup address"),
            ErrorClass::NetworkDns
        );
        // TLS
        assert_eq!(
            ErrorClass::classify_network(false, false, "invalid peer certificate: Expired"),
            ErrorClass::NetworkTls
        );
        // 连接被拒
        assert_eq!(
            ErrorClass::classify_network(false, true, "tcp connect error: Connection refused"),
            ErrorClass::NetworkConnect
        );
        // 无法判定时兜底
        assert_eq!(
            ErrorClass::classify_network(false, false, "some other failure"),
            ErrorClass::Network
        );
    }
}

#[cfg(test)]
mod describe_tests {
    use super::*;

    #[test]
    fn all_codes_are_unique_and_have_copy() {
        let mut seen = std::collections::HashSet::new();
        for c in ErrorClass::ALL {
            assert!(c.code().starts_with("catvod."), "{:?}", c);
            assert!(seen.insert(c.code()), "duplicate {}", c.code());
            assert!(!c.title().trim().is_empty(), "{:?}", c);
            assert!(!c.action().trim().is_empty(), "{:?}", c);
            assert!(!c.hint().trim().is_empty(), "{:?}", c);
        }
        assert_eq!(seen.len(), 30, "ErrorClass::ALL 的条目数应与 errorClass 全量表一致");
    }

    #[test]
    fn detect_prefers_the_full_code() {
        assert_eq!(
            ErrorClass::detect("catvod.result.parse：详情页缺少标题"),
            Some(ErrorClass::Parse)
        );
        assert_eq!(
            ErrorClass::detect("x catvod.net.timeout y"),
            Some(ErrorClass::NetworkTimeout)
        );
        assert_eq!(ErrorClass::detect("普通错误"), None);
    }

    #[test]
    fn describe_strips_code_and_prefix() {
        let text = describe_error("catvod.pan.cookie_required：该网盘操作需要 Cookie").unwrap();
        assert_eq!(text.code, "catvod.pan.cookie_required");
        assert_eq!(text.title, ErrorClass::PanCookieRequired.title());
        assert_eq!(text.detail, "该网盘操作需要 Cookie");
        assert!(text.retryable == false);

        let plain = describe_error("catvod.net.dns: dns error: failed to lookup");
        assert_eq!(plain.map(|t| t.code), Some("catvod.net.dns"));
    }

    #[test]
    fn retryable_flag_matches_class() {
        assert!(ErrorClass::NetworkTimeout.retryable());
        assert!(ErrorClass::Parse.retryable());
        assert!(!ErrorClass::ProtectedGuard.retryable());
        assert!(!ErrorClass::PanShareUnavailable.retryable());
    }
}

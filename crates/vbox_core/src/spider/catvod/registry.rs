//! type3 jar 站点的运行期画像表（registry）。
//!
//! 画像来源：P0-2 样本分析（6 份真实多仓配置、238 个 type3 站点、9 个已下载 jar，
//! 逐个用 DEX `class_defs` 精确统计）。`profiles.json` 随 crate 编译进二进制，
//! 运行期零网络、零文件依赖。
//!
//! 用途：
//! 1. 把「这种 api 是否可原生服务」变成可查询的事实，而不是散落的 if-else；
//! 2. 未覆盖站点返回**分类错误**（计划 G3），前端可以据此提示；
//! 3. 新家族移植后只改 JSON/注册表，不动分派逻辑（计划 §7.3）。

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::error::CoreError;
use crate::model::Site;

use super::contract::{CatvodError, Difficulty, ErrorClass, Strategy};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesFile {
    pub schema: u32,
    pub generated_at: String,
    pub source: String,
    pub totals: Totals,
    #[serde(default)]
    pub jars: Vec<JarInfo>,
    #[serde(default)]
    pub apis: HashMap<String, ApiProfile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    #[serde(default)]
    pub sites: u32,
    #[serde(default)]
    pub distinct_apis: u32,
    #[serde(default)]
    pub jars: u32,
    #[serde(default)]
    pub strategies: HashMap<String, u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JarInfo {
    pub name: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub classes: u32,
    #[serde(default)]
    pub spiders: u32,
    #[serde(default)]
    pub guarded: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiProfile {
    #[serde(default)]
    pub sites: u32,
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub jars: Vec<String>,
    #[serde(default)]
    pub methods: u32,
    #[serde(default)]
    pub guarded: bool,
    #[serde(default)]
    pub strategy: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub difficulty: Option<String>,
    #[serde(default)]
    pub configs: Vec<String>,
    /// 已知阻塞原因（仍为 unported，但可解释「为什么现在做不了」）：
    /// `decompiler_gap`（反编译缺口）、`login_required`（需登录态）、`engine_dependency`（需宿主引擎）。
    #[serde(default)]
    pub blocked_reason: Option<String>,
}

impl ApiProfile {
    pub fn strategy(&self) -> Strategy {
        match self.strategy.as_str() {
            "rulepack" => Strategy::Rulepack,
            "unported" => Strategy::Unported,
            "script" => Strategy::Script,
            _ => Strategy::Unsupported,
        }
    }

    pub fn difficulty(&self) -> Option<Difficulty> {
        self.difficulty.as_deref().and_then(Difficulty::parse)
    }

    /// 简短画像（日志/调试用）：`csp_Bili sites=13 jars=Yoursmile.jar methods=26 L2`
    pub fn summary(&self, api: &str) -> String {
        let mut parts = vec![format!("{api} sites={}", self.sites)];
        if !self.jars.is_empty() {
            parts.push(format!("jars={}", self.jars.join("|")));
        }
        if self.methods > 0 {
            parts.push(format!("methods={}", self.methods));
        }
        if let Some(d) = self.difficulty() {
            parts.push(d.as_str().to_string());
        }
        if let Some(reason) = self.blocked_reason.as_deref() {
            parts.push(format!("blocked={reason}"));
        }
        parts.join(" ")
    }

    /// 已知阻塞原因（无则 `None`）。
    pub fn blocked_reason(&self) -> Option<&str> {
        self.blocked_reason.as_deref()
    }
}

// ---------------------------------------------------------------------------
// 内嵌画像
// ---------------------------------------------------------------------------

const PROFILES_JSON: &str = include_str!("profiles.json");

static PROFILES: OnceLock<ProfilesFile> = OnceLock::new();

/// 全局画像表（首次访问时解析内嵌 JSON）。
pub fn profiles() -> &'static ProfilesFile {
    PROFILES.get_or_init(|| match serde_json::from_str::<ProfilesFile>(PROFILES_JSON) {
        Ok(p) => p,
        Err(e) => {
            // 内嵌常量损坏属于构建问题，给出空表并让分派走「未知 api」路径，
            // 避免整个进程 panic（type3 jar 本来就不是主链路）。
            tracing::error!("catvod profiles.json 解析失败: {e}");
            ProfilesFile {
                schema: 0,
                generated_at: String::new(),
                source: String::new(),
                totals: Totals {
                    sites: 0,
                    distinct_apis: 0,
                    jars: 0,
                    strategies: HashMap::new(),
                },
                jars: Vec::new(),
                apis: HashMap::new(),
            }
        }
    })
}

/// 按 api 查画像。
pub fn lookup(api: &str) -> Option<&'static ApiProfile> {
    profiles().apis.get(api.trim())
}

/// 该 api 是否已有原生实现（当前仅规则引擎覆盖的两族）。
pub fn is_native_ready(api: &str) -> bool {
    matches!(
        api.trim(),
        "csp_XBPQ"
            | "csp_XBPQ_upgraded"
            | "csp_XYQHiker"
            | "csp_XYQHikerAL"
            | "csp_XYQHikerV2"
            | "csp_PanSearch"
            | "csp_SixV"
            | "csp_Ysj" | "csp_Anime1" | "csp_Bili" | "csp_Dm84" | "csp_FirstAid" | "csp_NanGua" | "csp_Libvio" | "csp_Funletu" | "csp_QuarkPanso" | "csp_Jianpian"
            | "csp_Jpys" | "csp_YQKan" | "csp_AppTT" | "csp_Ying"
            | "csp_Kanqiu" | "csp_Mp4Mov" | "csp_SP360" | "csp_SP33" | "csp_Kugou"
            | "csp_Kunyu77" | "csp_AppGet"
            | "csp_Duanju"
            | "csp_Xb6v"
            | "csp_PanSou"
            | "csp_UpYun"
            | "csp_Dovx"
            | "csp_YiSo"
            | "csp_Upyunso"
            | "csp_Yisou"
            | "csp_AliPS"
            | "csp_HunHePan" | "csp_DaPanSo" | "csp_Zxzj"
            | "csp_MiaoSou" | "csp_EightEight" | "csp_YunPanOne" | "csp_PikaSo"
        | "csp_Qianfan"
        | "csp_YunSo"
        | "csp_Douban"
        | "csp_DouDou"
        | "csp_Meijumi"
        | "csp_Kkys"
        | "csp_Ddrk"
        | "csp_Ddys"
        | "csp_Auete"
        | "csp_Iktv"
        | "csp_Yinyuetai"
        | "csp_Alist"
        | "csp_Tangrenjie"
        | "csp_Quark"
        | "csp_Moli"
        | "csp_TZFile"
        | "csp_AppYs"
        | "csp_AppYsV2"
        | "csp_QuarkShare"
        | "csp_UCShare"
        | "csp_XiaoZhang"
        | "csp_TTian" | "csp_Huban"
        | "csp_Wogg"
        | "csp_Wobg"
        | "csp_Wo4k"
        | "csp_WoGG"
        | "csp_Xpanpan" | "csp_YingSo"
        | "csp_AppMao"
        | "csp_AppLd"
        | "csp_AppSK"
        | "csp_Bttwoo"
        | "csp_YunPan"
        | "csp_FourKHDR"
        | "csp_Push"
        | "csp_Xinsj"
        | "csp_SeedHub"
        | "csp_Yj1211" | "csp_LiteApple"
        | "csp_XPath"
        | "csp_Nmys"
        | "csp_NiNi"
        | "csp_Bdys01"
        | "csp_Czsapp"
        | "csp_Gitcafe"
        | "csp_Kuaikan"
        | "csp_PushShare"
        | "csp_Panyq"
            | "csp_P115Share"
            | "csp_PikPakShare"
            | "csp_ThunderShare"
            | "csp_SambaShare"
            | "csp_Youtube"
            | "csp_TGYunPan"
            | "csp_TGYunPanLocal"
            | "csp_AliShare"
            | "csp_Bt0"
        | "csp_Gaoqing"
        | "csp_Zhaozy"
        | "csp_Hdh"
        | "csp_XBiu"
        | "csp_XiaoZhiTiao"
    )
}

// ---------------------------------------------------------------------------
// 站点分派
// ---------------------------------------------------------------------------

/// 一个 type3 站点的处置结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// 已有原生规则引擎覆盖（XBPQ / XYQHiker）
    Rulepack { api: String },
    /// 已画像，尚未移植
    Unported {
        api: String,
        class: String,
        jars: Vec<String>,
        difficulty: Option<Difficulty>,
    },
    /// js/py 脚本家族
    Script { api: String },
    /// 明确不支持
    Unsupported { api: String, reason: String },
    /// 画像表里没有
    Unknown { api: String },
}

impl Classification {
    pub fn strategy(&self) -> Strategy {
        match self {
            Classification::Rulepack { .. } => Strategy::Rulepack,
            Classification::Unported { .. } => Strategy::Unported,
            Classification::Script { .. } => Strategy::Script,
            Classification::Unsupported { .. } | Classification::Unknown { .. } => {
                Strategy::Unsupported
            }
        }
    }

    /// 转成带分类码的用户可读错误。
    pub fn error(&self, site: &Site) -> CoreError {
        let api = site.api.trim();
        let err = match self {
            Classification::Rulepack { .. } | Classification::Script { .. } => {
                CatvodError::new(ErrorClass::HostApi, site, "内部分派错误：应由原生/脚本路径处理")
            }
            Classification::Unported {
                class, jars, difficulty, ..
            } => {
                let d = difficulty
                    .map(|d| d.as_str().to_string())
                    .unwrap_or_else(|| "?".into());
                // 画像里的 blockedReason 是审计的证据链产物；此前的运行时错误只报“难度 ?”，
                // 用户与排障者都看不到“为什么不能播”。这里把它与建议一起带进错误消息。
                let reason = lookup(api)
                    .and_then(|p| p.blocked_reason.clone())
                    .unwrap_or_else(|| "unknown".to_string());
                let hint = blocked_reason_hint(&reason);
                CatvodError::new(
                    ErrorClass::NotPorted,
                    site,
                    format!(
                        "类 {class} 已画像（难度 {d}）；阻断原因 {reason}：{hint}；定义于 {}",
                        if jars.is_empty() {
                            "未知 jar".to_string()
                        } else {
                            jars.join(", ")
                        }
                    ),
                )
            }
            Classification::Unsupported { reason, .. } => {
                let class = match reason.as_str() {
                    "protected_guard" => ErrorClass::ProtectedGuard,
                    "unknown_api" => ErrorClass::UnknownApi,
                    "class_not_in_downloaded_jars" => ErrorClass::MissingJar,
                    _ => ErrorClass::HostApi,
                };
                CatvodError::new(
                    class,
                    site,
                    format!(
                        "画像结论: {reason}（api={api}）；{}",
                        blocked_reason_hint(reason)
                    ),
                )
            }
            Classification::Unknown { .. } => CatvodError::new(
                ErrorClass::UnknownApi,
                site,
                format!("画像表无 api={api}"),
            ),
        };
        err.into_core()
    }
}

/// 判断 api 是否脚本家族（与 `sources/spider.rs` 判定保持一致）。
pub fn api_is_script(api: &str) -> bool {
    let api = api.trim();
    api.ends_with(".js")
        || api.contains(".js?")
        || api.ends_with(".py")
        || api.starts_with("py_")
        || api.starts_with("js_")
        || api.contains("drpy")
        || api.starts_with("http://")
        || api.starts_with("https://")
}

/// 对站点给出处置结论。
pub fn classify(site: &Site) -> Classification {
    let api = site.api.trim().to_string();
    if api.is_empty() {
        if site.jar.trim().is_empty() {
            return Classification::Unknown { api };
        }
        // 有 jar 但无 api：catvod 用 api=csp_X 定位类，缺 api 无法分派
        return Classification::Unsupported {
            api,
            reason: "missing_api".to_string(),
        };
    }
    if is_native_ready(&api) {
        return Classification::Rulepack { api };
    }
    if api_is_script(&api) {
        return Classification::Script { api };
    }
    match lookup(&api) {
        Some(p) => match p.strategy() {
            Strategy::Rulepack => Classification::Rulepack { api },
            Strategy::Script => Classification::Script { api },
            Strategy::Unported => Classification::Unported {
                api,
                class: p.class.clone().unwrap_or_default(),
                jars: p.jars.clone(),
                difficulty: p.difficulty(),
            },
            Strategy::Unsupported => Classification::Unsupported {
                api,
                reason: p
                    .reason
                    .clone()
                    .unwrap_or_else(|| "unsupported_by_profile".to_string()),
            },
        },
        None => Classification::Unknown { api },
    }
}

/// 便捷入口：直接给出分类错误（供 `sources::spider` 兜底分支使用）。
/// 某 api 的已知阻塞原因（从内嵌画像读取）。
pub fn blocked_reason(api: &str) -> Option<String> {
    lookup(api).and_then(|profile| profile.blocked_reason.clone())
}

/// 把画像里的阻断原因转成一句可执行的用户提示。
///
/// 这里的字符串同时被运行时错误与判据 68 消费：新增原因必须同时改这里，
/// 否则 `catvod_blocked_message` 会红（防止原因只活在台账里）。
pub fn blocked_reason_hint(reason: &str) -> &'static str {
    match reason {
        "site_dead" => "上游站点已失效（DNS/停放页），待站点复活后复评",
        "engine_dependency" => "依赖 TVBox 宿主 parser/merge 引擎（阿里/夸克/迅雷/115 等），桌面端尚未提供",
        "webview_dependency" => "依赖 Android WebView 拦截请求，桌面端无法等价执行",
        "decompiler_gap" => "反编译缺口：关键方法被 jadx 跳过，协议无法完整复原",
        "login_required" => "需要上游登录态/Cookie，匿名请求不可用",
        "crypto_dependency" => "依赖尚未复现的加密/许可证逻辑",
        "player_resolver_aggregator" => "依赖宿主 side 的解析器聚合（parser），桌面端未内置",
        "protected_guard" => "jar 受 ftyguard 类保护，业务类不可直接调用",
        "unknown_api" => "配置里的 api 未出现在任何已分析 jar/画像中",
        "class_not_in_downloaded_jars" => "类不在已下载的 20 个 jar 中，需补 jar 后复评",
        "missing_api" => "站点配置了 jar 但缺少 api，无法定位 spider 类",
        "android_app_market_semantics" => "应用市场语义：站点只提供 App 下载/跳转，不是视频源",
        "config_center_semantics" => "配置中心语义：站点用于聚合配置，不是可播放的视频源",
        "ali_signin_utility" => "阿里云盘扫码登录工具类，不是 Spider 视频源",
        _ => "尚未分类的阻断原因，见 docs/审计报告/阻断复核.md",
    }
}
pub fn site_error(site: &Site) -> CoreError {
    classify(site).error(site)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(key: &str, api: &str, jar: &str) -> Site {
        Site {
            key: key.into(),
            name: key.into(),
            kind: 3,
            api: api.into(),
            searchable: true,
            quick_search: true,
            changeable: true,
            filterable: true,
            player_url: String::new(),
            ext: None,
            jar: jar.into(),
            player_type: 0,
            timeout: 0,
            click: String::new(),
            style: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn profiles_are_embedded_and_consistent() {
        let p = profiles();
        assert_eq!(p.schema, 1);
        assert!(p.totals.sites >= 200, "样本站点数应 >= 200");
        assert!(p.jars.len() >= 9);
        assert!(p.apis.len() >= 100);
        assert!(!p.generated_at.is_empty());
        // 规则引擎两族必须在表里
        assert!(p.apis.contains_key("csp_XBPQ"));
        assert!(p.apis.contains_key("csp_XYQHiker"));
    }

    #[test]
    fn classifies_known_families() {
        assert_eq!(
            classify(&site("a", "csp_XBPQ", "")),
            Classification::Rulepack {
                api: "csp_XBPQ".into()
            }
        );
        assert_eq!(
            classify(&site("b", "https://x/y.js", "")),
            Classification::Script {
                api: "https://x/y.js".into()
            }
        );
        assert_eq!(
            classify(&site("c", "py_demo", "")),
            Classification::Script {
                api: "py_demo".into()
            }
        );
    }

    #[test]
    fn classifies_unported_with_jar_evidence() {
        // csp_Bili/csp_P115Share 已原生落地，改用仍未移植但能定位 jar 的 api 验证画像字段。
        match classify(&site("pushagent", "csp_PushAgent", "")) {
            Classification::Unported {
                class,
                jars,
                difficulty,
                ..
            } => {
                assert_eq!(class, "com.github.catvod.spider.PushAgent");
                assert!(!jars.is_empty(), "csp_PushAgent 应能定位到定义它的 jar");
                assert!(difficulty.is_some());
            }
            other => panic!("csp_PushAgent 应判为 unported，实际 {other:?}"),
        }
    }

    #[test]
    fn classifies_guarded_family_as_unsupported() {
        match classify(&site("guarded", "csp_BiliGuard", "")) {
            Classification::Unsupported { reason, .. } => {
                assert_eq!(reason, "protected_guard");
            }
            other => panic!("csp_BiliGuard 应判为受保护 jar，实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_api_is_classified_not_silent() {
        match classify(&site("x", "csp_DefinitelyNotInSamples", "")) {
            Classification::Unknown { .. } => {}
            other => panic!("未知 api 应判为 Unknown，实际 {other:?}"),
        }
        let err = site_error(&site("x", "csp_DefinitelyNotInSamples", ""));
        let msg = err.to_string();
        assert!(msg.contains("catvod.spider.unknown_api"), "错误信息需带分类码: {msg}");
    }

    #[test]
    fn unported_error_carries_class_and_difficulty() {
        let err = site_error(&site("pushagent", "csp_PushAgent", ""));
        let msg = err.to_string();
        assert!(msg.contains("catvod.spider.not_ported"), "{msg}");
        assert!(msg.contains("com.github.catvod.spider.PushAgent"), "{msg}");
    }
}

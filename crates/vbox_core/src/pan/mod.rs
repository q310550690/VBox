//! 网盘引擎（W9）：分享链接解析与协议客户端。
//!
//! 背景：剩余 unported 家族中很大一部分是「网盘分享/聚合」型（`csp_Quark`、
//! `csp_TZFile`、`csp_Moli`、`csp_XiaoZhang`、`csp_AliShare` 等）。原实现依赖
//! `com.github.catvod.parser.merge.A.Y`（Quark）等 1600+ 行的引擎层，本模块按
//! 同一协议在 Rust 侧重建最小闭环：**分享链接解析 → 分享 token → 目录列表 → 转存/直链**。
//!
//! 已实现 Quark（W9.1）、Ali（W9.3）、UC（W9.4）、115（W9.4）、Xunlei/PikPak 基座（W10.3）；
//! 后续按同样结构补 Thunder 家族与 YD。

pub mod ali;
pub mod login;
pub mod p115;
pub mod pikpak;
pub mod quark;
pub mod smb;
pub mod uc;
pub mod xunlei;

use crate::error::CoreError;

/// 网盘服务商。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanProvider {
    Quark,
    Ali,
    Uc,
    P115,
}

impl PanProvider {
    pub fn name(self) -> &'static str {
        match self {
            PanProvider::Quark => "quark",
            PanProvider::Ali => "ali",
            PanProvider::Uc => "uc",
            PanProvider::P115 => "p115",
        }
    }
}

/// 解析后的分享链接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareLink {
    pub provider: PanProvider,
    /// 分享 id（Quark 的 `pwd_id`）
    pub share_id: String,
    /// 提取码
    pub passcode: String,
    /// 分享内子目录 fid（Quark `#/list/share/{fid}`）
    pub sub_fid: String,
    /// 去掉 `#` 之后的原始链接
    pub url: String,
}

/// 解析分享链接；不支持的域名/形态返回 `None`。
pub fn parse_share_link(raw: &str) -> Option<ShareLink> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (before_hash, after_hash) = match raw.split_once('#') {
        Some((a, b)) => (a, Some(b)),
        None => (raw, None),
    };
    let (url, query) = match before_hash.split_once('?') {
        Some((a, b)) => (a, b),
        None => (before_hash, ""),
    };
    let passcode = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("pwd="))
        .unwrap_or("")
        .trim()
        .to_string();
    let sub_fid = after_hash
        .map(|h| {
            h.trim_start_matches('/')
                .strip_prefix("list/share/")
                .unwrap_or("")
                .split(['?', '/', '&'])
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .unwrap_or_default();

    let lower = url.to_ascii_lowercase();
    let (provider, start) = if let Some(i) = lower.find("pan.quark.cn/s/") {
        (PanProvider::Quark, i + "pan.quark.cn/s/".len())
    } else if let Some(i) = lower.find("www.alipan.com/s/") {
        (PanProvider::Ali, i + "www.alipan.com/s/".len())
    } else if let Some(i) = lower.find("www.aliyundrive.com/s/") {
        (PanProvider::Ali, i + "www.aliyundrive.com/s/".len())
    } else if let Some(i) = lower.find("alipan.com/s/") {
        (PanProvider::Ali, i + "alipan.com/s/".len())
    } else if let Some(i) = lower.find("aliyundrive.com/s/") {
        (PanProvider::Ali, i + "aliyundrive.com/s/".len())
    } else if let Some(i) = lower.find("drive.uc.cn/s/") {
        (PanProvider::Uc, i + "drive.uc.cn/s/".len())
    } else if let Some(i) = lower.find("pc.uc.cn/s/") {
        (PanProvider::Uc, i + "pc.uc.cn/s/".len())
    } else if let Some(i) = lower.find("115.com/s/") {
        (PanProvider::P115, i + "115.com/s/".len())
    } else {
        return None;
    };
    let tail = &url[start..];
    let segs: Vec<&str> = tail.split(['/', '?', '#', '&']).collect();
    let share_id = segs.first().copied().unwrap_or("").to_string();
    if share_id.is_empty() {
        return None;
    }
    // Ali 的分享内子目录走路径 `/s/{share}/folder/{fid}`；Quark 走 `#/list/share/{fid}`。
    let mut sub_fid = sub_fid;
    if provider == PanProvider::Ali && sub_fid.is_empty() {
        if let Some(pos) = segs.iter().position(|s| *s == "folder") {
            if let Some(v) = segs.get(pos + 1) {
                sub_fid = (*v).to_string();
            }
        }
    }
    Some(ShareLink {
        provider,
        share_id,
        passcode,
        sub_fid,
        url: before_hash.to_string(),
    })
}

/// 统一的网盘条目（各 provider 的文件模型字段差异较大，对外只暴露公共语义）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PanEntry {
    /// 文件/目录 id
    pub fid: String,
    pub name: String,
    pub is_dir: bool,
    /// 字节数（目录为 0）
    pub size: i64,
    pub thumbnail: String,
}

/// 分享浏览结果。
#[derive(Debug, Clone)]
pub struct ShareBrowse {
    pub provider: PanProvider,
    pub share_id: String,
    /// 分享标题（Quark 的 `data.share.title`，可能为空）
    pub title: String,
    pub files: Vec<PanEntry>,
}

/// 网盘配置：目前由 ext 的 `{"cookie": "...", "savepath": "..."}` 提供。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PanConfig {
    /// 通用 Cookie（桌面端约定的简化键）。
    #[serde(default)]
    pub cookie: String,
    /// 原 CatVod 配置键 `quark_cookie`（`parser/merge/I/a.java`）。
    #[serde(default)]
    pub quark_cookie: String,
    #[serde(default)]
    pub savepath: String,
    /// 原 CatVod 配置键 `ali_cookie`（`parser/merge/I/a.java`）。
    #[serde(default)]
    pub ali_cookie: String,
    /// 接口根覆盖（仅测试/镜像需要；原引擎固定 `drive.quark.cn`）。
    #[serde(default)]
    pub drive_base: String,
    /// Ali 接口根覆盖（默认 `https://api.aliyundrive.com/`）。
    #[serde(default)]
    pub ali_base: String,
    /// 原 CatVod 配置键 `uc_cookie`（`parser/merge/I/a.java`）。
    #[serde(default)]
    pub uc_cookie: String,
    /// UC 接口根覆盖（默认 `https://pc-api.uc.cn/1/clouddrive/`）。
    #[serde(default)]
    pub uc_base: String,
    /// 115 Cookie（原引擎从 WebView 拦截 `Set-Cookie` 的 `UID/SEID/CID`，桌面端从 ext 注入）。
    #[serde(default, alias = "115_cookie")]
    pub p115_cookie: String,
}

impl PanConfig {
    /// 解析 ext 字符串；空串/非 JSON 视为空配置（不报错，以便 ext 为空时可浏览公开分享）。
    pub fn parse(ext: &str) -> Self {
        serde_json::from_str::<PanConfig>(ext.trim()).unwrap_or_default()
    }

    /// 实际使用的 Cookie：优先 `quark_cookie`，回退通用 `cookie`。
    pub fn effective_cookie(&self) -> &str {
        if !self.quark_cookie.trim().is_empty() {
            self.quark_cookie.trim()
        } else {
            self.cookie.trim()
        }
    }

    /// 实际使用的 UC Cookie：优先 `uc_cookie`，回退通用 `cookie`。
    pub fn effective_uc_cookie(&self) -> &str {
        if !self.uc_cookie.trim().is_empty() {
            self.uc_cookie.trim()
        } else {
            self.cookie.trim()
        }
    }

    /// 实际使用的 115 Cookie：优先 `p115_cookie`，回退通用 `cookie`。
    pub fn effective_p115_cookie(&self) -> &str {
        if !self.p115_cookie.trim().is_empty() {
            self.p115_cookie.trim()
        } else {
            self.cookie.trim()
        }
    }

    /// 实际使用的 Ali Cookie：优先 `ali_cookie`，回退通用 `cookie`。
    pub fn effective_ali_cookie(&self) -> &str {
        if !self.ali_cookie.trim().is_empty() {
            self.ali_cookie.trim()
        } else {
            self.cookie.trim()
        }
    }

    /// 用登录 overlay 填充 ext 未提供的凭据（ext 显式配置优先，W13 §3 / W14 §9）。
    ///
    /// 只补空字段，不覆盖 ext；overlay 里对应 provider 无凭据或已过期时保持原样。
    pub fn with_overlay(mut self) -> Self {
        use crate::pan::login;
        if self.p115_cookie.trim().is_empty() {
            if let Some(c) = login::overlay_cookie(login::P115) {
                self.p115_cookie = c;
            }
        }
        if self.quark_cookie.trim().is_empty() {
            if let Some(c) = login::overlay_cookie(login::QUARK) {
                self.quark_cookie = c;
            }
        }
        if self.uc_cookie.trim().is_empty() {
            if let Some(c) = login::overlay_cookie(login::UC) {
                self.uc_cookie = c;
            }
        }
        if self.ali_cookie.trim().is_empty() {
            if let Some(c) = login::overlay_cookie(login::ALI) {
                self.ali_cookie = c;
            }
        }
        self
    }
}

/// 未配置 Cookie 时的统一错误（浏览公开分享不需要；转存/直链需要）。
pub fn cookie_required(provider: PanProvider) -> CoreError {
    CoreError::Config(format!(
        "{} 网盘需要 Cookie：请在站点 ext 中配置 {{\"cookie\":\"...\"}}",
        provider.name()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_share() {
        let l = parse_share_link("https://pan.quark.cn/s/24f565ff8b77").unwrap();
        assert_eq!(l.provider, PanProvider::Quark);
        assert_eq!(l.share_id, "24f565ff8b77");
        assert_eq!(l.passcode, "");
        assert_eq!(l.sub_fid, "");
        assert_eq!(l.url, "https://pan.quark.cn/s/24f565ff8b77");
    }

    #[test]
    fn parse_with_pwd_and_subdir() {
        let l = parse_share_link("https://pan.quark.cn/s/abc123?pwd=xy9z#/list/share/9f8e7d?nav=1")
            .unwrap();
        assert_eq!(l.share_id, "abc123");
        assert_eq!(l.passcode, "xy9z");
        assert_eq!(l.sub_fid, "9f8e7d");
        assert_eq!(l.url, "https://pan.quark.cn/s/abc123?pwd=xy9z");
    }

    #[test]
    fn reject_unknown_host() {
        assert!(parse_share_link("https://pan.baidu.com/s/1abc").is_none());
        assert!(parse_share_link("").is_none());
        assert!(parse_share_link("https://pan.quark.cn/s/").is_none());
    }

    #[test]
    fn config_reads_quark_cookie() {
        let c = PanConfig::parse(r#"{"quark_cookie":"ck=1","savepath":"/tv"}"#);
        assert_eq!(c.effective_cookie(), "ck=1");
        let c = PanConfig::parse(r#"{"cookie":"generic"}"#);
        assert_eq!(c.effective_cookie(), "generic");
    }

    #[test]
    fn parse_ali_share_plain() {
        let l = parse_share_link("https://www.alipan.com/s/abc123").unwrap();
        assert_eq!(l.provider, PanProvider::Ali);
        assert_eq!(l.share_id, "abc123");
        assert_eq!(l.passcode, "");
        assert_eq!(l.sub_fid, "");
        assert_eq!(l.url, "https://www.alipan.com/s/abc123");
    }

    #[test]
    fn parse_ali_share_folder_and_pwd() {
        let l = parse_share_link(
            "https://www.aliyundrive.com/s/abc123/folder/9f8e7d?pwd=xy9z",
        )
        .unwrap();
        assert_eq!(l.provider, PanProvider::Ali);
        assert_eq!(l.share_id, "abc123");
        assert_eq!(l.passcode, "xy9z");
        assert_eq!(l.sub_fid, "9f8e7d");
        assert_eq!(
            l.url,
            "https://www.aliyundrive.com/s/abc123/folder/9f8e7d?pwd=xy9z"
        );
    }

    #[test]
    fn config_reads_ali_cookie() {
        let c = PanConfig::parse(r#"{"ali_cookie":"ck=2"}"#);
        assert_eq!(c.effective_ali_cookie(), "ck=2");
        let c = PanConfig::parse(r#"{"cookie":"generic"}"#);
        assert_eq!(c.effective_ali_cookie(), "generic");
    }

    #[test]
    fn parse_uc_share_plain_and_subdir() {
        let l = parse_share_link("https://drive.uc.cn/s/abcdef").unwrap();
        assert_eq!(l.provider, PanProvider::Uc);
        assert_eq!(l.share_id, "abcdef");
        let l = parse_share_link("https://drive.uc.cn/s/abcdef?pwd=xy9z#/list/share/9f8e7d")
            .unwrap();
        assert_eq!(l.provider, PanProvider::Uc);
        assert_eq!(l.passcode, "xy9z");
        assert_eq!(l.sub_fid, "9f8e7d");
    }

    #[test]
    fn config_reads_uc_cookie() {
        let c = PanConfig::parse(r#"{"uc_cookie":"ck=uc"}"#);
        assert_eq!(c.effective_uc_cookie(), "ck=uc");
        let c = PanConfig::parse(r#"{"cookie":"generic"}"#);
        assert_eq!(c.effective_uc_cookie(), "generic");
    }

    #[test]
    fn pan_config_parses_and_defaults() {
        let c = PanConfig::parse("{\"cookie\":\"a=b; c=d\"}");
        assert_eq!(c.cookie, "a=b; c=d");
        assert_eq!(PanConfig::parse("").cookie, "");
        assert_eq!(PanConfig::parse("not-json").savepath, "");
    }
}

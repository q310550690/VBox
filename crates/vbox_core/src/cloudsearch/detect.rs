//! 分享链接识别与网盘类型判定（W14 §4）。
//!
//! 设计取舍：
//! - 已有 [`crate::pan::parse_share_link`] 覆盖 Quark / Ali / UC / 115 四家，优先复用；
//!   本模块只补它未覆盖的 Xunlei / PikPak / Baidu / 天翼 / 移动云盘 / 123 / 115 短域 / 磁力 / ED2K。
//! - 支持无 scheme 的裸域名（补 `https://`），也支持「一段文本里夹带链接」：先抽取候选 URL 再识别。
//! - 提取码从 URL 查询参数读取（`pwd` / `password` / `passcode` / `pass` 以及中文键），
//!   兼容 `?pwd=xxxx` 形态；magnet / ed2k 没有提取码。

use super::SearchProvider;
use crate::pan::{parse_share_link, PanProvider};
use url::Url;

/// 识别结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedShare {
    pub provider: SearchProvider,
    /// 归一化后的分享链接（裸域名会补 `https://`；magnet/ed2k 保留原文）。
    pub share_url: String,
    /// 分享 id（magnet 为 btih hash，ed2k 为文件 hash）。
    pub share_id: String,
    pub password: Option<String>,
}

/// 参与扫描的链接特征串（全部小写）；用于从文本里抽候选链接，也用于给 `detect_share` 兜底。
const SHARE_MARKERS: [&str; 16] = [
    "magnet:",
    "ed2k:",
    "pan.quark.cn",
    "drive.uc.cn",
    "pc.uc.cn",
    "115.com",
    "anxia.com",
    "pan.xunlei.com",
    "mypikpak.com",
    "pikpak.com",
    "aliyundrive.com",
    "alipan.com",
    "pan.baidu.com",
    "cloud.189.cn",
    "caiyun.139.com",
    "123pan.com",
];

/// 提取码查询参数名（大小写不敏感；兼容中文键）。
const PASSWORD_KEYS: [&str; 7] = [
    "pwd",
    "password",
    "passcode",
    "pass",
    "提取码",
    "访问码",
    "密码",
];

/// 识别一个分享链接；不支持的域名 / 形态返回 `None`。
pub fn detect_share(url: &str) -> Option<DetectedShare> {
    let cleaned = trim_wrapping(url.trim());
    if cleaned.is_empty() {
        return None;
    }
    if let Some(share) = detect_one(cleaned) {
        return Some(share);
    }
    // 容错：输入是「频道消息文本」这种夹带前后缀的字符串时，抽第一个候选链接再识别。
    extract_share_links(cleaned)
        .into_iter()
        .find_map(|candidate| detect_one(&candidate))
}

/// 从一段文本中抽取所有候选分享链接（去重、保持出现顺序）。
///
/// 仅做「定位 + 截断」，是否真的是可识别分享链接由 [`detect_share`] 判断。
pub fn extract_share_links(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let lower = text.to_ascii_lowercase();
    let mut hits: Vec<(usize, &'static str)> = Vec::new();
    for marker in SHARE_MARKERS {
        for (idx, _) in lower.match_indices(marker) {
            hits.push((idx, marker));
        }
    }
    if hits.is_empty() {
        return Vec::new();
    }
    hits.sort_by_key(|(idx, _)| *idx);

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut consumed_upto = 0usize;
    for (marker_idx, marker) in hits {
        if marker_idx < consumed_upto {
            continue;
        }
        let start = link_start(&lower, marker_idx);
        let end = link_end(text, start, marker.starts_with("ed2k"));
        if end <= start {
            continue;
        }
        let candidate = trim_trailing_punctuation(&text[start..end]).to_string();
        consumed_upto = end;
        if candidate.is_empty() || !seen.insert(candidate.clone()) {
            continue;
        }
        out.push(candidate);
    }
    out
}

/// 链接起点：优先补回紧邻的 `https://` / `http://`，其次补回 `www.` / `m.` 前缀。
fn link_start(lower: &str, marker_idx: usize) -> usize {
    let head = &lower[..marker_idx];
    for scheme in ["https://", "http://"] {
        if let Some(pos) = head.rfind(scheme) {
            // 只接受紧贴 marker 的 scheme（中间不允许有其它内容）。
            if marker_idx - pos <= scheme.len() + 4 {
                return pos;
            }
        }
    }
    for prefix in ["www.", "m.", "wap."] {
        if head.ends_with(prefix) {
            return marker_idx - prefix.len();
        }
    }
    marker_idx
}

/// 链接终点：一直取到空白 / 非 ASCII（中文）/ 包裹符为止；ED2K 额外允许 `|`。
fn link_end(text: &str, start: usize, is_ed2k: bool) -> usize {
    let mut end = start;
    for (offset, ch) in text[start..].char_indices() {
        let allowed = (ch.is_ascii()
            && !ch.is_ascii_whitespace()
            && !matches!(ch, '<' | '>' | '"' | '\'' | '`'))
            || (is_ed2k && ch == '|');
        if !allowed {
            end = start + offset;
            break;
        }
        end = start + offset + ch.len_utf8();
    }
    end
}

/// 去掉链接首尾的包裹字符（markdown / 中文标点）。
fn trim_wrapping(raw: &str) -> &str {
    raw.trim_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\''
                    | '`'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '<'
                    | '>'
                    | '，'
                    | '。'
                    | '、'
                    | '；'
                    | '：'
                    | '！'
                    | '？'
                    | '（'
                    | '）'
                    | '【'
                    | '】'
                    | '《'
                    | '》'
                    | '「'
                    | '」'
                    | '『'
                    | '』'
            )
    })
}

/// 去掉链接末尾的句读标点（保留 `|/` 之类协议结尾）。
fn trim_trailing_punctuation(raw: &str) -> &str {
    raw.trim_end_matches(|c: char| {
        matches!(
            c,
            '.' | ','
                | ';'
                | ':'
                | '!'
                | '?'
                | ')'
                | ']'
                | '}'
                | '\''
                | '"'
                | '&'
                | '、'
                | '。'
                | '，'
                | '：'
                | '；'
                | '！'
                | '？'
                | '）'
                | '】'
                | '》'
                | '」'
                | '』'
        )
    })
}

/// 单个候选串的识别。
fn detect_one(raw: &str) -> Option<DetectedShare> {
    let raw = trim_wrapping(raw.trim());
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("magnet:") {
        return detect_magnet(raw);
    }
    if lower.starts_with("ed2k://") {
        return detect_ed2k(raw);
    }

    let with_scheme = if lower.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let parsed = Url::parse(&with_scheme).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed
        .host_str()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let password = password_from_url(&parsed);

    // 优先级 1：复用 W9 的网盘分享解析（Quark / Ali / UC / 115）。
    if let Some(link) = parse_share_link(&with_scheme) {
        let provider = match link.provider {
            PanProvider::Quark => SearchProvider::Quark,
            PanProvider::Ali => SearchProvider::Aliyun,
            PanProvider::Uc => SearchProvider::Uc,
            PanProvider::P115 => SearchProvider::P115,
        };
        if link.share_id.is_empty() {
            return None;
        }
        let mut password = password;
        if password.is_none() && !link.passcode.trim().is_empty() {
            password = Some(link.passcode.trim().to_string());
        }
        return Some(DetectedShare {
            provider,
            share_url: with_scheme,
            share_id: link.share_id,
            password,
        });
    }

    // 优先级 2：本模块补充的域名。
    let provider = provider_for_host(&host)?;
    let share_id = share_id_for(provider, &parsed)?;
    Some(DetectedShare {
        provider,
        share_url: with_scheme,
        share_id,
        password,
    })
}

/// 域名 → provider（host 已小写、无末尾点）。
fn provider_for_host(host: &str) -> Option<SearchProvider> {
    let host = host.strip_prefix("www.").unwrap_or(host);
    let matched = if host.ends_with("quark.cn") {
        SearchProvider::Quark
    } else if host.ends_with("uc.cn") {
        SearchProvider::Uc
    } else if host == "115.com" || host.ends_with(".115.com") || host == "anxia.com" {
        SearchProvider::P115
    } else if host.ends_with("xunlei.com") {
        SearchProvider::Xunlei
    } else if host.ends_with("pikpak.com") {
        SearchProvider::PikPak
    } else if host.ends_with("aliyundrive.com") || host.ends_with("alipan.com") {
        SearchProvider::Aliyun
    } else if host.ends_with("baidu.com") {
        SearchProvider::Baidu
    } else if host.ends_with("189.cn") {
        SearchProvider::Tianyi
    } else if host.ends_with("139.com") {
        SearchProvider::Mobile
    } else if host.ends_with("123pan.com") {
        SearchProvider::P123
    } else {
        return None;
    };
    Some(matched)
}

/// 从 path / query 里取 share id。
fn share_id_for(provider: SearchProvider, parsed: &Url) -> Option<String> {
    let segments: Vec<&str> = parsed
        .path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let after = |key: &str| -> Option<String> {
        let position = segments
            .iter()
            .position(|segment| segment.eq_ignore_ascii_case(key))?;
        let id = clean_id(segments.get(position + 1)?);
        if id.is_empty() {
            None
        } else {
            Some(id)
        }
    };

    let id = match provider {
        SearchProvider::Tianyi => after("t").or_else(|| after("s")),
        // 139 云盘：`/w/i/<id>`、`/m/i?<id>` 等形态，最后回退到 query 首值。
        SearchProvider::Mobile => after("i").or_else(|| after("s")).or_else(|| {
            segments
                .last()
                .map(|segment| clean_id(segment))
                .filter(|id| !id.is_empty())
                .or_else(|| {
                    parsed
                        .query_pairs()
                        .map(|(_, value)| clean_id(&value))
                        .find(|id| !id.is_empty())
                })
        }),
        _ => after("s"),
    };
    id
}

/// 分享 id 只保留路径段内的有效字符。
fn clean_id(raw: &str) -> String {
    raw.trim()
        .split(['/', '?', '#', '&'])
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 从 URL query（以及带 `=` 的 fragment）提取提取码。
fn password_from_url(parsed: &Url) -> Option<String> {
    let from_pairs = |pairs: url::form_urlencoded::Parse<'_>| -> Option<String> {
        for (key, value) in pairs {
            if PASSWORD_KEYS
                .iter()
                .any(|candidate| key.eq_ignore_ascii_case(candidate))
            {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        None
    };
    if let Some(password) = from_pairs(parsed.query_pairs()) {
        return Some(password);
    }
    let fragment = parsed.fragment()?;
    if fragment.contains('=') {
        from_pairs(url::form_urlencoded::parse(fragment.as_bytes()))
    } else {
        None
    }
}

/// `magnet:?xt=urn:btih:<hash>&...`
fn detect_magnet(raw: &str) -> Option<DetectedShare> {
    let lower = raw.to_ascii_lowercase();
    let position = lower.find("urn:btih:")? + "urn:btih:".len();
    let hash: String = raw[position..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    // BTIH 为 40 位 hex 或 32 位 base32；过短视为噪声。
    if hash.len() < 20 {
        return None;
    }
    Some(DetectedShare {
        provider: SearchProvider::Magnet,
        share_url: raw.to_string(),
        share_id: hash.to_ascii_lowercase(),
        password: None,
    })
}

/// `ed2k://|file|<name>|<size>|<hash>|/`
fn detect_ed2k(raw: &str) -> Option<DetectedShare> {
    let parts: Vec<&str> = raw.split('|').collect();
    if parts.len() < 6 || !parts[1].eq_ignore_ascii_case("file") {
        return None;
    }
    let hash = parts[4].trim();
    if hash.is_empty() {
        return None;
    }
    Some(DetectedShare {
        provider: SearchProvider::Ed2k,
        share_url: raw.to_string(),
        share_id: hash.to_ascii_lowercase(),
        password: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(url: &str) -> DetectedShare {
        detect_share(url).unwrap_or_else(|| panic!("应该识别为分享链接: {url}"))
    }

    #[test]
    fn detects_quark_share() {
        let share = detect("https://pan.quark.cn/s/24f565ff8b77");
        assert_eq!(share.provider, SearchProvider::Quark);
        assert_eq!(share.share_id, "24f565ff8b77");
        assert_eq!(share.share_url, "https://pan.quark.cn/s/24f565ff8b77");
        assert!(share.password.is_none());
    }

    #[test]
    fn detects_uc_share_with_pwd() {
        let share = detect("https://drive.uc.cn/s/abcdef?pwd=xy9z");
        assert_eq!(share.provider, SearchProvider::Uc);
        assert_eq!(share.share_id, "abcdef");
        assert_eq!(share.password.as_deref(), Some("xy9z"));
    }

    #[test]
    fn detects_115_and_anxia() {
        let share = detect("https://115.com/s/sw123abc?password=abcd");
        assert_eq!(share.provider, SearchProvider::P115);
        assert_eq!(share.share_id, "sw123abc");
        assert_eq!(share.password.as_deref(), Some("abcd"));

        let short = detect("https://anxia.com/s/sw123abc");
        assert_eq!(short.provider, SearchProvider::P115);
        assert_eq!(short.share_id, "sw123abc");
    }

    #[test]
    fn detects_xunlei_share() {
        let share = detect("https://pan.xunlei.com/s/VNabc123");
        assert_eq!(share.provider, SearchProvider::Xunlei);
        assert_eq!(share.share_id, "VNabc123");
    }

    #[test]
    fn detects_pikpak_share() {
        let share = detect("https://mypikpak.com/s/VNabc123");
        assert_eq!(share.provider, SearchProvider::PikPak);
        assert_eq!(share.share_id, "VNabc123");
    }

    #[test]
    fn detects_aliyun_both_domains() {
        for url in [
            "https://www.aliyundrive.com/s/abc123",
            "https://www.alipan.com/s/abc123",
        ] {
            let share = detect(url);
            assert_eq!(share.provider, SearchProvider::Aliyun);
            assert_eq!(share.share_id, "abc123");
        }
    }

    #[test]
    fn detects_baidu_share_with_pwd() {
        let share = detect("https://pan.baidu.com/s/1abc?pwd=1234");
        assert_eq!(share.provider, SearchProvider::Baidu);
        assert_eq!(share.share_id, "1abc");
        assert_eq!(share.password.as_deref(), Some("1234"));
    }

    #[test]
    fn detects_tianyi_share() {
        let share = detect("https://cloud.189.cn/t/AbCdEf123");
        assert_eq!(share.provider, SearchProvider::Tianyi);
        assert_eq!(share.share_id, "AbCdEf123");
    }

    #[test]
    fn detects_mobile_139_share() {
        let share = detect("https://caiyun.139.com/w/i/2jQXnSample");
        assert_eq!(share.provider, SearchProvider::Mobile);
        assert_eq!(share.share_id, "2jQXnSample");
    }

    #[test]
    fn detects_123pan_share() {
        let share = detect("https://www.123pan.com/s/abc123");
        assert_eq!(share.provider, SearchProvider::P123);
        assert_eq!(share.share_id, "abc123");
    }

    #[test]
    fn detects_magnet_link() {
        let hash = "aaaabbbbccccddddeeeeffff0000111122223333";
        let share = detect(&format!("magnet:?xt=urn:btih:{hash}&dn=test"));
        assert_eq!(share.provider, SearchProvider::Magnet);
        assert_eq!(share.share_id, hash);
        assert!(share.share_url.starts_with("magnet:?xt=urn:btih:"));
        assert!(!share.provider.is_playable());
    }

    #[test]
    fn detects_ed2k_link() {
        let share = detect("ed2k://|file|movie.mkv|734003200|AABBCCDDEEFF00112233445566778899|/");
        assert_eq!(share.provider, SearchProvider::Ed2k);
        assert_eq!(share.share_id, "aabbccddeeff00112233445566778899");
        assert!(!share.provider.is_playable());
    }

    #[test]
    fn supports_bare_domain() {
        let share = detect("pan.quark.cn/s/abc123");
        assert_eq!(share.provider, SearchProvider::Quark);
        assert_eq!(share.share_url, "https://pan.quark.cn/s/abc123");
    }

    #[test]
    fn extracts_password_variants() {
        for url in [
            "https://pan.baidu.com/s/1abc?pwd=abcd",
            "https://pan.baidu.com/s/1abc?password=abcd",
            "https://pan.baidu.com/s/1abc?passcode=abcd",
        ] {
            assert_eq!(detect(url).password.as_deref(), Some("abcd"), "{url}");
        }
    }

    #[test]
    fn rejects_non_share_links() {
        for url in [
            "",
            "   ",
            "hello world",
            "https://example.com/s/abc",
            "https://pan.quark.cn/",
            "https://pan.quark.cn/s/",
            "https://www.bilibili.com/video/BV1xx",
            "magnet:?xt=urn:btih:short",
        ] {
            assert!(detect_share(url).is_none(), "不应识别为分享链接: {url:?}");
        }
    }

    #[test]
    fn detects_link_inside_text() {
        let text = "资源：https://pan.quark.cn/s/abc123?pwd=xy9z 提取码：xy9z";
        let share = detect_share(text).expect("应该从文本里识别出链接");
        assert_eq!(share.provider, SearchProvider::Quark);
        assert_eq!(share.share_id, "abc123");
        assert_eq!(share.password.as_deref(), Some("xy9z"));
    }

    #[test]
    fn extracts_multiple_links_in_order() {
        let text = "合集 magnet:?xt=urn:btih:aaaabbbbccccddddeeeeffff0000111122223333 \
                    和 https://pan.baidu.com/s/1abc?pwd=1234 。";
        let links = extract_share_links(text);
        assert_eq!(links.len(), 2, "{links:?}");
        assert!(links[0].starts_with("magnet:"));
        assert!(links[1].starts_with("https://pan.baidu.com/s/1abc"));
        // `。` 是中文句号，扫描时已在非 ASCII 处截断。
        assert_eq!(
            detect_share(&links[1]).unwrap().password.as_deref(),
            Some("1234")
        );
    }

    #[test]
    fn extract_share_links_dedupes() {
        let text = "https://pan.quark.cn/s/abc https://pan.quark.cn/s/abc";
        assert_eq!(extract_share_links(text).len(), 1);
    }
}

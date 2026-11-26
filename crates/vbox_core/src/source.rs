//! 源站分发层
//!
//! 前端/CLI 只调用这里的统一函数；内部按 `site.kind` 路由到具体实现：
//! - type=1  -> 苹果CMS JSON（sources/cms.rs，已实现）
//! - type=0  -> XML（sources/xml.rs，Stage 6 实现）
//! - type=3  -> Spider/drpy（sources/spider.rs，Stage 7 实现）

use crate::config::find_site;
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::progress::{shorten, ProgressSink};
use std::collections::HashMap;

/// 把源实现内部的裸错误统一收口为 catvod 稳定码（见 `sources::map_source_error`）。
fn mapped<T>(site: &Site, result: Result<T>) -> Result<T> {
    result.map_err(|e| crate::sources::map_source_error(site, e))
}

/// 列出某站点的分类
pub fn list_categories(cfg: &TvConfig, site_key: &str) -> Result<Vec<Category>> {
    let site = find_site(cfg, site_key)?;
    match site.kind {
        1 => mapped(site, crate::sources::cms::list_categories(cfg, site)),
        0 => mapped(site, crate::sources::xml::list_categories(cfg, site)),
        3 => mapped(site, crate::sources::spider::list_categories(cfg, site)),
        4 => mapped(site, crate::sources::remote::list_categories(cfg, site)),
        k => Err(CoreError::Config(format!("不支持的源类型 type={}", k))),
    }
}

/// 列出某分类下的视频
pub fn list_videos(cfg: &TvConfig, site_key: &str, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let site = find_site(cfg, site_key)?;
    match site.kind {
        1 => mapped(site, crate::sources::cms::list_videos(cfg, site, cat, page)),
        0 => mapped(site, crate::sources::xml::list_videos(cfg, site, cat, page)),
        3 => mapped(site, crate::sources::spider::list_videos(cfg, site, cat, page)),
        4 => mapped(site, crate::sources::remote::list_videos(cfg, site, cat, page)),
        k => Err(CoreError::Config(format!("不支持的源类型 type={}", k))),
    }
}

/// 获取详情（含多线路选集）
pub fn get_detail(cfg: &TvConfig, site_key: &str, id: &str) -> Result<VideoDetail> {
    let site = find_site(cfg, site_key)?;
    match site.kind {
        1 => mapped(site, crate::sources::cms::get_detail(cfg, site, id)),
        0 => mapped(site, crate::sources::xml::get_detail(cfg, site, id)),
        3 => mapped(site, crate::sources::spider::get_detail(cfg, site, id)),
        4 => mapped(site, crate::sources::remote::get_detail(cfg, site, id)),
        k => Err(CoreError::Config(format!("不支持的源类型 type={}", k))),
    }
}

/// 解析播放地址（旧 API，无站点上下文；仅能处理直接播放页 + 全局解析链）
pub fn resolve_play_url(cfg: &TvConfig, play_url: &str) -> Result<PlayResult> {
    crate::parse::run_parse_chain(cfg, play_url, None, &HashMap::new(), None)
        .map_err(crate::sources::map_play_error)
}

/// 点播播放：站点详情 -> 播放信息 -> parses 链（或直链）
///
/// - type0/1/4：`url` 是剧集地址（可能是直链，也可能是播放页）
/// - type3：`id` 是剧集 id，调用 spider.playerContent(flag,id,vipFlags)
pub fn get_play(
    cfg: &TvConfig,
    site_key: &str,
    flag: &str,
    id_or_url: &str,
    raw_url: &str,
    parse_name: Option<&str>,
) -> Result<PlayResult> {
    get_play_with_progress(
        cfg,
        site_key,
        flag,
        id_or_url,
        raw_url,
        parse_name,
        None,
        &ProgressSink::none(),
    )
}

/// 带进度回调的播放解析：桌面端用它展示「取播放信息 → 解析链 → 抓包」到哪一步。
///
/// 与 [`get_play`] 行为一致，只是把关键节点通过 [`ProgressSink`] 推给调用方。
pub fn get_play_with_progress(
    cfg: &TvConfig,
    site_key: &str,
    flag: &str,
    id_or_url: &str,
    raw_url: &str,
    parse_name: Option<&str>,
    default_parse: Option<&str>,
    progress: &ProgressSink,
) -> Result<PlayResult> {
    let site = find_site(cfg, site_key)?;
    let headers = HashMap::new();
    progress.emit_detail(
        "prepare",
        "start",
        format!("准备播放：{}", site.name),
        Some(if flag.trim().is_empty() {
            format!("站点 {} · type={}", site.key, site.kind)
        } else {
            format!("站点 {} · 线路 {} · type={}", site.key, flag, site.kind)
        }),
    );

    if site.kind == 3 {
        progress.emit_detail(
            "request",
            "start",
            "调用 spider 获取播放信息",
            Some(shorten(&site.api, 180)),
        );
        let json = mapped(site, crate::sources::spider::player_content(cfg, site, flag, id_or_url, &cfg.flags))
            .map_err(|e| {
                progress.emit_detail(
                    "request",
                    "fail",
                    "spider 获取播放信息失败",
                    Some(e.to_string()),
                );
                e
            })?;
        progress.emit("request", "ok", "spider 已返回播放信息");
        return play_result_from_spider_json(cfg, site, &json, parse_name, default_parse, progress);
    }

    // type4：spider 协议跑在远端 HTTP API 上；先取远端播放 JSON，再按 parse/jx 走解析链
    if site.kind == 4 {
        progress.emit_detail(
            "request",
            "start",
            "请求远程 spider 接口",
            Some(shorten(&site.api, 180)),
        );
        let json = mapped(site, crate::sources::remote::get_play(cfg, site, flag, id_or_url)).map_err(|e| {
            progress.emit_detail(
                "request",
                "fail",
                "远程 spider 接口请求失败",
                Some(e.to_string()),
            );
            e
        })?;
        progress.emit("request", "ok", "远程 spider 已返回播放信息");
        return play_result_from_spider_json(cfg, site, &json, parse_name, default_parse, progress);
    }

    let target = if !raw_url.trim().is_empty() {
        raw_url.trim()
    } else {
        id_or_url.trim()
    };
    if target.is_empty() {
        progress.emit("prepare", "fail", "剧集地址为空，无法解析");
        return Err(CoreError::Config("剧集地址为空".into()));
    }
    // 直链：直接播放（先做可用性探测：HEAD 失败回退 Range GET）
    if crate::parse::is_direct_media_url(target) {
        match crate::probe::probe_media_url(target, &headers) {
            crate::probe::ProbeOutcome::Available => {
                progress.emit_detail(
                    "prepare",
                    "ok",
                    "剧集地址已是媒体直链",
                    Some(shorten(target, 180)),
                );
                return Ok(PlayResult {
                    url: target.to_string(),
                    headers,
                    ..Default::default()
                });
            }
            crate::probe::ProbeOutcome::Unavailable(reason) => {
                progress.emit_detail(
                    "prepare",
                    "fail",
                    "直链不可用（HEAD/Range 校验失败）",
                    Some(format!("{reason}；可尝试切换线路或解析器")),
                );
                return Err(crate::error::CoreError::Api {
                    code: 0,
                    msg: format!("直链不可用（HEAD 与 Range GET 校验都失败）：{reason}"),
                });
            }
            crate::probe::ProbeOutcome::Inconclusive(reason) => {
                progress.emit_detail(
                    "prepare",
                    "info",
                    "直链探测无法判定（网络/服务器限制），仍交给播放器尝试",
                    Some(shorten(&reason, 180)),
                );
                return Ok(PlayResult {
                    url: target.to_string(),
                    headers,
                    ..Default::default()
                });
            }
        }
    }
    // 播放页：站点级 playUrl 前缀 + 剧集地址
    let play_page = if target.starts_with("http://") || target.starts_with("https://") {
        target.to_string()
    } else {
        format!("{}{}", site.player_url, target)
    };
    progress.emit_detail(
        "prepare",
        "info",
        "拼装播放页地址",
        Some(shorten(&play_page, 180)),
    );

    // 对齐 TVBox `PlayFragment.initParse`（useParse=false 分支）：
    // 站点（CMS/XML）自己返回的播放页**默认直接交给 WebView 嗅探**，不走全局 parses 链。
    // 只有「用户显式指定了解析器」或「播放 flag 命中配置的 VIP 解析白名单」时，
    // 才跑全局解析链（对应 TVBox 的 `playUrl.isEmpty() && vipParseFlags.contains(flag)`）。
    // 这正是「同样的源，TVBox 能播、VBox 解析失败」的根因：旧逻辑先跑全局解析器，
    // 结果落到第三方解析页（广告页）里嗅探，而真正的 m3u8 就写在站点播放页的内联脚本里。
    let vip_flag = !flag.trim().is_empty() && cfg.flags.iter().any(|f| f == flag);
    if parse_name.is_none() && !vip_flag {
        progress.emit_detail(
            "prepare",
            "ok",
            "播放页交给 WebView 嗅探（TVBox 默认路径）",
            Some(shorten(&play_page, 180)),
        );
        return Ok(PlayResult {
            url: play_page,
            headers,
            sniff: true,
            ..Default::default()
        });
    }
    progress.emit("prepare", "info", "按解析器进入解析链");
    let preferred = parse_name
        .map(|s| s.to_string())
        .or_else(|| default_parse.map(|s| s.to_string()))
        .or_else(|| cfg.parses.first().map(|p| p.name.clone()));
    crate::parse::run_parse_chain_with(
        cfg,
        &play_page,
        preferred.as_deref(),
        &headers,
        Some(site),
        progress,
    )
    .map_err(crate::sources::map_play_error)
}

/// 把 spider.playerContent 返回的 JSON 归一成播放页/直链
fn play_result_from_spider_json(
    cfg: &TvConfig,
    site: &Site,
    json: &serde_json::Value,
    parse_name: Option<&str>,
    default_parse: Option<&str>,
    progress: &ProgressSink,
) -> Result<PlayResult> {
    // 宽容口径：老式 spider 的 playerContent 直接返回地址字符串（或字符串数组），
    // 不是 JSON 对象；TVBox 对这两种形态都按直链处理。
    if let Some(raw) = json.as_str() {
        let raw = raw.trim();
        if !raw.is_empty() {
            return Ok(PlayResult {
                url: raw.to_string(),
                ..Default::default()
            });
        }
        return Err(CoreError::Config("spider playerContent 返回空字符串".into()));
    }
    if let Some(arr) = json.as_array() {
        if let Some(raw) = arr.iter().find_map(|v| v.as_str()).map(str::trim) {
            if !raw.is_empty() {
                return Ok(PlayResult {
                    url: raw.to_string(),
                    ..Default::default()
                });
            }
        }
        return Err(CoreError::Config("spider playerContent 返回空数组".into()));
    }
    let obj = json
        .as_object()
        .ok_or_else(|| CoreError::Config("spider playerContent 返回不是 JSON 对象".into()))?;
    let raw = obj
        .get("url")
        .map(|v| {
            if let Some(arr) = v.as_array() {
                // 清晰度数组：奇数位是地址，取第一个
                arr.iter()
                    .skip(1)
                    .step_by(2)
                    .find_map(|x| x.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                v.as_str().unwrap_or("").to_string()
            }
        })
        .unwrap_or_default();
    if raw.is_empty() {
        let msg = obj.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        return Err(CoreError::Config(if msg.is_empty() {
            "spider playerContent 未返回 url".into()
        } else {
            format!("spider playerContent 失败: {msg}")
        }));
    }
    let mut headers: HashMap<String, String> = HashMap::new();
    for key in ["header", "headers"] {
        if let Some(h) = obj.get(key).and_then(|v| v.as_object()) {
            for (k, v) in h {
                headers.insert(k.clone(), v.as_str().unwrap_or_default().to_string());
            }
        }
    }
    let play_url_prefix = obj.get("playUrl").and_then(|v| v.as_str()).unwrap_or("");
    // 对齐 TVBox `info.optString("parse", "1")`：字段缺省视为需要解析。
    let parse_flag = match obj.get("parse") {
        None => true,
        Some(v) => match v {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(n) => n.as_i64() == Some(1),
            serde_json::Value::String(s) => matches!(s.trim(), "1" | "true" | "TRUE" | "True"),
            _ => false,
        },
    };
    let jx = obj.get("jx").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
    let play_flag = obj.get("flag").and_then(|v| v.as_str()).unwrap_or("");

    let direct = if crate::parse::is_direct_media_url(&raw) {
        raw.clone()
    } else {
        format!("{play_url_prefix}{raw}")
    };
    if !parse_flag && !jx {
        progress.emit_detail(
            "prepare",
            "ok",
            "播放信息已是媒体直链",
            Some(shorten(&direct, 180)),
        );
        return Ok(PlayResult {
            url: direct,
            headers,
            ..Default::default()
        });
    }

    // 对齐 TVBox `initParse`：只有 jx、或「播放信息没有 playUrl 前缀且 flag 命中 VIP
    // 解析白名单」、或用户显式选了解析器时，才跑全局 parses 链；否则播放页直接嗅探。
    let vip_flag = play_url_prefix.is_empty()
        && !play_flag.trim().is_empty()
        && cfg.flags.iter().any(|f| f == play_flag);
    if parse_name.is_none() && !jx && !vip_flag {
        let sniff = !crate::parse::is_direct_media_url(&direct);
        progress.emit_detail(
            "prepare",
            "ok",
            if sniff {
                "播放页交给 WebView 嗅探（TVBox 默认路径）"
            } else {
                "播放信息已是媒体直链"
            },
            Some(shorten(&direct, 180)),
        );
        return Ok(PlayResult {
            url: direct,
            headers,
            sniff,
            ..Default::default()
        });
    }

    progress.emit_detail(
        "prepare",
        "info",
        "播放信息需要二次解析（jx / VIP flag / 指定解析器）",
        Some(shorten(&direct, 180)),
    );
    let preferred = parse_name
        .map(|s| s.to_string())
        .or_else(|| default_parse.map(|s| s.to_string()))
        .or_else(|| cfg.parses.first().map(|p| p.name.clone()));
    let mut result = crate::parse::run_parse_chain_with(
        cfg,
        &direct,
        preferred.as_deref(),
        &headers,
        Some(site),
        progress,
    )
    .map_err(crate::sources::map_play_error)?;
    if result.parse_name.is_none() {
        result.parse_name = preferred;
    }
    Ok(result)
}

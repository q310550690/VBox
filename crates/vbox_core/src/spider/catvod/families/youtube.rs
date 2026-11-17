//! `csp_Youtube` 家族：经外部 `yt-dlp` 进程桥接 YouTube（W10.4）。
//!
//! 逆向结论（`Youtube.java` 1756 行）：原始蜘蛛是 InnerTube + DASH 生成器，签名格式需要
//! 执行 YouTube 混淆 JS（`g0/*` 约 2400 行）。桌面端不移植该 JS 解释器，改为调用
//! `yt-dlp` 子进程：搜索/列表用 `--flat-playlist -J`，详情用 `-J`，播放用 `-f best -J`
//! 取直链与请求头。命令可从站点 ext 的 `ytdlp` 键覆盖，
//! 也可用环境变量 `VBOX_YTDLP`（两者都支持带参数的命令行，按空白切分）。

use serde_json::{json, Value};

use crate::error::CoreError;

use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::spider::catvod::contract::{CatvodError, ErrorClass};

/// 家族 api 标识。
pub const API: &str = "csp_Youtube";
/// 详情页线路名。
const GROUP: &str = "YouTube";
/// 列表每页条数（与手写家族一致）。
const PAGE_SIZE: usize = 20;
/// 搜索最多条数。
const SEARCH_LIMIT: usize = 20;
/// 环境变量覆盖键。
const ENV_RUNNER: &str = "VBOX_YTDLP";
/// 默认命令。
const DEFAULT_RUNNER: &str = "yt-dlp";

/// 是否由本家族接管该 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

fn play_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::PlayParse, site, detail).into_core()
}

/// 站点 ext 归一为 JSON 对象（ext 可以是对象，也可以是 JSON 字符串）。
fn ext_object(site: &Site) -> Option<Value> {
    match site.ext.as_ref()? {
        Value::String(text) => serde_json::from_str(text.trim()).ok(),
        other => Some(other.clone()),
    }
    .filter(Value::is_object)
}

/// `yt-dlp` 命令（命令 + 固定参数）。
fn runner(site: &Site) -> Vec<String> {
    let from_ext = ext_object(site)
        .and_then(|v| v.get("ytdlp").and_then(|x| x.as_str()).map(str::to_string));
    let raw = from_ext
        .or_else(|| std::env::var(ENV_RUNNER).ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_RUNNER.to_string());
    raw.split_whitespace().map(str::to_string).collect()
}

/// 运行 `yt-dlp` 并解析 stdout 为 JSON。
fn run_json(site: &Site, args: &[String]) -> Result<Value, CoreError> {
    let parts = runner(site);
    if parts.is_empty() {
        return Err(host_err(site, "csp_Youtube 未配置 yt-dlp 命令"));
    }
    let mut cmd = std::process::Command::new(&parts[0]);
    cmd.args(&parts[1..]).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let output = cmd
        .output()
        .map_err(|e| host_err(site, format!("csp_Youtube 无法启动 yt-dlp（{}）：{e}", parts[0])))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.chars().rev().take(300).collect::<String>().chars().rev().collect();
        return Err(host_err(
            site,
            format!("csp_Youtube yt-dlp 退出码 {:?}：{}", output.status.code(), tail.trim()),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(text.trim())
        .map_err(|e| parse_err(site, format!("csp_Youtube JSON 解析失败: {e}")))
}

/// 从 `entries[]` 归一列表行。
fn entries(value: &Value) -> Vec<VideoItem> {
    let list = value
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| value.as_array().cloned())
        .unwrap_or_default();
    let mut out = Vec::new();
    for row in list {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| row.get("url").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default();
        if id.trim().is_empty() {
            continue;
        }
        let name = row
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let cover = row
            .get("thumbnail")
            .and_then(Value::as_str)
            .filter(|s| s.starts_with("http"))
            .or_else(|| {
                row.get("thumbnails")
                    .and_then(Value::as_array)
                    .and_then(|a| a.last())
                    .and_then(|t| t.get("url"))
                    .and_then(Value::as_str)
                    .filter(|s| s.starts_with("http"))
            })
            .map(str::to_string);
        let remark = row
            .get("uploader")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                row.get("channel")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| {
                row.get("duration")
                    .and_then(Value::as_f64)
                    .map(|d| format_duration(d))
            });
        out.push(VideoItem {
            id,
            name,
            cover,
            remark,
        });
    }
    out
}

fn format_duration(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// 把视频 id 归一为 watch URL。
fn watch_url(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    if id.starts_with("http://") || id.starts_with("https://") {
        return Some(id.to_string());
    }
    Some(format!("https://www.youtube.com/watch?v={id}"))
}

/// 分类：固定「搜索」入口 + ext `class` 追加项。
pub fn categories(_cfg: &TvConfig, site: &Site) -> Result<Vec<Category>, CoreError> {
    let mut out = vec![Category {
        type_id: "JARSEARCH".to_string(),
        type_name: "搜索".to_string(),
    }];
    if let Some(list) = ext_object(site).and_then(|v| v.get("class").cloned()) {
        if let Some(rows) = list.as_array() {
            for row in rows {
                let id = row
                    .get("type_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let name = row
                    .get("type_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if !id.is_empty() && !name.is_empty() {
                    out.push(Category {
                        type_id: id,
                        type_name: name,
                    });
                }
            }
        }
    }
    Ok(out)
}

/// 列表：`JARSEARCH` 返回空；`http` 分类按播放列表展开；其余按关键词搜索。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    cat: &str,
    page: u32,
) -> Result<Vec<VideoItem>, CoreError> {
    let _ = cfg;
    let cat = cat.trim();
    let page = page.max(1) as usize;
    if cat.is_empty() || cat == "JARSEARCH" {
        return Ok(Vec::new());
    }
    let args = if cat.starts_with("http://") || cat.starts_with("https://") {
        vec!["-J".to_string(), "--flat-playlist".to_string(), cat.to_string()]
    } else {
        vec![
            "-J".to_string(),
            "--flat-playlist".to_string(),
            format!("ytsearch{SEARCH_LIMIT}:{cat}"),
        ]
    };
    let value = run_json(site, &args)?;
    let rows = entries(&value);
    let start = (page - 1) * PAGE_SIZE;
    Ok(rows.into_iter().skip(start).take(PAGE_SIZE).collect())
}

/// 详情：单视频元数据 → 单线路单集。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail, CoreError> {
    let _ = cfg;
    let url = watch_url(id).ok_or_else(|| parse_err(site, "csp_Youtube 详情 id 为空"))?;
    let value = run_json(site, &[ "-J".to_string(), "--no-warnings".to_string(), url.clone() ])?;
    let name = value
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() {
        return Err(parse_err(
            site,
            format!("csp_Youtube 详情缺少 title（id={id}）"),
        ));
    }
    let cover = value
        .get("thumbnail")
        .and_then(Value::as_str)
        .filter(|s| s.starts_with("http"))
        .map(str::to_string);
    let desc = value
        .get("description")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let director = value
        .get("uploader")
        .and_then(Value::as_str)
        .map(str::to_string);
    let year = value
        .get("upload_date")
        .and_then(Value::as_str)
        .filter(|s| s.len() >= 4)
        .map(|s| s[..4].to_string());
    let episode = Episode {
        title: name.clone(),
        url: id.trim().to_string(),
    };
    Ok(VideoDetail {
        id: id.trim().to_string(),
        name,
        cover,
        desc,
        year,
        area: None,
        director,
        actor: None,
        episodes: vec![EpisodeGroup {
            group: GROUP.to_string(),
            items: vec![episode],
        }],
    })
}

/// 搜索：`ytsearch{SEARCH_LIMIT}` 扁平列表。
pub fn search(
    cfg: &TvConfig,
    site: &Site,
    keyword: &str,
) -> Result<Vec<VideoItem>, CoreError> {
    let _ = cfg;
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let value = run_json(
        site,
        &[
            "-J".to_string(),
            "--flat-playlist".to_string(),
            format!("ytsearch{SEARCH_LIMIT}:{keyword}"),
        ],
    )?;
    Ok(entries(&value))
}

/// 播放：`-f best -J` 取直链与请求头。
pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
) -> Result<Value, CoreError> {
    let _ = cfg;
    let url = watch_url(id).ok_or_else(|| play_err(site, "csp_Youtube 播放 id 为空"))?;
    let value = run_json(
        site,
        &[
            "-J".to_string(),
            "-f".to_string(),
            "best".to_string(),
            "--no-warnings".to_string(),
            url,
        ],
    )?;
    let picked = pick_format(&value)
        .ok_or_else(|| play_err(site, format!("csp_Youtube 无可播放格式（id={id}）")))?;
    let play_url = picked
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if play_url.is_empty() {
        return Err(play_err(site, format!("csp_Youtube 播放直链为空（id={id}）")));
    }
    let header = picked
        .get("http_headers")
        .and_then(Value::as_object)
        .map(|map| {
            Value::Object(
                map.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), Value::String(s.to_string()))))
                    .collect(),
            )
        })
        .unwrap_or_else(|| json!({}));
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": play_url,
        "header": header,
    }))
}

/// 在 `-J` 输出里挑一个带直链的格式。
fn pick_format(value: &Value) -> Option<&Value> {
    if value.get("url").and_then(Value::as_str).is_some() {
        return Some(value);
    }
    if let Some(list) = value.get("requested_formats").and_then(Value::as_array) {
        if let Some(hit) = list.iter().find(|f| f.get("url").and_then(Value::as_str).is_some()) {
            return Some(hit);
        }
    }
    value
        .get("formats")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .rev()
                .find(|f| f.get("url").and_then(Value::as_str).is_some())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Value) -> Site {
        Site {
            key: "test".to_string(),
            name: "YouTube".to_string(),
            kind: 3,
            api: API.to_string(),
            ext: Some(ext),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Youtube"));
        assert!(handles(" csp_youtube "));
        assert!(!handles("csp_XBPQ"));
    }

    #[test]
    fn categories_include_jar_search_and_ext_class() {
        let s = site(json!({"class":[{"type_id":"PL123","type_name":"我的列表"}]}));
        let cfg = TvConfig::default();
        let cats = categories(&cfg, &s).expect("categories");
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "JARSEARCH");
        assert_eq!(cats[1].type_name, "我的列表");
    }

    #[test]
    fn runner_defaults_to_ytdlp() {
        std::env::remove_var(ENV_RUNNER);
        let s = site(json!({}));
        assert_eq!(runner(&s), vec!["yt-dlp".to_string()]);
        let s2 = site(json!({"ytdlp":"node /tmp/fake.mjs --x"}));
        assert_eq!(runner(&s2)[0], "node");
        assert_eq!(runner(&s2).len(), 3);
    }

    #[test]
    fn entries_parse_flat_playlist() {
        let value = json!({"entries":[
            {"id":"abc","title":"标题一","uploader":"作者","thumbnail":"https://i.test/1.jpg","duration":95.0},
            {"id":"def","title":"标题二"},
            {"title":"缺 id"},
            {"id":"ghi","title":"  "}
        ]});
        let rows = entries(&value);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "abc");
        assert_eq!(rows[0].remark.as_deref(), Some("作者"));
        assert_eq!(rows[1].cover, None);
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(95.0), "1:35");
        assert_eq!(format_duration(3661.0), "1:01:01");
    }

    #[test]
    fn watch_url_normalises_ids() {
        assert_eq!(
            watch_url("abc123").as_deref(),
            Some("https://www.youtube.com/watch?v=abc123")
        );
        assert_eq!(watch_url("").as_deref(), None);
    }
}

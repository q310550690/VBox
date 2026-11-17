//! XYQHiker 原生规则引擎（不执行 Android jar）
//!
//! 许多 `csp_XYQHiker` / `csp_XYQHikerAL` 站点并不包含站点专用代码，而是用 `ext` 里的
//! 一套 HTML 规则描述站点。这里用 Rust 解释这套规则，覆盖"规则型"站点。
//!
//! 规则字段来自真实配置（见参考 `qist/tvbox/XYQHiker/*.json`），核心包括：
//! 分类链接、分类列表数组规则、分类片单标题/链接/图片/副标题、
//! 搜索链接、详情规则、线路列表数组规则、播放列表数组规则、选集标题/链接等。

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::html_rule;
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct XyqRules {
    #[serde(rename = "规则名")]
    pub name: String,
    #[serde(rename = "请求头参数")]
    pub headers: String,
    #[serde(rename = "网页编码格式")]
    pub charset: String,

    #[serde(rename = "是否开启获取首页数据")]
    pub home_enabled: String,
    #[serde(rename = "首页推荐链接")]
    pub home_url: String,
    #[serde(rename = "首页列表数组规则")]
    pub home_list_rule: String,
    #[serde(rename = "首页片单列表数组规则")]
    pub home_item_rule: String,
    #[serde(rename = "首页片单标题")]
    pub home_title: String,
    #[serde(rename = "首页片单链接")]
    pub home_link: String,
    #[serde(rename = "首页片单图片")]
    pub home_pic: String,
    #[serde(rename = "首页片单副标题")]
    pub home_remark: String,
    #[serde(rename = "首页片单是否Jsoup写法")]
    pub home_item_jsoup: String,

    #[serde(rename = "分类起始页码")]
    pub cat_start_page: String,
    #[serde(rename = "分类截取模式")]
    pub cat_mode: String,
    #[serde(rename = "分类Json数据二次截取")]
    pub cat_json_cut: String,
    #[serde(rename = "分类链接")]
    pub cat_url: String,
    #[serde(rename = "分类名称")]
    pub cat_names: String,
    #[serde(rename = "分类名称替换词")]
    pub cat_values: String,
    #[serde(rename = "分类列表数组规则")]
    pub cat_list_rule: String,
    #[serde(rename = "分类片单标题")]
    pub cat_title: String,
    #[serde(rename = "分类片单链接")]
    pub cat_link: String,
    #[serde(rename = "分类片单图片")]
    pub cat_pic: String,
    #[serde(rename = "分类片单副标题")]
    pub cat_remark: String,
    #[serde(rename = "分类片单链接加前缀")]
    pub cat_link_prefix: String,
    #[serde(rename = "分类片单链接加后缀")]
    pub cat_link_suffix: String,
    #[serde(rename = "分类片单是否Jsoup写法")]
    pub cat_item_jsoup: String,

    #[serde(rename = "搜索请求头参数")]
    pub search_headers: String,
    #[serde(rename = "搜索截取模式")]
    pub search_mode: String,
    #[serde(rename = "搜索Json数据二次截取")]
    pub search_json_cut: String,
    #[serde(rename = "搜索链接")]
    pub search_url: String,
    #[serde(rename = "POST请求数据")]
    pub search_post: String,
    #[serde(rename = "搜索列表数组规则")]
    pub search_list_rule: String,
    #[serde(rename = "搜索片单标题")]
    pub search_title: String,
    #[serde(rename = "搜索片单链接")]
    pub search_link: String,
    #[serde(rename = "搜索片单图片")]
    pub search_pic: String,
    #[serde(rename = "搜索片单副标题")]
    pub search_remark: String,
    #[serde(rename = "搜索片单链接加前缀")]
    pub search_link_prefix: String,
    #[serde(rename = "搜索片单链接加后缀")]
    pub search_link_suffix: String,
    #[serde(rename = "搜索片单是否Jsoup写法")]
    pub search_item_jsoup: String,

    #[serde(rename = "链接是否直接播放")]
    pub direct_play: String,
    #[serde(rename = "直接播放链接加前缀")]
    pub direct_prefix: String,
    #[serde(rename = "直接播放链接加后缀")]
    pub direct_suffix: String,
    #[serde(rename = "直接播放直链视频请求头")]
    pub direct_headers: String,

    #[serde(rename = "详情是否Jsoup写法")]
    pub detail_jsoup: String,
    #[serde(rename = "类型详情")]
    pub detail_type: String,
    #[serde(rename = "年代详情")]
    pub detail_year: String,
    #[serde(rename = "地区详情")]
    pub detail_area: String,
    #[serde(rename = "演员详情")]
    pub detail_actor: String,
    #[serde(rename = "简介详情")]
    pub detail_desc: String,
    #[serde(rename = "线路列表数组规则")]
    pub line_list_rule: String,
    #[serde(rename = "线路标题")]
    pub line_title: String,
    #[serde(rename = "播放列表数组规则")]
    pub play_list_rule: String,
    #[serde(rename = "选集列表数组规则")]
    pub ep_list_rule: String,
    #[serde(rename = "选集标题")]
    pub ep_title: String,
    #[serde(rename = "选集链接")]
    pub ep_link: String,
    #[serde(rename = "是否反转选集序列")]
    pub ep_reverse: String,
    #[serde(rename = "选集链接加前缀")]
    pub ep_link_prefix: String,
    #[serde(rename = "选集链接加后缀")]
    pub ep_link_suffix: String,
    #[serde(rename = "选集标题链接是否Jsoup写法")]
    pub ep_jsoup: String,
    #[serde(rename = "分析MacPlayer")]
    pub analyze_mac_player: String,
    #[serde(rename = "是否开启手动嗅探")]
    pub manual_sniff: String,
    #[serde(rename = "手动嗅探视频链接关键词")]
    pub sniff_keywords: String,
    #[serde(rename = "手动嗅探视频链接过滤词")]
    pub sniff_filters: String,
}

pub struct XyqEngine<'a> {
    pub site: &'a Site,
    pub cfg: &'a TvConfig,
    pub rules: XyqRules,
}

impl<'a> XyqEngine<'a> {
    pub fn new(site: &'a Site, cfg: &'a TvConfig) -> Result<Self> {
        let rules = load_rules(site, cfg)?;
        Ok(Self { site, cfg, rules })
    }

    pub fn home(&self) -> Result<(Vec<Category>, Vec<VideoItem>)> {
        let cats = self.categories();
        let mut videos = Vec::new();
        if self.truthy(&self.rules.home_enabled) && !self.rules.home_url.is_empty() {
            if let Ok(html) = self.get(&self.rules.home_url, &self.rules.headers) {
                videos = self.parse_list(
                    &html,
                    &self.rules.home_list_rule,
                    &self.rules.home_item_rule,
                    &self.rules.home_title,
                    &self.rules.home_link,
                    &self.rules.home_pic,
                    &self.rules.home_remark,
                    "",
                    "",
                    self.truthy(&self.rules.home_item_jsoup),
                );
            }
        }
        Ok((cats, videos))
    }

    pub fn categories(&self) -> Vec<Category> {
        let names: Vec<&str> = self.rules.cat_names.split('&').collect();
        let values: Vec<&str> = self.rules.cat_values.split('&').collect();
        let mut out = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let value = values.get(i).map(|s| s.trim()).unwrap_or(name);
            out.push(Category::new(value, name));
        }
        out
    }

    pub fn category(&self, tid: &str, page: u32) -> Result<Vec<VideoItem>> {
        let start: u32 = self.rules.cat_start_page.trim().parse().unwrap_or(1);
        let page = start + page.saturating_sub(1);
        let mut url = self
            .rules
            .cat_url
            .replace("{cateId}", tid)
            .replace("{catePg}", &page.to_string())
            .replace("{by}", "time")
            .replace("{area}", "*")
            .replace("{class}", "*")
            .replace("{year}", "*")
            .replace("{lang}", "*");
        url = strip_first_page_annotation(&url, page);
        let html = self.get(&url, &self.rules.headers)?;
        if self.rules.cat_mode.trim() == "0" {
            return self.parse_json_list(
                &html,
                &self.rules.cat_json_cut,
                &self.rules.cat_list_rule,
                &self.rules.cat_title,
                &self.rules.cat_link,
                &self.rules.cat_pic,
                &self.rules.cat_remark,
                &self.rules.cat_link_prefix,
                &self.rules.cat_link_suffix,
            );
        }
        Ok(self.parse_list(
            &html,
            &self.rules.cat_list_rule,
            "",
            &self.rules.cat_title,
            &self.rules.cat_link,
            &self.rules.cat_pic,
            &self.rules.cat_remark,
            &self.rules.cat_link_prefix,
            &self.rules.cat_link_suffix,
            self.truthy(&self.rules.cat_item_jsoup),
        ))
    }

    pub fn search(&self, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
        if self.rules.search_url.is_empty() {
            return Ok(Vec::new());
        }
        let mut url = self.rules.search_url.replace("{wd}", keyword);
        if !self.rules.search_post.is_empty() {
            url = format!(
                "{}?{}",
                url,
                self.rules.search_post.replace("{wd}", keyword)
            );
        }
        let html = self.get(&url, &self.rules.search_headers)?;
        if self.rules.search_mode.trim() == "0" {
            return self.parse_json_list(
                &html,
                &self.rules.search_json_cut,
                &self.rules.search_list_rule,
                &self.rules.search_title,
                &self.rules.search_link,
                &self.rules.search_pic,
                &self.rules.search_remark,
                &self.rules.search_link_prefix,
                &self.rules.search_link_suffix,
            );
        }
        Ok(self.parse_list(
            &html,
            &self.rules.search_list_rule,
            "",
            &self.rules.search_title,
            &self.rules.search_link,
            &self.rules.search_pic,
            &self.rules.search_remark,
            &self.rules.search_link_prefix,
            &self.rules.search_link_suffix,
            self.truthy(&self.rules.search_item_jsoup),
        ))
    }

    pub fn detail(&self, id: &str) -> Result<VideoDetail> {
        let html = self.get(id, &self.rules.headers)?;
        let detail_jsoup = self.truthy(&self.rules.detail_jsoup);
        let name = self.site.name.clone();
        let desc = self.field_value(&html, &self.rules.detail_desc, detail_jsoup);
        let year = self.field_value(&html, &self.rules.detail_year, detail_jsoup);
        let area = self.field_value(&html, &self.rules.detail_area, detail_jsoup);
        let actor = self.field_value(&html, &self.rules.detail_actor, detail_jsoup);

        let mut groups = Vec::new();
        let line_blocks = if self.rules.line_list_rule.is_empty() {
            vec![html.clone()]
        } else {
            html_rule::pdfa(&html, &self.rules.line_list_rule)
        };
        for block in line_blocks {
            let group_name = html_rule::pdfh(&block, &self.rules.line_title);
            let play_blocks = if self.rules.play_list_rule.is_empty() {
                vec![block.clone()]
            } else {
                html_rule::pdfa(&block, &self.rules.play_list_rule)
            };
            for pb in play_blocks {
                let eps: Vec<Episode> = html_rule::pdfa(&pb, &self.rules.ep_list_rule)
                    .into_iter()
                    .map(|item| {
                        let title = literal_title(&self.rules.ep_title)
                            .unwrap_or_else(|| self.field_value(&item, &self.rules.ep_title, true));
                        let mut url = self.field_link(&item, &self.rules.ep_link, id, true);
                        if !url.starts_with("http") {
                            url = format!(
                                "{}{}{}",
                                self.rules.ep_link_prefix, url, self.rules.ep_link_suffix
                            );
                        }
                        Episode { title, url }
                    })
                    .filter(|e| !e.url.is_empty())
                    .collect();
                if !eps.is_empty() {
                    groups.push(EpisodeGroup {
                        group: if group_name.is_empty() {
                            "默认".into()
                        } else {
                            group_name.clone()
                        },
                        items: eps,
                    });
                }
            }
        }
        if self.truthy(&self.rules.ep_reverse) {
            for group in &mut groups {
                group.items.reverse();
            }
        }
        Ok(VideoDetail {
            id: id.to_string(),
            name: if name.is_empty() { id.into() } else { name },
            desc: (!desc.is_empty()).then_some(desc),
            year: (!year.is_empty()).then_some(year),
            area: (!area.is_empty()).then_some(area),
            actor: (!actor.is_empty()).then_some(actor),
            episodes: groups,
            ..Default::default()
        })
    }

    pub fn player_content(&self, flag: &str, id: &str, _vip: &[String]) -> Result<Value> {
        if self.truthy(&self.rules.direct_play) {
            let url = format!(
                "{}{}{}",
                self.rules.direct_prefix, id, self.rules.direct_suffix
            );
            return Ok(
                serde_json::json!({"url": url, "parse": 0, "header": parse_headers(&self.rules.direct_headers)}),
            );
        }
        // 尝试从播放页提取 MacPlayer / 常见 m3u8；提取不到则交给 VBox 嗅探链。
        let html = self.get(id, &self.rules.headers)?;
        if let Some(url) =
            extract_media_url(&html, &self.rules.sniff_keywords, &self.rules.sniff_filters)
        {
            return Ok(
                serde_json::json!({"url": url, "parse": 0, "header": parse_headers(&self.rules.headers)}),
            );
        }
        Ok(serde_json::json!({
            "url": id,
            "parse": 1,
            "jx": 1,
            "flag": flag,
            "header": parse_headers(&self.rules.headers)
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn parse_list(
        &self,
        html: &str,
        list_rule: &str,
        item_rule: &str,
        title: &str,
        link: &str,
        pic: &str,
        remark: &str,
        prefix: &str,
        suffix: &str,
        jsoup: bool,
    ) -> Vec<VideoItem> {
        let rule = if item_rule.is_empty() {
            list_rule.to_string()
        } else {
            format!("{list_rule}&&{item_rule}")
        };
        let items = if item_rule.is_empty() {
            html_rule::pdfa(html, &rule)
        } else {
            // 先取列表容器，再逐项处理，避免把 item_rule 错误拼在整体上
            html_rule::pdfa(html, list_rule)
                .into_iter()
                .flat_map(|block| html_rule::pdfa(&block, item_rule))
                .collect()
        };
        items
            .into_iter()
            .filter_map(|item| {
                let name = self.field_value(&item, title, jsoup);
                if name.trim().is_empty() {
                    return None;
                }
                let mut url = self.field_link(&item, link, "", jsoup);
                if url.trim().is_empty() {
                    return None;
                }
                if !url.starts_with("http") {
                    url = format!("{prefix}{url}{suffix}");
                }
                let pic_url = self.field_link(&item, pic, "", jsoup);
                let remark_text = self.field_value(&item, remark, jsoup);
                Some(VideoItem {
                    id: url,
                    name: name.trim().to_string(),
                    cover: (!pic_url.is_empty()).then_some(pic_url),
                    remark: (!remark_text.trim().is_empty()).then_some(remark_text),
                })
            })
            .collect()
    }

    /// 规则取值：`*是否Jsoup写法` 为 0 时，`A&&B` 表示"从 A 到 B 之间截取"（XYQHiker/XBPQ 常见），
    /// 否则走 Jsoup 链式选择器。
    fn field_value(&self, html: &str, rule: &str, jsoup: bool) -> String {
        if jsoup || rule.contains(':') || rule.ends_with("Text") || rule.ends_with("Html") {
            return html_rule::pdfh(html, rule);
        }
        html_rule::extract_marker(html, rule).unwrap_or_else(|| html_rule::pdfh(html, rule))
    }

    #[allow(clippy::too_many_arguments)]
    fn parse_json_list(
        &self,
        text: &str,
        cut: &str,
        path: &str,
        title: &str,
        link: &str,
        pic: &str,
        remark: &str,
        prefix: &str,
        suffix: &str,
    ) -> Result<Vec<VideoItem>> {
        let payload = if cut.trim().is_empty() {
            text.trim().to_string()
        } else {
            html_rule::extract_between_raw(text, cut).unwrap_or_else(|| text.trim().to_string())
        };
        let value: Value = serde_json::from_str(&payload)
            .map_err(|e| CoreError::Config(format!("XYQHiker JSON 解析失败: {e}")))?;
        let Some(items) = json_path(&value, path).and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for item in items {
            let name = json_string(item, title);
            if name.is_empty() {
                continue;
            }
            let mut url = json_string(item, link);
            if url.is_empty() {
                continue;
            }
            if !url.starts_with("http") {
                url = format!("{prefix}{url}{suffix}");
            }
            let pic_url = json_string(item, pic);
            let remark_text = json_string(item, remark);
            out.push(VideoItem {
                id: url,
                name,
                cover: (!pic_url.is_empty()).then_some(pic_url),
                remark: (!remark_text.is_empty()).then_some(remark_text),
            });
        }
        Ok(out)
    }

    fn field_link(&self, html: &str, rule: &str, base: &str, jsoup: bool) -> String {
        let raw = self.field_value(html, rule, jsoup);
        let raw = raw.trim();
        if raw.is_empty() {
            return String::new();
        }
        if raw.starts_with("http://") || raw.starts_with("https://") {
            return raw.to_string();
        }
        html_rule::join_url(base, raw)
    }

    fn get(&self, url: &str, header_param: &str) -> Result<String> {
        if url.starts_with("file://") {
            let p = url.trim_start_matches("file://");
            return std::fs::read_to_string(p).map_err(CoreError::Io);
        }
        let client = http::client_for(&self.cfg.http)?;
        let headers = parse_headers(header_param);
        http::get_text(&client, url, &headers, self.cfg.http.retry)
    }

    fn truthy(&self, value: &str) -> bool {
        matches!(value.trim(), "1" | "true" | "yes" | "on")
    }
}

fn load_rules(site: &Site, cfg: &TvConfig) -> Result<XyqRules> {
    let ext = site.ext_string();
    if ext.trim().is_empty() {
        return Err(CoreError::Config(format!(
            "站点「{}」缺少 XYQHiker ext 规则",
            site.name
        )));
    }
    let text = if ext.trim_start().starts_with('{') {
        ext
    } else if ext.starts_with("file://") {
        read_file_url(&ext)?
    } else {
        let client = http::client_for(&cfg.http)?;
        http::get_text(&client, ext.trim(), &HashMap::new(), cfg.http.retry)?
    };
    serde_json::from_str::<XyqRules>(&crate::normalize::normalize(&text))
        .map_err(|e| CoreError::Config(format!("XYQHiker 规则解析失败（{}）: {e}", site.name)))
}

fn read_file_url(url: &str) -> Result<String> {
    if let Ok(parsed) = url::Url::parse(url) {
        if let Ok(path) = parsed.to_file_path() {
            return std::fs::read_to_string(&path).map_err(|e| {
                CoreError::Config(format!(
                    "读取 XYQHiker 规则文件失败（{}）: {e}",
                    path.display()
                ))
            });
        }
    }
    let path = url.trim_start_matches("file://");
    let path = if cfg!(windows) {
        path.trim_start_matches('/')
    } else {
        path
    };
    std::fs::read_to_string(path)
        .map_err(|e| CoreError::Config(format!("读取 XYQHiker 规则文件失败（{path}）: {e}")))
}

/* ============================================================
 * 对外接口（sources::spider 调用）
 * ============================================================ */

pub fn list_categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    Ok(XyqEngine::new(site, cfg)?.home()?.0)
}

pub fn list_videos(cfg: &TvConfig, site: &Site, tid: &str, page: u32) -> Result<Vec<VideoItem>> {
    XyqEngine::new(site, cfg)?.category(tid, page)
}

pub fn get_detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    XyqEngine::new(site, cfg)?.detail(id)
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, quick: bool) -> Result<Vec<VideoItem>> {
    XyqEngine::new(site, cfg)?.search(keyword, quick)
}

pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    flag: &str,
    id: &str,
    vip: &[String],
) -> Result<Value> {
    XyqEngine::new(site, cfg)?.player_content(flag, id, vip)
}

pub fn parse_headers(spec: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let spec = spec.trim();
    if spec.is_empty() {
        return out;
    }
    if spec == "手机" {
        out.insert(
            "User-Agent".into(),
            "Mozilla/5.0 (Linux; Android 13) AppleWebKit/537.36 Chrome/120 Mobile Safari/537.36"
                .into(),
        );
        return out;
    }
    for pair in spec.split('#') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        if let Some((key, value)) = pair.split_once('$') {
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() {
                continue;
            }
            let value = if value == "手机" {
                "Mozilla/5.0 (Linux; Android 13) AppleWebKit/537.36 Chrome/120 Mobile Safari/537.36"
                    .to_string()
            } else {
                value.to_string()
            };
            out.insert(key.to_string(), value);
        }
    }
    out
}

fn literal_title(rule: &str) -> Option<String> {
    let rule = rule.trim();
    for quote in ['\'', '"'] {
        if rule.len() >= 2 && rule.starts_with(quote) && rule.ends_with(quote) {
            return Some(rule[1..rule.len() - 1].to_string());
        }
    }
    None
}

fn json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.split('.').filter(|p| !p.trim().is_empty()) {
        let part = part.trim();
        if let Ok(index) = part.parse::<usize>() {
            current = current.get(index)?;
        } else {
            current = current.get(part)?;
        }
    }
    Some(current)
}

fn json_string(value: &Value, key: &str) -> String {
    let Some(found) = json_path(value, key) else {
        return String::new();
    };
    match found {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn strip_first_page_annotation(url: &str, page: u32) -> String {
    let Some(start) = url.find("[firstPage=") else {
        return url.to_string();
    };
    let Some(end_rel) = url[start..].find(']') else {
        return url.to_string();
    };
    let end = start + end_rel;
    let annotation = &url[start + 11..end];
    let base = format!("{}{}", &url[..start], &url[end + 1..]);
    if page == 1 {
        annotation.to_string()
    } else {
        base
    }
}

fn extract_media_url(html: &str, keywords: &str, filters: &str) -> Option<String> {
    let html = html.replace("\\/", "/");
    let html = html.as_str();
    let keywords: Vec<&str> = keywords
        .split('#')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let filters: Vec<&str> = filters
        .split('#')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let candidates = [
        r#""url"\s*:\s*"([^"]+\.m3u8[^"]*)""#,
        r#"'url'\s*:\s*'([^']+\.m3u8[^']*)'"#,
        r#"https?://[^\s"']+\.m3u8[^\s"']*"#,
        r#"https?://[^\s"']+\.mp4[^\s"']*"#,
    ];
    for pattern in candidates {
        let Ok(re) = regex::Regex::new(pattern) else {
            continue;
        };
        for cap in re.captures_iter(html) {
            let url = cap
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or_else(|| cap.get(0).unwrap().as_str());
            let url = url.replace("\\/", "/");
            if filters.iter().any(|f| url.contains(f)) {
                continue;
            }
            if keywords.is_empty() || keywords.iter().any(|k| url.contains(k)) {
                return Some(url);
            }
            return Some(url);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_header_spec() {
        let h = parse_headers("User-Agent$手机#Cookie$a=1");
        assert!(h["User-Agent"].contains("Android"));
        assert_eq!(h["Cookie"], "a=1");
    }

    #[test]
    fn first_page_annotation() {
        assert_eq!(
            strip_first_page_annotation(
                "http://a/{cateId}/index{catePg}.html[firstPage=http://a/1.html]",
                1
            ),
            "http://a/1.html"
        );
        assert_eq!(
            strip_first_page_annotation("http://a/1/index2.html[firstPage=http://a/1.html]", 2),
            "http://a/1/index2.html"
        );
    }

    #[test]
    fn media_extract() {
        let html = r#"<script>player = {url: "https:\/\/a.com\/x.m3u8"}</script>"#;
        assert_eq!(
            extract_media_url(html, ".m3u8#.mp4", ".html").as_deref(),
            Some("https://a.com/x.m3u8")
        );
    }
}

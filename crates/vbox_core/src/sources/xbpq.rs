//! XBPQ（小白解析）原生规则引擎（实验性）
//!
//! `csp_XBPQ` 的 `ext` 是一套字符串规则（start/end marker、`*>` 通配、`[替换:...]`），
//! 不是 Jsoup 选择器。这里实现其常用字段，覆盖规则型 XBPQ 站点；复杂站点的
//! 特殊拼接语法（`+` 拼接）只做降级处理。

use serde::Deserialize;
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::html_rule;
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct XbpqRules {
    #[serde(rename = "站名")]
    pub name: String,
    #[serde(rename = "请求头")]
    pub headers: String,
    #[serde(rename = "编码")]
    pub charset: String,
    #[serde(rename = "主页url")]
    pub home_url: String,
    #[serde(rename = "分类url")]
    pub cat_url: String,
    #[serde(rename = "分类")]
    pub cat_names: String,
    #[serde(rename = "起始页")]
    pub start_page: String,

    #[serde(rename = "二次截取")]
    pub second_cut: String,
    #[serde(rename = "数组")]
    pub list_blocks: String,
    #[serde(rename = "标题")]
    pub title: String,
    #[serde(rename = "图片")]
    pub pic: String,
    #[serde(rename = "副标题")]
    pub remark: String,
    #[serde(rename = "链接")]
    pub link: String,

    #[serde(rename = "影片年代")]
    pub year: String,
    #[serde(rename = "影片地区")]
    pub area: String,
    #[serde(rename = "影片类型")]
    pub kind: String,
    #[serde(rename = "状态")]
    pub status: String,
    #[serde(rename = "导演")]
    pub director: String,
    #[serde(rename = "主演")]
    pub actor: String,
    #[serde(rename = "简介")]
    pub desc: String,

    #[serde(rename = "线路数组")]
    pub line_blocks: String,
    #[serde(rename = "线路标题")]
    pub line_title: String,
    #[serde(rename = "播放数组")]
    pub play_blocks: String,
    #[serde(rename = "播放列表")]
    pub play_items: String,
    #[serde(rename = "播放标题")]
    pub play_title: String,
    #[serde(rename = "播放链接")]
    pub play_link: String,
    #[serde(rename = "跳转播放链接")]
    pub jump_play: String,

    #[serde(rename = "搜索url")]
    pub search_url: String,
    #[serde(rename = "搜索数组")]
    pub search_blocks: String,
    #[serde(rename = "搜索标题")]
    pub search_title: String,
    #[serde(rename = "搜索图片")]
    pub search_pic: String,
    #[serde(rename = "搜索副标题")]
    pub search_remark: String,
    #[serde(rename = "搜索链接")]
    pub search_link: String,

    #[serde(rename = "直接播放")]
    pub direct_play: String,
    #[serde(rename = "播放请求头")]
    pub play_headers: String,
}

pub struct XbpqEngine<'a> {
    pub site: &'a Site,
    pub cfg: &'a TvConfig,
    pub rules: XbpqRules,
    pub base: String,
}

impl<'a> XbpqEngine<'a> {
    pub fn new(site: &'a Site, cfg: &'a TvConfig) -> Result<Self> {
        let ext = site.ext_string();
        let text = if ext.trim_start().starts_with('{') {
            ext
        } else if ext.starts_with("file://") {
            let path = url::Url::parse(&ext)
                .ok()
                .and_then(|u| u.to_file_path().ok())
                .unwrap_or_else(|| {
                    let raw = ext.trim_start_matches("file://");
                    std::path::PathBuf::from(if cfg!(windows) {
                        raw.trim_start_matches('/')
                    } else {
                        raw
                    })
                });
            std::fs::read_to_string(path).map_err(CoreError::Io)?
        } else {
            let client = http::client_for(&cfg.http)?;
            http::get_text(
                &client,
                ext.trim(),
                &std::collections::HashMap::new(),
                cfg.http.retry,
            )?
        };
        let rules: XbpqRules = serde_json::from_str(&crate::normalize::normalize(&text))
            .map_err(|e| CoreError::Config(format!("XBPQ 规则解析失败（{}）: {e}", site.name)))?;
        let base = if rules.home_url.is_empty() {
            rules
                .cat_url
                .split('/')
                .take(3)
                .collect::<Vec<_>>()
                .join("/")
        } else {
            rules.home_url.clone()
        };
        Ok(Self {
            site,
            cfg,
            rules,
            base,
        })
    }

    pub fn categories(&self) -> Vec<Category> {
        let mut out = Vec::new();
        for item in self.rules.cat_names.split('#') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            let (name, value) = match item.split_once('$') {
                Some((name, value)) => (name.trim(), value.trim()),
                None => (item, item),
            };
            out.push(Category::new(value, name));
        }
        out
    }

    pub fn category(&self, tid: &str, page: u32) -> Result<Vec<VideoItem>> {
        let start: u32 = self.rules.start_page.trim().parse().unwrap_or(1);
        let page = start + page.saturating_sub(1);
        let url = self
            .rules
            .cat_url
            .replace("{cateId}", tid)
            .replace("{catePg}", &page.to_string())
            .replace("{area}", "*")
            .replace("{by}", "time")
            .replace("{class}", "*")
            .replace("{lang}", "*")
            .replace("{letter}", "*")
            .replace("{year}", "*");
        let html = self.get(&url, &self.rules.headers)?;
        Ok(self.parse_list(&html, &self.rules.list_blocks, None))
    }

    pub fn search(&self, keyword: &str) -> Result<Vec<VideoItem>> {
        if self.rules.search_url.is_empty() {
            return Ok(Vec::new());
        }
        let url = self
            .rules
            .search_url
            .replace("{wd}", keyword)
            .replace("{pg}", "1");
        let html = self.get(&url, &self.rules.headers)?;
        Ok(self.parse_list(
            &html,
            &self.rules.search_blocks,
            Some((
                &self.rules.search_title,
                &self.rules.search_link,
                &self.rules.search_pic,
                &self.rules.search_remark,
            )),
        ))
    }

    pub fn detail(&self, id: &str) -> Result<VideoDetail> {
        let html = self.get(id, &self.rules.headers)?;
        let mut groups = Vec::new();
        let line_blocks = if self.rules.line_blocks.is_empty() {
            Vec::new()
        } else {
            html_rule::split_blocks(&html, &self.rules.line_blocks)
        };
        for line in line_blocks {
            let group_name = self.extract(&line, &self.rules.line_title);
            let plays = if !self.rules.play_blocks.is_empty() {
                html_rule::split_blocks(&line, &self.rules.play_blocks)
            } else {
                vec![line.clone()]
            };
            for play in plays {
                let items = if !self.rules.play_items.is_empty() {
                    html_rule::split_blocks(&play, &self.rules.play_items)
                } else {
                    html_rule::pdfa(&play, "a")
                };
                let eps: Vec<Episode> = items
                    .into_iter()
                    .filter_map(|item| {
                        let title = self.extract(&item, &self.rules.play_title);
                        let mut url = self.extract(&item, &self.rules.play_link);
                        if url.is_empty() {
                            return None;
                        }
                        if !url.starts_with("http") {
                            url = html_rule::join_url(id, &url);
                        }
                        Some(Episode { title, url })
                    })
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
        Ok(VideoDetail {
            id: id.to_string(),
            name: self.site.name.clone(),
            year: non_empty(self.extract(&html, &self.rules.year)),
            area: non_empty(self.extract(&html, &self.rules.area)),
            actor: non_empty(self.extract(&html, &self.rules.actor)),
            director: non_empty(self.extract(&html, &self.rules.director)),
            desc: non_empty(self.extract(&html, &self.rules.desc)),
            episodes: groups,
            ..Default::default()
        })
    }

    pub fn player_content(&self, id: &str) -> Result<Value> {
        if self.rules.direct_play.trim() == "1" {
            return Ok(serde_json::json!({"url": id, "parse": 0,
                "header": super::xyq::parse_headers(&self.rules.play_headers)}));
        }
        let html = self.get(id, &self.rules.headers)?;
        if !self.rules.jump_play.is_empty() {
            if let Some(url) = self.extract_opt(&html, &self.rules.jump_play) {
                if url.starts_with("http") {
                    return Ok(serde_json::json!({"url": url, "parse": 0,
                        "header": super::xyq::parse_headers(&self.rules.play_headers)}));
                }
            }
        }
        if let Some(url) = extract_media(&html) {
            return Ok(serde_json::json!({"url": url, "parse": 0,
                "header": super::xyq::parse_headers(&self.rules.play_headers)}));
        }
        Ok(serde_json::json!({"url": id, "parse": 1, "jx": 1,
            "header": super::xyq::parse_headers(&self.rules.play_headers)}))
    }

    fn parse_list(
        &self,
        html: &str,
        block_rule: &str,
        search: Option<(&str, &str, &str, &str)>,
    ) -> Vec<VideoItem> {
        let html = if !self.rules.second_cut.is_empty() {
            html_rule::extract_marker(html, &self.rules.second_cut)
                .unwrap_or_else(|| html.to_string())
        } else {
            html.to_string()
        };
        let blocks = if block_rule.is_empty() {
            html_rule::pdfa(&html, "body&&li")
        } else {
            html_rule::split_blocks(&html, block_rule)
        };
        let (title_rule, link_rule, pic_rule, remark_rule) = search.unwrap_or((
            &self.rules.title,
            &self.rules.link,
            &self.rules.pic,
            &self.rules.remark,
        ));
        blocks
            .into_iter()
            .filter_map(|block| {
                let name = self.extract(&block, title_rule);
                if name.trim().is_empty() {
                    return None;
                }
                let mut url = self.extract(&block, link_rule);
                if url.trim().is_empty() {
                    return None;
                }
                if !url.starts_with("http") {
                    url = html_rule::join_url(&self.base, &url);
                }
                let pic = self.extract(&block, pic_rule);
                let remark = self.extract(&block, remark_rule);
                Some(VideoItem {
                    id: url,
                    name: name.trim().to_string(),
                    cover: non_empty(pic),
                    remark: non_empty(remark),
                })
            })
            .collect()
    }

    fn extract(&self, html: &str, rule: &str) -> String {
        self.extract_opt(html, rule).unwrap_or_default()
    }

    fn extract_opt(&self, html: &str, rule: &str) -> Option<String> {
        if rule.trim().is_empty() {
            return None;
        }
        let (core, modifiers) = split_modifiers(rule);
        let value = if let Some((prefix, suffix)) = core.split_once("*>") {
            // `title=*>&&<`：prefix 后由 * 通配到 suffix，再清洗属性值/标签
            let suffix = suffix.strip_prefix("&&").unwrap_or(suffix);
            let start = html.find(prefix)? + prefix.len();
            let rest = &html[start..];
            let end = rest.find(suffix)?;
            Some(clean_attr_value(&rest[..end]))
        } else {
            html_rule::extract_marker(html, core)
        }?;
        Some(apply_modifiers(value, modifiers))
    }

    fn get(&self, url: &str, header: &str) -> Result<String> {
        let client = http::client_for(&self.cfg.http)?;
        http::get_text(
            &client,
            url,
            &super::xyq::parse_headers(header),
            self.cfg.http.retry,
        )
    }
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn clean_attr_value(raw: &str) -> String {
    let raw = html_rule::strip_tags(raw);
    let raw = raw.trim().trim_end_matches('>').trim();
    if let Some(rest) = raw.strip_prefix('"') {
        return rest.split('"').next().unwrap_or(rest).trim().to_string();
    }
    if let Some(rest) = raw.strip_prefix('\'') {
        return rest.split('\'').next().unwrap_or(rest).trim().to_string();
    }
    raw.split_whitespace().next().unwrap_or("").to_string()
}

fn split_modifiers(rule: &str) -> (&str, Option<&str>) {
    match rule.find('[') {
        Some(pos) => (&rule[..pos], Some(&rule[pos..])),
        None => (rule, None),
    }
}

fn apply_modifiers(mut value: String, modifiers: Option<&str>) -> String {
    let Some(mods) = modifiers else {
        return value;
    };
    if let Some(start) = mods.find("[替换:") {
        let body_start = start + mods[start..].find(':').map(|p| p + 1).unwrap_or(0);
        if let Some(end) = mods[body_start..].find(']') {
            let body = &mods[body_start..body_start + end];
            for pair in body.split('#') {
                if let Some((from, to)) = pair.split_once(">>") {
                    value = value.replace(from, to);
                }
            }
        }
    }
    value
}

fn extract_media(html: &str) -> Option<String> {
    let normalized = html.replace("\\/", "/");
    let re = regex::Regex::new(r#"https?://[^\s"']+\.(?:m3u8|mp4)[^\s"']*"#).ok()?;
    re.find(&normalized).map(|m| m.as_str().to_string())
}

/* ============================================================
 * 对外接口
 * ============================================================ */

pub fn list_categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    Ok(XbpqEngine::new(site, cfg)?.categories())
}

pub fn list_videos(cfg: &TvConfig, site: &Site, tid: &str, page: u32) -> Result<Vec<VideoItem>> {
    XbpqEngine::new(site, cfg)?.category(tid, page)
}

pub fn get_detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    XbpqEngine::new(site, cfg)?.detail(id)
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    XbpqEngine::new(site, cfg)?.search(keyword)
}

pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
    _vip: &[String],
) -> Result<Value> {
    XbpqEngine::new(site, cfg)?.player_content(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site(ext: Value) -> Site {
        Site {
            key: "x".into(),
            name: "X".into(),
            kind: 3,
            api: "csp_XBPQ".into(),
            ext: Some(Value::String(ext.to_string())),
            ..Default::default()
        }
    }

    #[test]
    fn categories_and_markers() {
        let cfg = TvConfig::default();
        let s = site(json!({
            "主页url": "https://example.com",
            "分类": "电影$1#电视剧$2",
            "分类url": "https://example.com/list/{cateId}-{catePg}.html"
        }));
        let e = XbpqEngine::new(&s, &cfg).unwrap();
        assert_eq!(e.categories()[0].type_name, "电影");
        assert_eq!(
            e.extract(r#"<a title="影片A" href="/d/1">x</a>"#, "title=*>&&<")
                .as_str(),
            "影片A"
        );
        assert_eq!(
            e.extract(r#"<a href="/d/1">x</a>"#, r#"href="&&""#)
                .as_str(),
            "/d/1"
        );
        assert_eq!(
            apply_modifiers("vodplay/1-1.html".into(), Some("[替换:vodplay>>voddetail]")),
            "voddetail/1-1.html"
        );
    }
}

//! 临时订阅实测探针（仅用于订阅可播性统计，不属于正式构建）
//!
//! 用法：
//!   cargo run --example subprobe -- --config <url> [--stats-only] [--out out.jsonl]
//!        [--max-sites N] [--type 3] [--dedupe]
//!
//! 每条输出一行 JSON：统计行 `{"kind":"stats",...}`，站点行 `{"kind":"site",...}`。

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::time::Instant;

use serde_json::{json, Value};
use vbox_core::model::Site;
use vbox_core::probe::{probe_media_url, ProbeOutcome};
use vbox_core::source::get_play;
use vbox_core::{get_detail, list_categories, list_videos, load_config};

fn arg_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == name {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

fn has_flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

fn short(s: &str, n: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= n {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn ext_key(site: &Site) -> String {
    let ext = site.ext.as_ref().map(|v| v.to_string()).unwrap_or_default();
    let mut h = DefaultHasher::new();
    ext.hash(&mut h);
    format!("{}|{}|{}|{:x}", site.kind, site.api, site.jar, h.finish())
}

fn impl_key(site: &Site) -> String {
    format!("{}|{}|{}", site.kind, site.api, site.jar)
}

fn test_site(cfg: &vbox_core::model::TvConfig, site: &Site) -> Value {
    let mut rec = json!({
        "kind": "site",
        "key": site.key,
        "name": site.name,
        "type": site.kind,
        "api": short(&site.api, 140),
        "jar": short(&site.jar, 160),
        "cats_ok": false,
        "cats_n": 0,
        "list_ok": false,
        "list_n": 0,
        "detail_ok": false,
        "play_ok": false,
        "play_url": null,
        "sniff": false,
        "probe": null,
        "stage": "",
        "err": null,
        "ms": 0
    });
    let started = Instant::now();
    let cats = match list_categories(cfg, &site.key) {
        Ok(c) if !c.is_empty() => {
            rec["cats_ok"] = json!(true);
            rec["cats_n"] = json!(c.len());
            c
        }
        Ok(_) => {
            rec["stage"] = json!("categories");
            rec["err"] = json!("empty categories");
            rec["ms"] = json!(started.elapsed().as_millis());
            return rec;
        }
        Err(e) => {
            rec["stage"] = json!("categories");
            rec["err"] = json!(short(&e.to_string(), 240));
            rec["ms"] = json!(started.elapsed().as_millis());
            return rec;
        }
    };

    let mut items: Vec<vbox_core::model::VideoItem> = Vec::new();
    for cat in cats.iter().take(3) {
        if let Ok(v) = list_videos(cfg, &site.key, &cat.type_id, 1) {
            if !v.is_empty() {
                items = v;
                break;
            }
        }
    }
    if items.is_empty() {
        rec["stage"] = json!("list");
        rec["err"] = json!("no items in first categories");
        rec["ms"] = json!(started.elapsed().as_millis());
        return rec;
    }
    rec["list_ok"] = json!(true);
    rec["list_n"] = json!(items.len());

    let item = &items[0];
    let detail = match get_detail(cfg, &site.key, &item.id) {
        Ok(d) => d,
        Err(e) => {
            rec["stage"] = json!("detail");
            rec["err"] = json!(short(&e.to_string(), 240));
            rec["ms"] = json!(started.elapsed().as_millis());
            return rec;
        }
    };
    rec["detail_ok"] = json!(true);
    rec["detail_name"] = json!(detail.name);

    let mut first_ep: Option<(String, String)> = None;
    for group in &detail.episodes {
        if let Some(it) = group.items.first() {
            if !it.url.trim().is_empty() {
                first_ep = Some((group.group.clone(), it.url.clone()));
                break;
            }
        }
    }
    let (group, ep_url) = match first_ep {
        Some(v) => v,
        None => {
            rec["stage"] = json!("episodes");
            rec["err"] = json!("no playable episode");
            rec["ms"] = json!(started.elapsed().as_millis());
            return rec;
        }
    };

    match get_play(cfg, &site.key, &group, &ep_url, &ep_url, None) {
        Ok(play) => {
            rec["play_ok"] = json!(true);
            rec["play_url"] = json!(short(&play.url, 320));
            rec["sniff"] = json!(play.sniff);
            if !play.sniff && play.url.starts_with("http") {
                let probe = match probe_media_url(&play.url, &play.headers) {
                    ProbeOutcome::Available => "available".to_string(),
                    ProbeOutcome::Unavailable(r) => format!("unavailable: {}", short(&r, 160)),
                    ProbeOutcome::Inconclusive(r) => format!("inconclusive: {}", short(&r, 160)),
                };
                rec["probe"] = json!(probe);
            }
        }
        Err(e) => {
            rec["stage"] = json!("play");
            rec["err"] = json!(short(&e.to_string(), 240));
        }
    }
    rec["ms"] = json!(started.elapsed().as_millis());
    rec
}

fn main() {
    let url = match arg_value("--config") {
        Some(v) => v,
        None => {
            eprintln!("usage: subprobe --config <url> [--stats-only] [--out file] [--max-sites N] [--type N] [--dedupe]");
            std::process::exit(2);
        }
    };
    let stats_only = has_flag("--stats-only");
    let dedupe = has_flag("--dedupe");
    let dedupe_impl = has_flag("--dedupe-impl");
    let max_sites: usize = arg_value("--max-sites")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let only_type: Option<i32> = arg_value("--type").and_then(|s| s.parse().ok());

    let mut sink: Box<dyn Write> = match arg_value("--out") {
        Some(p) => match File::create(&p) {
            Ok(f) => Box::new(BufWriter::new(f)),
            Err(e) => {
                eprintln!("cannot create {p}: {e}");
                std::process::exit(2);
            }
        },
        None => Box::new(std::io::stdout()),
    };

    let cfg = match load_config(&url) {
        Ok(c) => c,
        Err(e) => {
            let line = json!({"kind": "fatal", "config": url, "err": short(&e.to_string(), 400)});
            let _ = writeln!(sink, "{line}");
            let _ = sink.flush();
            return;
        }
    };

    let mut by_kind: BTreeMap<i32, usize> = BTreeMap::new();
    let mut jar_sites = 0usize;
    let mut js_sites = 0usize;
    let mut py_sites = 0usize;
    let mut rule_sites = 0usize;
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for s in &cfg.sites {
        *by_kind.entry(s.kind).or_insert(0) += 1;
        if s.kind == 3 {
            if !s.jar.trim().is_empty() {
                jar_sites += 1;
            } else if s.api.contains(".js") || s.api.contains(".mjs") {
                js_sites += 1;
            } else if s.api.ends_with(".py") {
                py_sites += 1;
            } else {
                rule_sites += 1;
            }
        }
        if dedupe {
            let k = if dedupe_impl { impl_key(s) } else { ext_key(s) };
            *groups.entry(k).or_insert(0) += 1;
        }
    }

    let mut picked: Vec<&Site> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for s in &cfg.sites {
        if let Some(t) = only_type {
            if s.kind != t {
                continue;
            }
        }
        let dk = if dedupe_impl { impl_key(s) } else { ext_key(s) };
        if dedupe && !seen.insert(dk) {
            continue;
        }
        picked.push(s);
        if max_sites > 0 && picked.len() >= max_sites {
            break;
        }
    }

    let stats = json!({
        "kind": "stats",
        "config": url,
        "sites_total": cfg.sites.len(),
        "by_kind": by_kind,
        "type3_with_jar": jar_sites,
        "type3_js": js_sites,
        "type3_python": py_sites,
        "type3_rule_or_other": rule_sites,
        "parses": cfg.parses.len(),
        "dedupe_groups": groups.len(),
        "picked": picked.len(),
    });
    let _ = writeln!(sink, "{stats}");
    let _ = sink.flush();
    if stats_only {
        return;
    }

    for s in picked {
        let key = if dedupe_impl { impl_key(s) } else { ext_key(s) };
        let covers = groups.get(&key).copied().unwrap_or(1);
        let mut rec = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test_site(&cfg, s)))
            .unwrap_or_else(|_| json!({"kind": "site", "key": s.key, "panic": true}));
        if dedupe {
            rec["covers"] = json!(covers);
        }
        let _ = writeln!(sink, "{rec}");
        let _ = sink.flush();
    }
}

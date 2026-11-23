//! 判据 69：每个 `blockedReason` 都必须有源码级证据，且跨 jar 的同名类分歧必须登记。
//!
//! 判据 37 只校验「画像 → 归因矩阵」的文字一致，判据 68 只校验「阻断原因是否进入运行时
//! 错误消息」——两者都不检查**阻断理由本身是否被反编译源码支持**。如果某天把
//! `site_dead` 误写成 `engine_dependency`，或者引擎类换成纯 HTTP 实现，画像与报告会
//! 照样自洽。本轮同时发现一个真实的数据质量事件：`com.github.catvod.spider.Youtube`
//! 在 3 个 jar 里有**语义完全不同的同名变体**（2 个 43 行的静态 helper、1 个 1756 行的
//! 完整 Spider），而画像把它们并成了同一个 api。本判据把这类「跨 jar 同名类分歧」
//! 变成必须登记的白名单，新增分歧会直接红。
//!
//! `_rea/` 缺失时按 CI 约定软跳过。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn decompiled_root() -> PathBuf {
    repo_root().join("_rea/decompiled")
}

fn collect_java(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_java(&path, out);
        } else if path.extension().is_some_and(|e| e == "java") {
            out.push(path);
        }
    }
}

fn normalize_jar(name: &str) -> String {
    let stem = name
        .trim_end_matches(".jar")
        .trim_end_matches(".png")
        .trim_end_matches(".jpg")
        .trim_end_matches(".zip");
    let mut out = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out
}

/// (jar 目录, 简单类名) → 反编译源码。
fn evidence_index() -> BTreeMap<(String, String), PathBuf> {
    let root = decompiled_root();
    let mut files = Vec::new();
    collect_java(&root, &mut files);
    let mut out: BTreeMap<(String, String), PathBuf> = BTreeMap::new();
    for f in files {
        let normalized = f.to_string_lossy().replace('\\', "/");
        let Some(pos) = normalized.find("/com/github/catvod/spider/") else {
            continue;
        };
        let tail = &normalized[pos + "/com/github/catvod/spider/".len()..];
        if !tail.ends_with(".java") || tail.contains('/') {
            continue;
        }
        // 路径形如 _rea/decompiled/<jar>/deobf/com/...，jar 目录是根之后的第一个分量。
        let after_root = normalized
            .split("/_rea/decompiled/")
            .nth(1)
            .unwrap_or_default();
        let jar = after_root.split('/').next().unwrap_or_default().to_string();
        let simple = tail.trim_end_matches(".java").to_string();
        // 同一 jar 常有 deobf/ 与 sources/ 两份；deobf 是二次反混淆产物，优先。
        let key = (jar, simple);
        match out.get(&key) {
            Some(existing) if normalized.contains("/deobf/") => {
                let existing = existing.to_string_lossy();
                if existing.contains("/sources/") {
                    out.insert(key, f);
                }
            }
            Some(_) => {}
            None => {
                out.insert(key, f);
            }
        }
    }
    out
}

fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// 从类声明里提取基类名（兼容泛型/多空格）；没有 extends 返回 None。
fn base_class(text: &str, simple: &str) -> Option<String> {
    let needle = format!("class {simple}");
    let start = text.find(&needle)?;
    let tail = &text[start + needle.len()..];
    let head = &tail[..tail.find('{').unwrap_or(tail.len())];
    let pos = head.find("extends")?;
    let word = head[pos + "extends".len()..]
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == '<' || c == '{')
        .next()
        .unwrap_or_default();
    if word.is_empty() {
        None
    } else {
        Some(word.to_string())
    }
}

/// Spider 或任一宿主引擎基类（这些基类最终都继承 Spider）。
fn extends_spider(text: &str, simple: &str) -> bool {
    let engines = [
        "Spider", "Push", "Pan", "Ali", "PikPak", "Thunder", "Samba", "YD", "Quark", "UC", "P115",
        "Wogg", "Huban", "XPathMacFilter", "XYQHiker",
    ];
    match base_class(text, simple) {
        Some(base) => engines.contains(&base.as_str()),
        None => false,
    }
}

fn is_engine_base(text: &str) -> bool {
    [
        "Push", "Pan", "PikPak", "Thunder", "Samba", "YD", "Ali", "Quark", "UC", "P115",
    ]
    .iter()
    .any(|base| text.contains(&format!("extends {base} ")) || text.contains(&format!("extends {base}\n")))
}

fn reason_has_evidence(reason: &str, api: &str, text: &str, recheck_doc: &str) -> bool {
    match reason {
        // 引擎依赖：要么直接引用宿主 parser/merge（或 jar 内 merge）支持类，要么继承
        // 宿主引擎基类，要么依赖 Android 存储（PushAgent）。
        "engine_dependency" => {
            text.contains("com.github.catvod.parser.merge")
                || text.contains("com.github.catvod.spider.merge")
                || text.contains("SharedPreferences")
                || text.contains("android.os.Environment")
                || is_engine_base(text)
        }
        // 反编译缺口：jadx 明确留下了跳过的证据。
        "decompiler_gap" => {
            text.contains("Method dump skipped")
                || text.contains("Method not decompiled")
                || text.contains("Code decompiled incorrectly")
        }
        // WebView 依赖：必须出现 WebView / onLoadResource 或 android.webkit 导入。
        "webview_dependency" => {
            text.contains("android.webkit") || text.contains("WebView") || text.contains("onLoadResource")
        }
        // 站点失效：必须出现在阻断复核报告的表格行里。
        "site_dead" => recheck_doc.contains(&format!("`{api}`")),
        // 播放解析器聚合：依赖宿主规则表 / merge 支持类。
        "player_resolver_aggregator" => {
            text.contains("com.github.catvod.spider.merge") || text.contains("com.github.catvod.parser.merge")
        }
        _ => false,
    }
}

fn profiles() -> serde_json::Value {
    serde_json::from_str(include_str!("../src/spider/catvod/profiles.json"))
        .expect("profiles.json 必须是合法 JSON")
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn blocked_reasons_are_backed_by_declared_variant_evidence() {
    let root = decompiled_root();
    if !root.is_dir() {
        eprintln!("跳过判据 69：_rea/decompiled 不存在（CI 无逆向产物）");
        return;
    }
    let index = evidence_index();
    let profiles = profiles();
    let recheck = read_text(&repo_root().join("docs/审计报告/阻断复核.md"));
    let apis = profiles["apis"].as_object().expect("apis 必须是对象");

    let mut checked = 0usize;
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing_files = Vec::new();
    let mut unevidenced = Vec::new();

    for (api, entry) in apis {
        let Some(reason) = entry["blockedReason"].as_str() else {
            continue;
        };
        let simple = entry["class"]
            .as_str()
            .unwrap_or_default()
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_string();
        let jars = entry["jars"].as_array().cloned().unwrap_or_default();
        let mut variants = Vec::new();
        for jar in jars {
            let jar = normalize_jar(jar.as_str().unwrap_or_default());
            if let Some(path) = index.get(&(jar.clone(), simple.clone())) {
                variants.push((jar, read_text(path)));
            }
        }
        if variants.is_empty() {
            missing_files.push(format!("{api} ({simple})"));
            continue;
        }
        checked += 1;
        *reasons.entry(reason.to_string()).or_default() += 1;
        let ok = variants
            .iter()
            .any(|(_, text)| reason_has_evidence(reason, api, text, &recheck));
        if !ok {
            let detail = variants
                .iter()
                .map(|(jar, text)| format!("{jar}:{}行", text.lines().count()))
                .collect::<Vec<_>>()
                .join(", ");
            unevidenced.push(format!("{api} [{reason}]（{detail}）"));
        }
    }

    assert!(
        missing_files.is_empty(),
        "以下 unported api 在其声明的 jar 中找不到反编译源码：{missing_files:?}"
    );
    assert!(
        unevidenced.is_empty(),
        "以下 api 的 blockedReason 在其声明的任何一个 jar 变体里都没有源码证据：{unevidenced:?}"
    );
    assert_eq!(checked, 5, "unported api 数量应为 5，实际 {checked}");
    assert_eq!(
        reasons,
        BTreeMap::from([
            ("engine_dependency".to_string(), 1),
            ("site_dead".to_string(), 3),
            ("webview_dependency".to_string(), 1),
        ]),
        "unported 阻断原因分布应保持 1/3/1（decompiler_gap 已于第 258 轮清零，csp_ThunderShare 第 260 轮、csp_SambaShare 第 261 轮、csp_Youtube 第 262 轮、csp_PushShare 第 263 轮、csp_Panyq 第 264 轮原生落地）"
    );
    println!("判据 69：{checked} 个 unported api 的阻断原因均有源码证据；原因分布 {reasons:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn cross_jar_non_spider_collisions_are_documented() {
    let root = decompiled_root();
    if !root.is_dir() {
        eprintln!("跳过判据 69：_rea/decompiled 不存在（CI 无逆向产物）");
        return;
    }
    let index = evidence_index();
    let profiles = profiles();
    let apis = profiles["apis"].as_object().expect("apis 必须是对象");

    // 只检查画像声明的 jar 变体；同一个类名在某个 jar 里是完整 Spider、在另一个 jar
    // 里却不是 Spider（静态 helper），就构成「跨 jar 同名类分歧」。
    let mut collisions: BTreeMap<String, Vec<(String, bool, usize)>> = BTreeMap::new();
    for entry in apis.values() {
        let simple = entry["class"]
            .as_str()
            .unwrap_or_default()
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_string();
        if simple.is_empty() {
            continue;
        }
        let jars = entry["jars"].as_array().cloned().unwrap_or_default();
        let mut variants = Vec::new();
        for jar in jars {
            let jar = normalize_jar(jar.as_str().unwrap_or_default());
            if let Some(path) = index.get(&(jar.clone(), simple.clone())) {
                let text = read_text(path);
                variants.push((jar, extends_spider(&text, &simple), text.lines().count()));
            }
        }
        if variants.len() < 2 {
            continue;
        }
        let spider = variants.iter().filter(|(_, is_spider, _)| *is_spider).count();
        if spider > 0 && spider < variants.len() {
            collisions.insert(simple, variants);
        }
    }

    let names: BTreeSet<String> = collisions.keys().cloned().collect();
    let expected: BTreeSet<String> = ["Youtube".to_string()].into_iter().collect();
    assert_eq!(
        names, expected,
        "跨 jar 同名类「部分是 Spider、部分不是」的分歧集合与登记白名单不一致：{collisions:?}"
    );

    let youtube = &collisions["Youtube"];
    println!("判据 69：跨 jar 同名类分歧已登记 — Youtube 变体 {youtube:?}");
    assert!(
        youtube.iter().any(|(_, is_spider, _)| *is_spider)
            && youtube.iter().any(|(_, is_spider, _)| !*is_spider),
        "Youtube 必须同时保留 Spider 与非 Spider 变体，否则白名单该删除"
    );
}

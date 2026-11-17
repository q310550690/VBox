//! 判据 63：画像里的 `class` 必须有真实反编译证据，且证据必须来自画像声明的 jar。
//!
//! 目标文档要求「每个家族带 REA 反编译证据」，但此前所有判据都只核对
//! `profiles.json` 与 dex 摘要（类数/方法数/字节），**没有一个判据证明
//! `_rea/decompiled/` 里真的存在对应的 `.java` 源码**。如果反编译产物被清理、
//! 或者 `class` 写成拼写相近的另一个类，画像与文档会照样自洽，而证据链断裂。
//!
//! 本判据做两件事：
//! 1. 每个带 `class` 的 api，都能在 `_rea/decompiled/**/com/github/catvod/spider/`
//!    下找到同名 `.java`；找不到的必须恰好是「类不在已下载 jar 中」的 14 个哨兵
//!    （`strategy=unsupported`、`jars=[]`、`reason=class_not_in_downloaded_jars`）。
//! 2. 找到的源码必须来自该 api 画像 `jars` 声明的 jar 目录——防止「同名类碰巧在
//!    别的 jar 里」让证据张冠李戴。
//!
//! `_rea/` 缺失时按 CI 约定软跳过（仓库不提交逆向产物）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use vbox_core::spider::catvod::registry;

fn decompiled_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../_rea/decompiled")
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

/// `class` 的简单名 → 证据文件（可多个 jar 重复提供同一类）。
fn evidence_by_class() -> BTreeMap<String, Vec<PathBuf>> {
    let root = decompiled_root();
    let mut files = Vec::new();
    collect_java(&root, &mut files);
    let mut map: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for f in files {
        let normalized = f.to_string_lossy().replace('\\', "/");
        let Some(pos) = normalized.find("/com/github/catvod/spider/") else {
            continue;
        };
        let tail = &normalized[pos + "/com/github/catvod/spider/".len()..];
        if !tail.ends_with(".java") || tail.contains('/') {
            continue;
        }
        map.entry(tail.trim_end_matches(".java").to_string())
            .or_default()
            .push(f);
    }
    map
}

/// 与 `_rea` 下载/解压脚本一致的目录名归一化：去扩展名，非
/// `[A-Za-z0-9_-]` 字符折叠成单个 `_`（非 ASCII 名字在磁盘上被替换）。
fn normalize_jar(name: &str) -> String {
    let stem = name
        .trim_end_matches(".jar")
        .trim_end_matches(".png")
        .trim_end_matches(".jpg")
        .trim_end_matches(".zip");
    let mut out = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out
}

fn jar_dir_of(path: &Path) -> Option<String> {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let root = decompiled_root().to_string_lossy().replace('\\', "/");
    let rest = normalized.strip_prefix(&root)?.trim_start_matches('/');
    rest.split('/').next().map(str::to_string)
}

#[test]
fn every_class_bearing_api_has_decompiled_evidence() {
    let p = registry::profiles();
    if !decompiled_root().is_dir() {
        eprintln!("跳过判据 63：{} 不存在（CI 不提交逆向产物）", decompiled_root().display());
        return;
    }
    let evidence = evidence_by_class();
    assert!(
        evidence.len() >= 150,
        "反编译类证据数量异常偏低（{}），疑似路径/布局变化",
        evidence.len()
    );

    let mut found = 0usize;
    let mut sentinels = BTreeSet::new();
    let mut errors = Vec::new();
    for (api, profile) in &p.apis {
        let Some(class) = profile.class.as_deref().filter(|c| !c.is_empty()) else {
            continue;
        };
        let simple = class.rsplit('.').next().unwrap_or(class);
        let is_sentinel = profile.reason.as_deref() == Some("class_not_in_downloaded_jars");
        if is_sentinel {
            sentinels.insert(api.clone());
            if evidence.contains_key(simple) {
                errors.push(format!(
                    "{api}: 画像称 {simple} 不在已下载 jar 中，但 `_rea/decompiled` 里存在该类源码"
                ));
            }
            if profile.strategy != "unsupported" || !profile.jars.is_empty() {
                errors.push(format!(
                    "{api}: 类缺失哨兵必须 strategy=unsupported 且 jars=[]，实际 strategy={} jars={:?}",
                    profile.strategy, profile.jars
                ));
            }
            continue;
        }
        match evidence.get(simple) {
            Some(files) => {
                found += 1;
                if !files.iter().any(|f| std::fs::metadata(f).map(|m| m.len() > 0).unwrap_or(false)) {
                    errors.push(format!("{api}: {simple} 的证据文件为空"));
                }
            }
            None => errors.push(format!(
                "{api}: 画像 class={class}，但 `_rea/decompiled` 下没有 {simple}.java 证据"
            )),
        }
    }
    assert!(
        errors.is_empty(),
        "画像 class ↔ 反编译证据不一致（{} 项）：\n{}",
        errors.len(),
        errors.join("\n")
    );
    assert!(found >= 140, "有证据的 api 数量偏低：{found}");
    assert_eq!(
        sentinels.iter().filter(|api| !p.apis[*api].jars.is_empty()).count(),
        0,
        "哨兵 api 的 jars 必须为空"
    );
    println!(
        "判据 63：{found} 个 api 有反编译证据；{} 个类缺失哨兵与证据互斥",
        sentinels.len()
    );
}

#[test]
fn decompiled_evidence_comes_from_declared_jars() {
    let p = registry::profiles();
    if !decompiled_root().is_dir() {
        eprintln!("跳过判据 63：反编译目录不存在");
        return;
    }
    let evidence = evidence_by_class();
    let mut checked = 0usize;
    let mut errors = Vec::new();
    for (api, profile) in &p.apis {
        let Some(class) = profile.class.as_deref().filter(|c| !c.is_empty()) else {
            continue;
        };
        if profile.reason.as_deref() == Some("class_not_in_downloaded_jars") {
            continue;
        }
        let simple = class.rsplit('.').next().unwrap_or(class);
        let Some(files) = evidence.get(simple) else {
            continue;
        };
        let declared: BTreeSet<String> = profile.jars.iter().map(|j| normalize_jar(j)).collect();
        let from_declared = files.iter().any(|f| {
            jar_dir_of(f)
                .map(|d| declared.contains(&d))
                .unwrap_or(false)
        });
        if !from_declared {
            let dirs: Vec<_> = files.iter().filter_map(|f| jar_dir_of(f)).collect();
            errors.push(format!(
                "{api}: {simple} 的源码来自 {dirs:?}，不在画像声明的 jar {declared:?} 中"
            ));
        }
        checked += 1;
    }
    assert!(errors.is_empty(), "类证据与画像 jar 归属不一致：\n{}", errors.join("\n"));
    assert!(checked >= 140, "逐 jar 核对的有证据 api 数量偏低：{checked}");
    println!("判据 63：{checked} 个 api 的类证据全部来自画像声明的 jar 目录");
}

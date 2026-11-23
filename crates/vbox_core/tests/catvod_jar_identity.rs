//! 判据 59：jar 台账的「名字即身份」与守卫语义必须可从分析产物逐项重算。
//!
//! 背景：`profiles.jars[].name` 不是随手起的文件名，而是
//! `<md5 前 8 位>-<dexinfo 文件名>` 的规范名（下载去重、按名引用、审计台账都依赖它）。
//! `guarded` 也不是人工标记：它由 dexinfo 的 entry 名（`ftyguard` 等）或 spider 类名
//! 是否以 `Guard` 结尾推导。两者一旦被手改，画像仍自洽，但「受保护 jar → 35 个
//! Guard 壳 api 不移植」的归因就失去依据。本判据从 dexinfo 重算这两件事。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/vbox_core 应位于仓库两级目录下")
        .to_path_buf()
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn load(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} 不是合法 JSON: {e}", path.display())))
}

fn is_hex(s: &str) -> bool {
    s.len() == 32 || s.chars().all(|c| c.is_ascii_hexdigit())
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn jar_names_encode_md5_prefix_and_file_basename() {
    let root = repo_root();
    let profiles = load(&root.join("crates/vbox_core/src/spider/catvod/profiles.json"))
        .expect("profiles.json 必须存在");
    let Some(dexinfo) = load(&root.join("_rea/samples/library/dexinfo.json")) else {
        eprintln!("_rea 不在当前检出，跳过判据 59（CI 软跳过）");
        return;
    };

    let dex_by_sha: BTreeMap<&str, &serde_json::Value> = dexinfo["jars"]
        .as_array()
        .expect("dexinfo.jars 应为数组")
        .iter()
        .filter_map(|j| j["sha256"].as_str().map(|sha| (sha, j)))
        .collect();

    let jars = profiles["jars"].as_array().expect("profiles.jars 应为数组");
    assert!(jars.len() >= 20, "jar 台账只有 {} 条，疑似被截断", jars.len());
    let mut failures: Vec<String> = Vec::new();
    for jar in jars {
        let name = jar["name"].as_str().expect("jar.name 应为字符串");
        let sha = jar["sha256"].as_str().unwrap_or_default();
        let Some(dex) = dex_by_sha.get(sha) else {
            failures.push(format!("{name}: sha256 {sha:?} 在 dexinfo 中找不到"));
            continue;
        };
        let md5 = dex["md5"].as_str().unwrap_or_default();
        let file = dex["file"].as_str().unwrap_or_default();
        if file.is_empty() || name != basename(file) {
            failures.push(format!(
                "{name}: 名字应为 dexinfo 文件名 basename，实际 file={file:?}"
            ));
        }
        let Some((prefix, rest)) = name.split_once('-') else {
            failures.push(format!("{name}: 规范名应含 `-` 分隔的 md5 前缀"));
            continue;
        };
        if rest.is_empty() {
            failures.push(format!("{name}: `-` 后缺少文件名"));
        }
        if md5.len() != 32 || !is_hex(md5) || prefix != &md5[..8] {
            failures.push(format!(
                "{name}: 前缀 {prefix:?} 应为 md5 前 8 位（md5={md5:?}）"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "jar 规范名与 dexinfo 不闭合（{} 处）：\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!("判据 59：{} 个 jar 的规范名均由 md5 前缀 + dexinfo basename 构成", jars.len());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn jar_guard_flag_matches_guard_entries() {
    let root = repo_root();
    let profiles = load(&root.join("crates/vbox_core/src/spider/catvod/profiles.json"))
        .expect("profiles.json 必须存在");
    let Some(dexinfo) = load(&root.join("_rea/samples/library/dexinfo.json")) else {
        eprintln!("_rea 不在当前检出，跳过判据 59（CI 软跳过）");
        return;
    };
    let dex_by_sha: BTreeMap<&str, &serde_json::Value> = dexinfo["jars"]
        .as_array()
        .expect("dexinfo.jars 应为数组")
        .iter()
        .filter_map(|j| j["sha256"].as_str().map(|sha| (sha, j)))
        .collect();

    let mut failures: Vec<String> = Vec::new();
    let mut guarded_count = 0usize;
    for jar in profiles["jars"].as_array().expect("profiles.jars 应为数组") {
        let name = jar["name"].as_str().unwrap_or("<no-name>");
        let declared = jar["guarded"].as_bool().expect("jar.guarded 应为布尔");
        let Some(dex) = jar["sha256"].as_str().and_then(|sha| dex_by_sha.get(sha)) else {
            failures.push(format!("{name}: sha256 在 dexinfo 中找不到"));
            continue;
        };
        let entry_guard = dex["entries"].as_array().map_or(false, |entries| {
            entries
                .iter()
                .filter_map(|e| e.as_str())
                .any(|e| e.to_ascii_lowercase().contains("guard"))
        });
        let class_guard = dex["dexes"].as_array().map_or(false, |dexes| {
            dexes.iter().any(|dx| {
                dx["spiderClasses"].as_array().map_or(false, |classes| {
                    classes.iter().any(|c| {
                        c["name"]
                            .as_str()
                            .map_or(false, |n| n.ends_with("Guard"))
                    })
                })
            })
        });
        let derived = entry_guard || class_guard;
        if derived != declared {
            failures.push(format!(
                "{name}: guarded={declared}，但按 entries/spiderClasses 重算为 {derived}"
            ));
        }
        if declared {
            guarded_count += 1;
        }
    }
    assert!(guarded_count >= 1, "台账中至少应有一个受保护 jar");
    let total = profiles["jars"].as_array().map_or(0, Vec::len);
    assert!(
        guarded_count < total,
        "全部 jar 都被标为 guarded，语义可疑"
    );
    assert!(
        failures.is_empty(),
        "guarded 标记无法由分析产物重算（{} 处）：\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!(
        "判据 59：{total} 个 jar 的 guarded 标记全部可由 entries/spiderClasses 重算（{guarded_count} 个受保护）"
    );
}

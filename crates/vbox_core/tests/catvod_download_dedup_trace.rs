//! 判据 71：去重台账的「每条下载来源都可复算」与画像 jar 名的来源可追溯。
//!
//! 判据 65 已锁「canonical 记录 ↔ dexinfo ↔ 画像」的 hash 一致，判据 59 锁
//! 「画像 jar 名 ↔ 分析文件 basename」；但 28 条 `from` 下载来源本身从未被重新读取。
//! 如果某个 `dup` 来源指向的文件被替换、或 `from` 只剩一个约名字符串，
//! dexinfo/画像仍然自洽，**下载→去重**这一跳却断了。
//!
//! 本判据断言：
//! - 28 条记录（21 canonical + 7 dup）的 `from` 文件全部存在，长度与 md5 等于其 canonical 记录；
//! - 28 条 `from` 互不重复，21 个 canonical `file` 互不重复；
//! - 画像 20 个 jar 的 `sha256` 都能在 canonical 记录中找到；
//! - 画像 jar 名可由「下载来源 basename」推导：要么同名，要么加 `<md5 前 8 位>-` 前缀；
//! - 下载根必须恰好是三个订阅根（4k / GYCK / dc），证明全部来源都被登记。
//!
//! `_rea/` 缺失时 CI 软跳过。

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("仓库根目录")
        .to_path_buf()
}

fn base_name(path: &str) -> String {
    path.replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

fn norm(path: &str) -> String {
    path.replace('\\', "/")
}

fn jar_root(path: &str) -> String {
    let normalized = norm(path);
    let parts: Vec<&str> = normalized.split('/').collect();
    let idx = parts
        .iter()
        .rposition(|part| *part == "jars")
        .expect("下载路径必须位于 jars 目录");
    parts[..=idx].join("/")
}

fn md5_hex(path: &PathBuf) -> String {
    let bytes = std::fs::read(path).expect("读取下载来源");
    format!("{:x}", md5::compute(&bytes))
}

fn load_index() -> Option<Value> {
    let text = std::fs::read_to_string(root().join("_rea/samples/library/index.json")).ok()?;
    Some(serde_json::from_str(&text).expect("index.json 必须是合法 JSON"))
}

fn load_profiles() -> Value {
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/spider/catvod/profiles.json"),
    )
    .expect("画像文件必须存在");
    serde_json::from_str(&text).expect("profiles.json 必须是合法 JSON")
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn every_download_source_rehashes_to_its_canonical() {
    let Some(index) = load_index() else {
        return;
    };
    let records = index["jars"].as_array().expect("jars 必须是数组");
    assert_eq!(records.len(), 28, "应登记 28 次下载（21 唯一 + 7 重复）");

    let canon: Vec<&Value> = records
        .iter()
        .filter(|record| record["dup"].as_bool() != Some(true))
        .collect();
    assert_eq!(canon.len(), 21, "canonical 记录应为 21 条");

    let mut files = BTreeSet::new();
    for record in &canon {
        let file = norm(record["file"].as_str().expect("canonical 缺 file"));
        assert!(files.insert(file.clone()), "canonical file 重复：{file}");
        assert!(record["bytes"].is_u64(), "canonical 缺 bytes：{file}");
        assert!(record["md5"].is_string(), "canonical 缺 md5：{file}");
        assert!(record["sha256"].is_string(), "canonical 缺 sha256：{file}");
    }
    assert_eq!(files.len(), 21);

    let by_file: HashMap<String, &Value> = canon
        .iter()
        .map(|record| (norm(record["file"].as_str().unwrap()), *record))
        .collect();

    let mut sources = BTreeSet::new();
    let mut root_hits = BTreeSet::new();
    for record in records {
        let from = record["from"].as_str().expect("记录缺 from");
        assert!(sources.insert(from.to_string()), "from 重复：{from}");
        root_hits.insert(jar_root(from));

        let path = root().join(from);
        assert!(path.exists(), "下载来源不存在：{from}");

        let file = norm(record["file"].as_str().expect("记录缺 file"));
        let target = by_file
            .get(&file)
            .unwrap_or_else(|| panic!("记录指向不存在的 canonical：{file}"));
        let expected_bytes = target["bytes"].as_u64().unwrap();
        let actual_bytes = std::fs::metadata(&path).expect("读取来源元数据").len();
        assert_eq!(actual_bytes, expected_bytes, "{from} 长度与 canonical 不符");
        assert_eq!(
            md5_hex(&path),
            target["md5"].as_str().unwrap(),
            "{from} 内容与 canonical 不符（来源被替换？）"
        );
    }
    assert_eq!(sources.len(), 28, "28 次下载的 from 应互不重复");
    assert_eq!(
        root_hits,
        ["_rea/samples/duocang/jars", "_rea/samples/gyck/jars", "_rea/samples/jars"]
            .iter()
            .map(|root| root.to_string())
            .collect::<BTreeSet<_>>(),
        "下载根应恰好是 dc / GYCK / 4k 三个订阅"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn profile_names_trace_to_download_roots() {
    let Some(index) = load_index() else {
        return;
    };
    let profiles = load_profiles();

    let canon: Vec<&Value> = index["jars"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["dup"].as_bool() != Some(true))
        .collect();
    let by_sha: HashMap<&str, &Value> = canon
        .iter()
        .map(|record| (record["sha256"].as_str().unwrap(), *record))
        .collect();

    let mut roots = BTreeSet::new();
    let mut checked = 0;
    for jar in profiles["jars"].as_array().expect("jars 必须是数组") {
        let name = jar["name"].as_str().expect("画像 jar 缺 name");
        let sha = jar["sha256"].as_str().expect("画像 jar 缺 sha256");
        let canonical = by_sha
            .get(sha)
            .unwrap_or_else(|| panic!("画像 jar 的 sha256 不在 canonical 集合：{name}"));

        let from = canonical["from"].as_str().unwrap();
        let original = base_name(from);
        let prefix = &canonical["md5"].as_str().unwrap()[..8];
        assert!(
            name == original || name == format!("{prefix}-{original}"),
            "画像 jar 名 {name} 无法由下载来源 {original} 推导"
        );
        roots.insert(jar_root(from));
        checked += 1;
    }
    assert_eq!(checked, 20, "画像应登记 20 个可用 jar");
    assert_eq!(roots.len(), 3, "20 个 jar 应分布在三个订阅根：{roots:?}");
}

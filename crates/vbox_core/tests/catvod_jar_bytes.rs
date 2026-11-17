//! 判据 54：jar 文件字节 ↔ dex 分析产物 ↔ 画像的最后一跳 provenance。
//!
//! 判据 53 把 `profiles.json` 的 sha256/bytes/classes/spiders 对回了
//! `_rea/samples/library/dexinfo.json` 的分析观测，但 dexinfo 里的 sha256/bytes
//! 本身仍可能是手工抄的。这里直接读 `_rea/samples/library/jars/` 下的文件字节：
//!   * 文件长度 == dexinfo.bytes；
//!   * 文件 MD5 == dexinfo.md5（dexinfo 同时记录 md5 与 sha256，md5 足以证明
//!     「分析的那份字节就是磁盘上这份」，无碰撞顾虑）；
//!   * 画像 sha256 能在 dexinfo 找到同一条目（与判据 53 形成三角）；
//!   * `jars/` 目录文件集合 == dexinfo 记录集合（无孤儿、无缺失）。
//!
//! 使用已有 `md5 = "0.7"` 依赖，不新增 crate；`_rea/` 缺失时 CI 软跳过。
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates 目录")
        .parent()
        .expect("仓库根")
        .to_path_buf()
}

fn load_json(path: &Path) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{} 不是合法 JSON: {e}", path.display())))
}

fn dexinfo_path() -> PathBuf {
    repo_root().join("_rea/samples/library/dexinfo.json")
}

fn jars_dir() -> PathBuf {
    repo_root().join("_rea/samples/library/jars")
}

fn normalize_rel(file: &str) -> PathBuf {
    PathBuf::from(file.replace('\\', "/"))
}

fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

#[test]
fn jar_files_match_dexinfo_checksums() {
    let Some(dexinfo) = load_json(&dexinfo_path()) else {
        eprintln!("skip: {} 不存在（CI 无 _rea 快照）", dexinfo_path().display());
        return;
    };
    let jars = dexinfo["jars"].as_array().expect("dexinfo 缺 jars 数组");
    let root = repo_root();
    let mut errors = Vec::new();

    for jar in jars {
        let file = jar["file"].as_str().unwrap_or_else(|| panic!("dexinfo jar 缺 file: {jar}"));
        let path = root.join(normalize_rel(file));
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("{file}: 读取失败 {e}"));
                continue;
            }
        };
        let want_bytes = jar["bytes"].as_u64().unwrap_or(0);
        if bytes.len() as u64 != want_bytes {
            errors.push(format!("{file}: 文件 {} B != dexinfo {} B", bytes.len(), want_bytes));
        }
        let want_md5 = jar["md5"].as_str().unwrap_or("").to_ascii_lowercase();
        let got_md5 = md5_hex(&bytes);
        if got_md5 != want_md5 {
            errors.push(format!("{file}: md5 {got_md5} != dexinfo {want_md5}"));
        }
    }
    assert!(errors.is_empty(), "jar 文件字节与 dexinfo 脱节（{} 处）:\n{}", errors.len(), errors.join("\n"));
}

#[test]
fn jar_file_set_matches_dexinfo() {
    let Some(dexinfo) = load_json(&dexinfo_path()) else {
        eprintln!("skip: {} 不存在（CI 无 _rea 快照）", dexinfo_path().display());
        return;
    };
    let dir = jars_dir();
    let mut on_disk = BTreeSet::new();
    for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", dir.display())) {
        let entry = entry.expect("目录项");
        if entry.file_type().expect("file_type").is_file() {
            on_disk.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    let recorded: BTreeSet<String> = dexinfo["jars"]
        .as_array()
        .expect("dexinfo 缺 jars 数组")
        .iter()
        .map(|j| {
            let file = j["file"].as_str().expect("dexinfo jar 缺 file");
            Path::new(&file.replace('\\', "/"))
                .file_name()
                .expect("file 无文件名")
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    let only_disk: Vec<_> = on_disk.difference(&recorded).cloned().collect();
    let only_record: Vec<_> = recorded.difference(&on_disk).cloned().collect();
    assert!(
        only_disk.is_empty() && only_record.is_empty(),
        "jars/ 与 dexinfo 记录不一致：仅磁盘 {only_disk:?}；仅记录 {only_record:?}"
    );

    // 画像侧 sha256 三角：每个画像 jar 都能在 dexinfo 找到（判据 53 正向的复核）。
    let profiles = load_json(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/spider/catvod/profiles.json"),
    )
    .expect("profiles.json 必须存在");
    let dex_shas: BTreeSet<&str> = dexinfo["jars"]
        .as_array()
        .expect("dexinfo 缺 jars 数组")
        .iter()
        .map(|j| j["sha256"].as_str().expect("dexinfo jar 缺 sha256"))
        .collect();
    let missing: Vec<String> = profiles["jars"]
        .as_array()
        .expect("profiles 缺 jars 数组")
        .iter()
        .filter_map(|j| {
            let sha = j["sha256"].as_str().expect("画像 jar 缺 sha256");
            let name = j["name"].as_str().unwrap_or("<未命名>");
            (!dex_shas.contains(sha)).then(|| format!("{name} ({sha})"))
        })
        .collect();
    assert!(missing.is_empty(), "画像 jar 在 dexinfo 中没有字节记录：{missing:?}");
}

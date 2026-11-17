//! 判据 T3：描述台账（lock）+ 差分可复现 + 语义/非语义分类。
//!
//! - `.lock` 记录每份描述的 `jar`/`jar_sha256`/描述字节指纹；
//! - jar 更新但描述未更新时，`lock_is_current_with_profiles` 直接变红（待审差分 > 0）；
//! - 差分工具对同一对输入两次运行必须字节一致；证据行号漂移不算语义差异。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use vbox_core::spider::catvod::descriptor::{registry, schema};
use vbox_core::spider::catvod::families::pan::sha256_hex;

const LOCK: &str = include_str!("../src/spider/catvod/descriptor/data/lock.json");
const PROFILES: &str = include_str!("../src/spider/catvod/profiles.json");

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/spider/catvod/descriptor/data")
}

fn parse_json(text: &str) -> Value {
    serde_json::from_str(text).expect("JSON 解析失败")
}

/// lock 与内嵌描述三方一致：api 对齐、jar 指纹一致、描述字节 sha256 一致。
#[test]
fn lock_matches_embedded_descriptors() {
    let lock = parse_json(LOCK);
    assert_eq!(lock["schema"], 1, "lock schema 版本应为 1");
    let entries = lock["entries"].as_object().expect("lock.entries 必须是对象");
    assert_eq!(
        entries.len(),
        registry::embedded().len(),
        "lock 条目数应与内嵌描述数一致"
    );
    for (api, text) in registry::embedded() {
        let desc = schema::parse(text).expect("内嵌描述解析失败");
        let entry = entries
            .get(&desc.api)
            .unwrap_or_else(|| panic!("lock 缺少 {} 条目（api={api}）", desc.api));
        assert_eq!(entry["jar"], desc.jar, "{} 的 jar 不一致", desc.api);
        assert_eq!(
            entry["jarSha256"], desc.jar_sha256,
            "{} 的 jar_sha256 不一致",
            desc.api
        );
        assert_eq!(
            entry["descriptorSha256"].as_str().unwrap_or_default(),
            sha256_hex(text),
            "{} 的描述字节指纹不一致（改描述后需重跑 --write-lock）",
            desc.api
        );
    }
}

/// jar 指纹与 profiles.json 一致（jar 更新未重抽取 => 待审差分，判据变红）。
#[test]
fn lock_is_current_with_profiles() {
    let profiles = parse_json(PROFILES);
    let jars = profiles["jars"].as_array().expect("profiles.jars 缺失");
    let lock = parse_json(LOCK);
    let entries = lock["entries"].as_object().expect("lock.entries 缺失");
    let mut pending = Vec::new();
    for (api, entry) in entries {
        let jar = entry["jar"].as_str().unwrap_or_default();
        let want = entry["jarSha256"].as_str().unwrap_or_default();
        let actual = jars
            .iter()
            .find(|j| j["name"].as_str() == Some(jar))
            .unwrap_or_else(|| panic!("profiles.json 中没有 jar {jar}（{api}）"));
        let actual = actual["sha256"].as_str().unwrap_or_default();
        if actual != want {
            pending.push(format!("{api}: {jar} 已更新，描述待重抽取"));
        }
    }
    assert!(pending.is_empty(), "存在待审差分: {pending:#?}");
}

fn run_diff(old: &Path, new: &Path) -> (i32, String) {
    let out = Command::new("node")
        .arg(repo_root().join("_rea/tools/diff-descriptor.mjs"))
        .arg("--old")
        .arg(old)
        .arg("--new")
        .arg(new)
        .arg("--json")
        .current_dir(repo_root())
        .output()
        .expect("无法运行 node diff-descriptor.mjs");
    let code = out.status.code().unwrap_or(-1);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (code, text)
}

fn temp_pair(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "vbox-descriptor-diff-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    (dir.clone(), dir.join("old.json"), dir.join("new.json"))
}

/// 同一对输入两次 diff 结果必须完全一致；空 diff 返回 0。
#[test]
fn diff_is_reproducible() {
    let src = data_dir().join("ysj.json");
    let (dir, old, new) = temp_pair("repro");
    std::fs::copy(&src, &old).expect("复制描述失败");
    std::fs::copy(&src, &new).expect("复制描述失败");
    let (code, first) = run_diff(&old, &new);
    let (code2, second) = run_diff(&old, &new);
    assert_eq!(code, 0, "空 diff 应返回 0，输出：{first}");
    assert_eq!(code2, code, "两次运行的退出码必须一致");
    assert_eq!(first, second, "同一对输入两次 diff 输出必须逐字节一致");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 语义差异必须判红；仅证据行号漂移不算语义差异。
#[test]
fn diff_classifies_semantic_and_evidence_only() {
    let src = data_dir().join("ysj.json");
    let text = std::fs::read_to_string(&src).expect("读取描述失败");
    let (dir, old, new) = temp_pair("classify");

    // 证据行号漂移（非语义）=> exit 0
    let mut value: Value = parse_json(&text);
    value["evidence"][0]["lines"] = Value::String("1-1".into());
    std::fs::write(&old, text.as_bytes()).expect("写 old 失败");
    std::fs::write(&new, serde_json::to_string_pretty(&value).unwrap()).expect("写 new 失败");
    let (code, out) = run_diff(&old, &new);
    assert_eq!(code, 0, "仅证据漂移不应判红，输出：{out}");
    assert!(out.contains("\"evidence\": 1"), "应把差异归入 evidence 分类：{out}");

    // 端点语义变化 => exit 1，并列出 semantic 差异
    let mut semantic: Value = parse_json(&text);
    semantic["endpoints"]["list"]["path"] = Value::String("/index.php/vod/show-changed".into());
    std::fs::write(&new, serde_json::to_string_pretty(&semantic).unwrap()).expect("写 new 失败");
    let (code, out) = run_diff(&old, &new);
    assert_eq!(code, 1, "语义差异必须返回 1，输出：{out}");
    assert!(
        out.contains("endpoints.list.path") && out.contains("\"semantic\": 1"),
        "应定位到端点语义差异：{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

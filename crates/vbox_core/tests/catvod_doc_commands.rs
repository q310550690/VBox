//! 判据 80：计划文档内嵌的复现命令必须可追溯。
//!
//! 判据 29 只校验 `docs/审计报告/兼容性审计.md` 的「复现命令」区块；计划文档正文里还有
//! 大量行内命令（§154.5 的 triage、§156.4 的探针、历轮的全量/审计命令），
//! 脚本改名或删掉后正文照旧、没人发现。本判据做静态可追溯性：
//! - `node _rea/...`：脚本必须存在；带 `--flag` 时脚本的「用法」行必须声明该开关；
//! - `cargo test ...`：`--test <name>` 必须有对应 `tests/<name>.rs`，
//!   `--manifest-path` 必须存在。
//! 不执行任何命令；CI 无 `_rea/` 时软跳过 node 侧。

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn plan_text() -> String {
    fs::read_to_string(root().join("docs/type3兼容/type3-实施计划-原生兼容100%.md"))
        .expect("读取计划文档")
}

fn has_ellipsis(s: &str) -> bool {
    s.contains('…') || s.contains('<') || s.contains('>')
}

fn usage_line(script: &Path) -> Option<String> {
    let text = fs::read_to_string(script).ok()?;
    text.lines()
        .find(|l| l.contains("用法"))
        .map(|l| l.trim().to_string())
}

fn flags_in(rest: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in rest.chars() {
        if ch == '-' {
            cur.push(ch);
        } else if ch.is_ascii_alphanumeric() && !cur.is_empty() {
            cur.push(ch);
        } else if !cur.is_empty() {
            if cur.starts_with("--") && cur.len() > 2 {
                out.push(cur.clone());
            }
            cur.clear();
            // 重新处理当前分隔字符：分隔字符本身不是标志的一部分
        } else {
            cur.clear();
        }
    }
    if cur.starts_with("--") && cur.len() > 2 {
        out.push(cur);
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn plan_embedded_node_commands_are_traceable() {
    if !root().join("_rea").exists() {
        eprintln!("跳过：CI 无 _rea/ 证据目录");
        return;
    }
    let text = plan_text();
    let mut seen = std::collections::BTreeSet::new();
    let mut checked = 0usize;
    let re_marker = "`node ";
    let mut rest = text.as_str();
    while let Some(pos) = rest.find(re_marker) {
        let after = &rest[pos + re_marker.len()..];
        let Some(end) = after.find('`') else { break };
        let inner = &after[..end];
        rest = &after[end..];
        if has_ellipsis(inner) {
            continue;
        }
        let mut tokens = inner.split_whitespace();
        let script = tokens.next().unwrap_or("");
        if !script.starts_with("_rea/") {
            continue;
        }
        let tail: Vec<&str> = tokens.collect();
        let key = format!("{script} {}", tail.join(" "));
        if !seen.insert(key) {
            continue;
        }
        let path = root().join(script);
        assert!(path.exists(), "计划文档引用的脚本不存在：{script}");
        let flags = flags_in(&tail.join(" "));
        if !flags.is_empty() {
            let usage = usage_line(&path)
                .unwrap_or_else(|| panic!("脚本 {script} 缺少「用法」注释，无法核对开关"));
            for flag in &flags {
                assert!(
                    usage.contains(flag.as_str()),
                    "脚本 {script} 的用法行未声明 {flag}（用法行：{usage}）"
                );
            }
        }
        checked += 1;
    }
    assert!(checked >= 15, "计划文档内嵌 node 命令过少：{checked}");
    for required in [
        "_rea/tmp/r153/triage.mjs",
        "_rea/tools/compat-audit.mjs",
        "_rea/tools/audit-all.mjs",
    ] {
        assert!(
            seen.iter().any(|k| k.starts_with(required)),
            "缺少关键复现命令：{required}"
        );
    }
}

#[test]
fn plan_embedded_cargo_commands_target_existing_files() {
    let text = plan_text();
    let mut seen = std::collections::BTreeSet::new();
    let mut targets = 0usize;
    let mut rest = text.as_str();
    while let Some(pos) = rest.find("`cargo test") {
        let after = &rest[pos + 1..];
        let Some(end) = after.find('`') else { break };
        let cmd = after[..end].trim().to_string();
        rest = &after[end..];
        if has_ellipsis(&cmd) || !seen.insert(cmd.clone()) {
            continue;
        }
        if let Some(name) = cmd.split("--test ").nth(1) {
            let name: &str = name.split_whitespace().next().unwrap_or("");
            assert!(!name.is_empty(), "cargo 行缺少测试名：{cmd}");
            let file = root().join("crates/vbox_core/tests").join(format!("{name}.rs"));
            assert!(file.exists(), "cargo 行引用的测试目标不存在：{name}（命令：{cmd}）");
            targets += 1;
        }
        if let Some(path) = cmd.split("--manifest-path ").nth(1) {
            let path: &str = path.split_whitespace().next().unwrap_or("");
            assert!(
                root().join(path).exists(),
                "cargo 行引用的 manifest 不存在：{path}（命令：{cmd}）"
            );
        }
    }
    assert!(seen.len() >= 15, "计划文档内嵌 cargo 命令过少：{}", seen.len());
    assert!(targets >= 10, "带 --test 的命令过少：{targets}");
}

//! 判据 29：审计报告「复现命令」区块的每一行都必须可追溯。
//!
//! 只做结构性校验（不执行命令）：node 脚本必须存在；`--flag` 必须在该脚本的
//! 「用法」注释里声明过；cargo 行必须是 `cargo test -p vbox_core ...` 形态。
//! 动机：复现命令是运维照着敲的，脚本改名/参数漂移会让复现静默失败。

use std::fs;
use std::path::PathBuf;

const REPORT: &str = "docs/审计报告/兼容性审计.md";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn report_text() -> String {
    fs::read_to_string(root().join(REPORT)).expect("读取审计报告")
}

fn repro_block(text: &str) -> String {
    let start = text
        .lines()
        .position(|l| l.starts_with("## ") && l.contains("复现命令"))
        .expect("缺少「复现命令」章节");
    let lines: Vec<&str> = text.lines().skip(start).collect();
    let fence_start = lines
        .iter()
        .position(|l| l.trim_start().starts_with("```bash"))
        .expect("复现命令缺少 bash 代码块");
    let rest = &lines[fence_start + 1..];
    let end = rest
        .iter()
        .position(|l| l.trim_start().starts_with("```"))
        .expect("bash 代码块未闭合");
    rest[..end].join("\n")
}

fn usage_line(script: &str) -> Option<String> {
    let path = root().join(script);
    let text = fs::read_to_string(path).ok()?;
    text.lines()
        .find(|l| l.contains("用法"))
        .map(|l| l.to_string())
}

#[test]
fn repro_commands_are_traceable() {
    let text = report_text();
    let block = repro_block(&text);
    let mut commands = 0usize;
    let mut checks = 0usize;
    for raw in block.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let command = line.split('#').next().unwrap_or("").trim();
        if command.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = command.split_whitespace().collect();
        commands += 1;
        match tokens[0] {
            "node" => {
                let script = tokens.get(1).unwrap_or_else(|| panic!("node 行缺少脚本：{line}"));
                let script_path = root().join(script);
                assert!(script_path.exists(), "脚本不存在：{script}（命令：{line}）");
                let usage =
                    usage_line(script).unwrap_or_else(|| panic!("脚本缺少「用法」注释：{script}"));
                for tok in tokens.iter().skip(2) {
                    if !tok.starts_with("--") {
                        continue;
                    }
                    assert!(
                        usage.contains(tok),
                        "脚本 {script} 的用法行未声明开关 {tok}（用法行：{usage}）"
                    );
                }
            }
            "cargo" => {
                assert!(
                    tokens.len() >= 4
                        && tokens[1] == "test"
                        && tokens[2] == "-p"
                        && tokens[3] == "vbox_core",
                    "cargo 行必须是 `cargo test -p vbox_core …`：{line}"
                );
            }
            other => panic!("复现命令以未知程序开头：{other}（命令：{line}）"),
        }
        if command.contains("--check") {
            checks += 1;
        }
    }
    assert!(commands >= 6, "复现命令过少：{commands}");
    assert!(checks >= 1, "复现命令缺少 `--check` 类型（幂等校验）命令");
}

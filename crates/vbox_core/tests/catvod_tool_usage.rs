//! 判据 25：审计工具的 `// 用法：` 注释必须覆盖它实现的所有开关，且文档化的开关必须真的存在。
//!
//! 命令行参数是最容易「文档漂移」的地方：脚本改了、用法注释没改，运维照着注释执行就会静默做错事。
//! 本判据对门禁工具集做双向的结构校验（不执行工具）：
//!  1) 每个门禁工具必须有一行含「用法」的注释；
//!  2) 该行出现的每个 `--flag` 必须在文件其余部分以引号字面量形式出现（即真的被处理）；

use std::fs;
use std::path::{Path, PathBuf};

const TOOLS: &[&str] = &[
    "audit-all.mjs",
    "audit-index.mjs",
    "audit-manifest.mjs",
    "audit-selftest.mjs",
    "compat-audit.mjs",
    "live-audit-report.mjs",
    "live-coverage.mjs",
    "site-coverage.mjs",
    "upstream-audit.mjs",
    "blocked-recheck.mjs",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("定位仓库根目录失败")
        .to_path_buf()
}

/// 提取一行里形如 `--flag-name` 的开关（只认 ASCII 小写/数字/短横线）。
fn flags_in(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'-' && bytes[i + 1] == b'-' {
            let mut j = i + 2;
            while j < bytes.len()
                && (bytes[j].is_ascii_lowercase() || bytes[j].is_ascii_digit() || bytes[j] == b'-')
            {
                j += 1;
            }
            if j > i + 2 {
                out.push(line[i..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// 文件里除去所有含「用法」的行之后的内容（避免把注释本身当成实现）。
fn body_without_usage(text: &str) -> String {
    text.lines().filter(|l| !l.contains("用法")).collect::<Vec<_>>().join("\n")
}

fn quoted_literals(body: &str, flag: &str) -> bool {
    for quote in ['\'', '"', '`'] {
        if body.contains(&format!("{quote}{flag}{quote}")) {
            return true;
        }
    }
    false
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn every_gate_tool_documents_its_flags_and_implements_them() {
    let root = repo_root();
    let mut problems = Vec::new();
    for tool in TOOLS {
        let path = root.join("_rea/tools").join(tool);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()));
        let usage_line = text
            .lines()
            .find(|l| l.contains("用法"))
            .map(str::to_string);
        let Some(usage_line) = usage_line else {
            problems.push(format!("{tool}: 缺少 `// 用法：` 注释行"));
            continue;
        };
        let documented = flags_in(&usage_line);
        if documented.is_empty() {
            problems.push(format!("{tool}: 用法行未列出任何 `--flag`"));
            continue;
        }
        let body = body_without_usage(&text);
        for flag in &documented {
            if !quoted_literals(&body, flag) {
                problems.push(format!("{tool}: 用法声明了 {flag}，但文件里没有该开关的字符串字面量"));
            }
        }
    }
    assert!(problems.is_empty(), "用法注释与实现漂移：\n{}", problems.join("\n"));
}

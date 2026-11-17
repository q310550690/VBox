//! 判据 21：每份自动生成报告都必须能追溯到「真实存在且被索引的生成器」。
//!
//! 五份报告头部都写着 `> 生成时间：…　|　生成命令：`node _rea/tools/<tool>.mjs --write``，
//! 但此前没有测试保证：该工具文件还在、命令里带 `--write`、且该工具被 `docs/审计索引.md` 收录。
//! 任一环节断裂（工具改名/删除/漏收录），报告都会变成「无法复现的孤儿」。

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("定位仓库根目录失败")
        .to_path_buf()
}

fn read(p: &str) -> String {
    let path = repo_root().join(p);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()))
}

fn generate_commands(report: &str) -> Vec<String> {
    report
        .lines()
        .take(8)
        .filter_map(|l| l.find("生成命令").map(|pos| &l[pos..]))
        .flat_map(|rest| {
            let mut out = Vec::new();
            let mut s = rest;
            while let Some(pos) = s.find("node _rea/tools/") {
                let tail = &s[pos..];
                let end = tail
                    .find('`')
                    .or_else(|| tail.find('　'))
                    .unwrap_or(tail.len());
                out.push(tail[..end].to_string());
                s = &tail[end..];
            }
            out
        })
        .collect()
}

const REPORTS: [&str; 5] = [
    "docs/兼容性审计.md",
    "docs/站点覆盖报告.md",
    "docs/真站回归矩阵.md",
    "docs/上游复核.md",
    "docs/阻断复核.md",
];

#[test]
fn every_report_names_an_existing_generator() {
    for report in REPORTS {
        let text = read(report);
        let cmds = generate_commands(&text);
        assert_eq!(cmds.len(), 1, "{report} 头部应有且仅有一条「生成命令：node ...」");
        let cmd = &cmds[0];
        let tool = cmd.split_whitespace().nth(1).expect("生成命令缺少工具路径");
        let path = repo_root().join(tool);
        assert!(path.is_file(), "{report} 的生成器 {tool} 不存在");
        assert!(
            cmd.contains("--write"),
            "{report} 的生成命令 {cmd} 缺少 `--write`——报告不应由只读模式生成"
        );
    }
}

#[test]
fn every_report_generator_is_indexed() {
    let index = read("docs/审计索引.md");
    for report in REPORTS {
        let text = read(report);
        let cmd = generate_commands(&text)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{report} 缺少生成命令"));
        let tool = cmd.split_whitespace().nth(1).expect("生成命令缺少工具路径");
        assert!(
            index.contains(&format!("`{tool}`")),
            "{report} 的生成器 {tool} 未被 docs/审计索引.md 收录——报告无法被门禁追溯"
        );
    }
}

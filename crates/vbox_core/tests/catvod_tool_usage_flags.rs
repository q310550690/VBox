//! 判据 31：工具「用法」行开关 ↔ 实际实现 双向一致。
//!
//! 判据 25 只保证「用法行的开关在别处被引用」，判据 30 只校验路径。
//! 本判据补齐双向：
//! 1. 用法行声明的开关，必须在文件别处以字符串字面量出现（否则文档写了没实现）；
//! 2. 从 argv 解析行里出现的开关，必须写进用法行（否则实现了没文档，运维不知道）。

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn collect_tools(dir: &PathBuf, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_tools(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("mjs") {
            out.push(path);
        }
    }
}

fn flags_in(text: &str) -> Vec<String> {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == '-' && bytes[i + 1] == '-' && bytes[i + 2].is_ascii_alphabetic() {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == '-') {
                j += 1;
            }
            let flag: String = bytes[i..j].iter().collect();
            if !out.contains(&flag) {
                out.push(flag);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[test]
fn usage_flags_and_argv_flags_agree() {
    let mut tools = Vec::new();
    collect_tools(&root().join("_rea/tools"), &mut tools);
    collect_tools(&root().join("_rea/samples/inventory"), &mut tools);
    assert!(tools.len() >= 12, "工具数量过少：{}", tools.len());

    let mut checked = 0usize;
    for path in &tools {
        let content = fs::read_to_string(path).expect("读取工具");
        let lines: Vec<&str> = content.lines().collect();
        let Some(usage_line) = lines.iter().find(|l| l.contains("用法：")) else {
            continue;
        };
        let declared = flags_in(usage_line);
        checked += 1;
        if declared.is_empty() {
            // 允许「纯位置参数 / 环境变量」型工具（如 decode-arrays.mjs <JavaFile>），
            // 但用法行必须显式给出位置参数或环境变量说明。
            assert!(
                usage_line.contains('<') || usage_line.contains("环境变量") || usage_line.trim_end().ends_with(".mjs"),
                "{} 的用法行既无开关也无位置参数说明：{usage_line}",
                path.display()
            );
            continue;
        }

        // 1) 声明的开关必须真的被实现（在用法行之外的任意位置以「'--x'」形式出现）
        let body = lines
            .iter()
            .filter(|l| !l.contains("用法："))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        for flag in &declared {
            let quoted = format!("'{flag}'");
            let quoted_dq = format!("\"{flag}\"");
            let quoted_bq = format!("`{flag}`");
            assert!(
                body.contains(&quoted) || body.contains(&quoted_dq) || body.contains(&quoted_bq),
                "{} 的用法行声明了 {flag}，但文件中未实现",
                path.display()
            );
        }

        // 2) argv 解析行里出现的开关必须写进用法行
        for line in &lines {
            if !line.contains("argv") {
                continue;
            }
            for flag in flags_in(line) {
                assert!(
                    declared.contains(&flag),
                    "{} 在 argv 解析里使用了 {flag}，但用法行未声明：{usage_line}",
                    path.display()
                );
            }
        }
    }
    assert!(checked >= 10, "带用法行的工具过少：{checked}");
}

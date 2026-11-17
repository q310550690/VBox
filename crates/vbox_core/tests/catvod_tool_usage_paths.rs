//! 判据 30：工具「用法」注释里的脚本路径必须与文件真实路径一致。
//!
//! 判据 25/29 校验的是「开关有没有被声明/有没有漂移」，但不校验用法行里的
//! 脚本路径拼写（复制粘贴旧路径、改目录后忘改注释都会导致运维敲错命令）。

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn collect_tools(dir: &PathBuf, out: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_tools(&path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("mjs") {
            continue;
        }
        let rel = path
            .strip_prefix(root())
            .expect("工具位于仓库内")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.push(rel);
    }
}

#[test]
fn usage_lines_point_at_real_paths() {
    let mut tools = Vec::new();
    collect_tools(&root().join("_rea/tools"), &mut tools);
    collect_tools(&root().join("_rea/samples/inventory"), &mut tools);
    tools.sort();
    assert!(tools.len() >= 12, "工具数量过少：{}", tools.len());

    let mut with_usage = 0usize;
    for rel in &tools {
        let content = fs::read_to_string(root().join(rel)).expect("读取工具");
        let usage = content
            .lines()
            .find(|l| l.contains("用法："))
            .map(str::trim);
        let Some(usage) = usage else { continue };
        with_usage += 1;
        // 形如 `// 用法：node _rea/tools/xxx.mjs [--flag]`
        let after = usage
            .split("用法：")
            .nth(1)
            .expect("用法行含分隔符")
            .trim();
        let mut it = after.split_whitespace();
        let program = it.next().unwrap_or_default();
        assert_eq!(program, "node", "{rel} 的用法行程序名应为 node：{usage}");
        let script = it.next().unwrap_or_else(|| panic!("{rel} 的用法行缺少脚本路径"));
        assert_eq!(
            script, rel,
            "{rel} 的用法行路径与真实路径不一致（用法行：{usage}）"
        );
    }
    // 门禁工具全部带用法行（12 个）；逆向辅助脚本多为一次性/交互式，不强制，
    // 但要求至少 10 个工具可被「用法：node <path>」追溯。
    assert!(with_usage >= 10, "带用法行的工具过少：{with_usage}");
}

//! 判据 22：两份计划文档的「进度节/附录」编号不得断档或重复。
//!
//! 约定（第 31 轮起严格执行）：
//! - `docs/计划-type3-原生兼容100%.md` 的 `## <N>. 进度补充…` 节号从 31 起连续递增；
//! - `docs/计划-type3-REA原生重写.md` 的 `## 附录 <LETTER>. …` 从 X 起按字母表连续递增。
//! 早期（<31 节 / <X 附录）存在历史写法差异，不在本判据范围。

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

fn plan_section_numbers(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("## ") else { continue };
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        if rest[digits.len()..].starts_with('.') {
            out.push(digits.parse::<usize>().expect("节号解析失败"));
        }
    }
    out
}

fn appendix_index(letter: &str) -> usize {
    letter.bytes().fold(0usize, |acc, b| acc * 26 + (b as usize - 64))
}

fn appendix_letters(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("## 附录 ") else { continue };
        let letters: String = rest.chars().take_while(|c| c.is_ascii_uppercase()).collect();
        if letters.is_empty() {
            continue;
        }
        if rest[letters.len()..].starts_with('.') {
            out.push(letters);
        }
    }
    out
}

#[test]
fn plan_progress_sections_are_contiguous() {
    let text = read("docs/计划-type3-原生兼容100%.md");
    let nums = plan_section_numbers(&text);
    assert!(nums.len() >= 60, "进度节数量异常: {}", nums.len());
    let max = *nums.iter().max().expect("没有进度节");
    assert!(max >= 90, "计划文档进度节最大编号异常: {max}（应 ≥90）");
    let set: std::collections::HashSet<_> = nums.iter().copied().collect();
    let mut gaps = Vec::new();
    for n in 31..=max {
        if !set.contains(&n) {
            gaps.push(n);
        }
    }
    assert!(gaps.is_empty(), "进度节编号断档: {gaps:?}");
    let mut sorted = nums.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), set.len(), "存在重复的进度节编号");
    assert_eq!(
        nums.clone(),
        sorted,
        "进度节编号未按出现顺序递增（前面出现了更大编号）"
    );
}

#[test]
fn rea_appendices_are_contiguous() {
    let text = read("docs/计划-type3-REA原生重写.md");
    let letters = appendix_letters(&text);
    assert!(letters.len() >= 60, "附录数量异常: {}", letters.len());
    let ids: Vec<usize> = letters.iter().map(|l| appendix_index(l)).collect();
    let base = appendix_index("X");
    let max = *ids.iter().max().expect("没有附录");
    assert!(max >= appendix_index("CU"), "附录最大字母异常: {}", letters.last().unwrap());
    let set: std::collections::HashSet<_> = ids.iter().copied().collect();
    let gaps: Vec<String> = (base..=max)
        .filter(|n| !set.contains(n))
        .map(|n| format!("#{n}"))
        .collect();
    assert!(gaps.is_empty(), "附录编号断档: {gaps:?}");
    assert_eq!(ids.len(), set.len(), "存在重复的附录编号");
    assert_eq!(ids.clone(), {
        let mut s = ids.clone();
        s.sort_unstable();
        s
    }, "附录未按字母顺序递增（前面出现了更大字母）");
}

//! 判据 24：两份计划文档的「最新轮次」必须互相对齐。
//!
//! 计划文档的 `## <N>. 进度补充（… 第 <K> 轮）` 与 REA 文档的 `## 附录 <L>. …（第 <K> 轮）`
//! 是同一轮工作的两份记录。若某一轮只更新了一边，读者会看到两份文档对「最新进展」的叙述不一致。
//! 本判据断言：两边记录的轮次序列单调不减，且最新轮次相同。

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

/// 从 `…（YYYY-MM-DD 第 K 轮）` 里取 K。
fn round_of(line: &str) -> Option<usize> {
    let pos = line.rfind("第 ")?;
    let tail = &line[pos + "第 ".len()..];
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || !line.contains("轮）") {
        return None;
    }
    digits.parse().ok()
}

fn plan_rounds() -> Vec<usize> {
    let text = read("docs/type3兼容/type3-实施计划-原生兼容100%.md");
    text.lines()
        .filter(|l| l.starts_with("## "))
        // 两种标题都承载轮次：早期「## N. 进度补充（… 第 K 轮）」，
        // 后期「## N. 第 K 轮：主题」。只认同一种会让判据在风格切换后失明。
        .filter_map(|l| {
            if l.contains("进度补充") {
                round_of(l)
            } else {
                let pos = l.find("第 ")?;
                let digits: String = l[pos + "第 ".len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if digits.is_empty() || !l.contains("轮") {
                    None
                } else {
                    digits.parse().ok()
                }
            }
        })
        .collect()
}

fn rea_rounds() -> Vec<String> {
    let text = read("docs/type3兼容/type3-路线-REA原生重写.md");
    text.lines()
        .filter(|l| l.starts_with("## 附录 "))
        .filter_map(|l| round_of(l).map(|k| format!("{k}")))
        .collect()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn latest_round_matches_between_plan_and_rea() {
    let plan = plan_rounds();
    let rea = rea_rounds();
    assert!(plan.len() >= 60, "计划文档带轮次的进度节过少: {}", plan.len());
    assert!(rea.len() >= 40, "REA 文档带轮次的附录过少: {}", rea.len());
    let plan_max = *plan.iter().max().expect("计划文档没有轮次标注");
    // 注意：REA 侧必须是**数值**最大，不能取字符串字典序（第 100 轮 < 第 99 轮）。
    let rea_max: usize = *rea
        .iter()
        .map(|s| s.parse::<usize>().expect("REA 轮次无法解析为数字"))
        .max()
        .as_ref()
        .expect("REA 文档没有轮次标注");
    assert!(plan_max >= 90, "计划文档最新轮次异常: {plan_max}（应 ≥90）");
    assert_eq!(
        plan_max, rea_max,
        "两份计划文档的最新轮次不一致：plan={plan_max}，rea={rea_max}——有一边漏更新"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn round_sequences_are_monotonic() {
    let plan = plan_rounds();
    for pair in plan.windows(2) {
        assert!(
            pair[1] >= pair[0],
            "计划文档轮次倒退：{} → {}",
            pair[0],
            pair[1]
        );
    }
    let rea: Vec<usize> = rea_rounds().iter().map(|s| s.parse().unwrap()).collect();
    for pair in rea.windows(2) {
        assert!(
            pair[1] >= pair[0],
            "REA 文档轮次倒退：{} → {}",
            pair[0],
            pair[1]
        );
    }
}

//! 判据 37：残余阻断归因矩阵必须与画像的 unported 集合一致（且每条证据列非空）。
//!
//! 背景：第 152 轮后主线家族移植归零，剩余 unported 项无法按家族方式继续推进。
//! 若不把「为什么不能原生等价」固化，`100% 判定` 里「站点覆盖」的可信度就无从复核：
//! 报告声称「剩余项均有阻断理由」，但理由是否真实、是否与画像一致，只有靠判据锁住。
//!
//! 本判据解析 `docs/type3兼容/type3-实施计划-原生兼容100%.md` 的 §154.2 归因矩阵：
//! - 表行数必须等于画像中 `strategy == "unported"` 的 api 数；
//! - api 集合必须与画像 unported 集合完全相等（多/少/改名都红）；
//! - 每行 blockedReason 必须与画像 `blockedReason` 一致；
//! - 证据列（源码行数/skip/host-core/继承/WebView/Android UI/SQLite）必须可解析且非空；
//! - `_rea/tmp/r153/triage.json` 存在时（本地开发环境）逐项复核指标；
//!   不存在时（CI / 发布环境无 `_rea`）降级为「文档 ↔ 画像」一致性校验，并打印说明。
//!
//! 为何要降级而不是失败：`_rea` 是本地反编译产物目录，不入库；CI 无法复算源码指标，
//! 但「画像 unported 集合 = 文档矩阵集合」这一层仍可校验，且这是最关键的防漂移约束。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn plan_text() -> String {
    fs::read_to_string(root().join("docs/type3兼容/type3-实施计划-原生兼容100%.md"))
        .expect("读取 docs/type3兼容/type3-实施计划-原生兼容100%.md")
}

fn profiles() -> Value {
    let path = root().join("crates/vbox_core/src/spider/catvod/profiles.json");
    let text = fs::read_to_string(path).expect("读取 profiles.json");
    serde_json::from_str(&text).expect("profiles.json 解析")
}

struct Row {
    api: String,
    sites: u64,
    difficulty: String,
    reason: String,
    lines: u64,
    skipped: u64,
    host_core: u64,
    extends: String,
    webview: u64,
    android_ui: u64,
    sqlite: u64,
}

fn parse_section(plan: &str) -> Vec<Row> {
    let start = plan.find("\n## 154.").expect("plan 中应有 §154 归因矩阵章节");
    let rest = &plan[start + 1..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    let section = &rest[..end];

    let mut rows = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        if !line.starts_with("| `csp_") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(|c| c.trim()).collect();
        // cells[0] 与末项是空串（行首/行尾的 |）
        assert!(
            cells.len() >= 13,
            "归因矩阵行格式错误（列数 {}）：{line}",
            cells.len()
        );
        let num = |idx: usize, name: &str| -> u64 {
            let raw = cells[idx].trim_matches('`');
            raw.parse::<u64>()
                .unwrap_or_else(|_| panic!("{name} 列不是数字: {raw} @ {line}"))
        };
        rows.push(Row {
            api: cells[1].trim_matches('`').to_string(),
            sites: num(2, "站数"),
            difficulty: cells[3].trim_matches('`').to_string(),
            reason: cells[4].trim_matches('`').to_string(),
            lines: num(5, "源码行数"),
            skipped: num(6, "skip 指令"),
            host_core: num(7, "host-core 依赖类"),
            extends: cells[8].trim_matches('`').to_string(),
            webview: num(9, "WebView"),
            android_ui: num(10, "Android UI"),
            sqlite: num(11, "SQLite/Prefs"),
        });
    }
    rows
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn residual_triage_matrix_matches_profiles_and_evidence() {
    let plan = plan_text();
    let rows = parse_section(&plan);
    assert_eq!(
        rows.len(),
        5,
        "§154.2 归因矩阵应恰有 5 行（第 257 轮 csp_P115Share、第 258 轮 csp_TGYunPan/TGYunPanLocal、第 259 轮 csp_PikPakShare、第 260 轮 csp_ThunderShare、第 261 轮 csp_SambaShare、第 262 轮 csp_Youtube、第 263 轮 csp_PushShare、第 264 轮 csp_Panyq 原生落地后移出），实际 {}",
        rows.len()
    );

    let profiles = profiles();
    let apis = profiles["apis"].as_object().expect("profiles.apis 对象");
    let mut unported: BTreeMap<String, String> = BTreeMap::new();
    for (api, value) in apis {
        if value["strategy"].as_str() == Some("unported") {
            unported.insert(
                api.clone(),
                value["blockedReason"]
                    .as_str()
                    .expect("unported 必须有 blockedReason")
                    .to_string(),
            );
        }
    }
    assert_eq!(
        unported.len(),
        5,
        "画像 unported 应恰有 5 项（第 257/258/259/260/261/262/263/264 轮移出 P115Share/TGYunPan×2/PikPakShare/ThunderShare/SambaShare/Youtube/PushShare/Panyq），实际 {:?}",
        unported.keys().collect::<Vec<_>>()
    );

    let matrix: BTreeMap<String, String> = rows
        .iter()
        .map(|r| (r.api.clone(), r.reason.clone()))
        .collect();
    assert_eq!(
        matrix.len(),
        rows.len(),
        "归因矩阵存在重复 api：{:?}",
        matrix.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        matrix.keys().collect::<Vec<_>>(),
        unported.keys().collect::<Vec<_>>(),
        "归因矩阵 api 集合必须与画像 unported 集合相等"
    );
    for (api, reason) in &unported {
        assert_eq!(
            matrix.get(api).map(String::as_str),
            Some(reason.as_str()),
            "{api} 的 blockedReason 与画像不一致"
        );
    }

    // 证据列非空 + 分类计数与文档 154.4 一致
    let mut reason_count: BTreeMap<&str, u64> = BTreeMap::new();
    for r in &rows {
        assert!(!r.extends.is_empty(), "{} 的继承列不能为空", r.api);
        assert!(r.lines > 0, "{} 的源码行数必须为正", r.api);
        assert!(r.sites > 0, "{} 的站数必须为正", r.api);
        *reason_count.entry(r.reason.as_str()).or_default() += 1;
    }
    for (reason, expected) in [
        ("decompiler_gap", 0u64),
        ("engine_dependency", 1),
        ("webview_dependency", 1),
        ("site_dead", 3),
    ] {
        assert_eq!(
            reason_count.get(reason).copied().unwrap_or(0),
            expected,
            "{reason} 分类数量与 §154.4 不一致"
        );
    }
    let sites_total: u64 = rows.iter().map(|r| r.sites).sum();
    let dist = format!("合计 **{} 项 / {} 站**", rows.len(), sites_total);
    assert!(
        plan.contains(&dist),
        "§154.4 数量分布应包含 `{dist}`"
    );    // 审计报告必须同步呈现残余归因摘要：报告读者看不到剩余缺口，等于把问题藏起来。
    // 该行由 `_rea/tools/compat-audit.mjs` 生成（第 153 轮加入），此处锁住「报告 ↔ 画像」一致。
    let mut report = fs::read_to_string(root().join("docs/审计报告/兼容性审计.md"))
        .expect("读取 docs/审计报告/兼容性审计.md");
    if !report.contains("- **残余阻断归因**：") && root().join("_rea/tools/compat-audit.mjs").exists() {
        // 报告可能尚未随本轮生成（例如先跑单测再生成报告）：重新生成一次再读。
        // 仅在本地有 _rea 生成器时重生成；CI/发布环境没有 _rea，报告是入库快照，跳过。
        let status = std::process::Command::new("node")
            .arg("_rea/tools/compat-audit.mjs")
            .arg("--write")
            .current_dir(root())
            .status()
            .expect("运行 compat-audit 生成审计报告");
        assert!(status.success(), "compat-audit --write 执行失败");
        report = fs::read_to_string(root().join("docs/审计报告/兼容性审计.md"))
            .expect("读取 docs/审计报告/兼容性审计.md（重生成后）");
    }
    let line = report
        .lines()
        .find(|l| l.starts_with("- **残余阻断归因**："))
        .expect("审计报告缺少「残余阻断归因」行");
    assert!(
        line.contains(&format!("{} 项", rows.len())),
        "残余归因总数与矩阵不一致: {line}"
    );
    for (reason, expected) in [
        ("engine_dependency", 1),
        ("webview_dependency", 1),
        ("site_dead", 3),
    ] {
        assert!(
            line.contains(&format!("{reason} {expected}")),
            "残余归因摘要缺少 `{reason} {expected}`: {line}"
        );
    }

    // 本地开发环境：用 triage.json 复核指标；CI 无 _rea 时降级
    let triage_path = root().join("_rea/tmp/r153/triage.json");
    if triage_path.exists() {
        let raw = fs::read_to_string(&triage_path).expect("读取 triage.json");
        let data: Value = serde_json::from_str(&raw).expect("triage.json 解析");
        let items = data.as_array().expect("triage.json 应为数组");
        assert_eq!(items.len(), rows.len(), "triage.json 项数与矩阵不一致");
        let by_api: BTreeMap<String, &Value> = items
            .iter()
            .map(|v| {
                (
                    v["api"].as_str().expect("triage api").to_string(),
                    v,
                )
            })
            .collect();
        for r in &rows {
            let v = by_api
                .get(&r.api)
                .unwrap_or_else(|| panic!("triage.json 缺少 {}", r.api));
            assert_eq!(v["lines"].as_u64(), Some(r.lines), "{} 行数漂移", r.api);
            assert_eq!(
                v["skipped"].as_u64(),
                Some(r.skipped),
                "{} skip 数漂移",
                r.api
            );
            assert_eq!(
                v["hostCore"].as_u64(),
                Some(r.host_core),
                "{} host-core 数漂移",
                r.api
            );
            assert_eq!(
                v["webview"].as_u64(),
                Some(r.webview),
                "{} WebView 数漂移",
                r.api
            );
            assert_eq!(
                v["androidUi"].as_u64(),
                Some(r.android_ui),
                "{} Android UI 数漂移",
                r.api
            );
            assert_eq!(
                v["sqlite"].as_u64(),
                Some(r.sqlite),
                "{} SQLite/Prefs 数漂移",
                r.api
            );
            assert_eq!(
                v["extends"].as_str(),
                Some(r.extends.as_str()),
                "{} 继承关系漂移",
                r.api
            );
        }
    } else {
        eprintln!(
            "note: _rea/tmp/r153/triage.json 不存在（CI/发布环境），仅校验文档 ↔ 画像一致性"
        );
    }
}

// 判据 38：复评线索矩阵必须覆盖全部 unported，并给出可行动的「解阻条件 / 结论」。
// 只有「为什么阻断」（§154）而没有「何时能解阻」（§156）时，阻断结论会随时间失去可复核性。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn review_clues_matrix_matches_unported() {
    let plan = plan_text();
    let start = plan
        .find("\n## 156.")
        .expect("plan 中应有 §156 复评线索矩阵章节");
    let rest = &plan[start + 1..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    let section = &rest[..end];

    struct ClueRow {
        api: String,
        reason: String,
        condition: String,
        upstream: String,
        probe: String,
        verdict: String,
    }
    let mut rows: Vec<ClueRow> = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        if !line.starts_with("| `csp_") {
            continue;
        }
        let cells: Vec<&str> = line
            .split('|')
            .map(|s| s.trim())
            .collect();
        assert!(cells.len() >= 8, "§156 表格行缺列: {line}");
        let api = cells[1].trim_matches('`').to_string();
        let reason = cells[2].trim_matches('`').to_string();
        let condition = cells[3].to_string();
        let upstream = cells[4].trim_matches('`').to_string();
        let probe = cells[5].to_string();
        let verdict = cells[6].to_string();
        assert!(!condition.is_empty(), "{api} 缺「解阻条件」");
        assert!(!upstream.is_empty(), "{api} 缺「已知上游」");
        assert!(!probe.is_empty(), "{api} 缺「今日探测」");
        assert!(
            matches!(verdict.as_str(), "维持阻断" | "暂缓" | "可解阻"),
            "{api} 结论取值非法: {verdict}"
        );
        rows.push(ClueRow { api, reason, condition, upstream, probe, verdict });
    }
    assert_eq!(rows.len(), 5, "§156 复评线索应覆盖 5 项 unported");
    assert_eq!(
        rows.iter().filter(|r| r.verdict == "维持阻断").count(),
        5,
        "§156 分类小结应为维持阻断 5 项"
    );
    assert_eq!(
        rows.iter().filter(|r| r.verdict == "暂缓").count(),
        0,
        "§156 分类小结应为暂缓 0 项（第 263 轮后清零）"
    );

    let data = profiles();
    let apis = data["apis"].as_object().expect("profiles.apis 对象");
    let mut unported: BTreeMap<String, String> = BTreeMap::new();
    for (api, value) in apis {
        if value["strategy"].as_str() == Some("unported") {
            unported.insert(
                api.clone(),
                value["blockedReason"]
                    .as_str()
                    .expect("unported 必须有 blockedReason")
                    .to_string(),
            );
        }
    }
    let mut seen = BTreeMap::new();
    for r in &rows {
        assert!(!seen.contains_key(&r.api), "§156 重复 api: {}", r.api);
        seen.insert(r.api.clone(), r.reason.clone());
        let expect = unported
            .get(&r.api)
            .unwrap_or_else(|| panic!("§156 含画像外 api: {}", r.api));
        assert_eq!(&r.reason, expect, "{} 的 reason 与画像不一致", r.api);
    }
    assert_eq!(
        seen.keys().collect::<Vec<_>>(),
        unported.keys().collect::<Vec<_>>(),
        "§156 覆盖集合与画像 unported 集合不一致"
    );
}

/// 判据 78：§154.2 的「难度」列与 §154.4 的合计/分布文字都必须能从画像重算。
///
/// 为什么需要这一条：§154.2 矩阵此前只校验了 api/sites/reason 与证据列，**难度列从未被读**；
/// §154.4 的「合计 N 项 / M 站」与「L1/L2/L3 分布」也一直是手写文字。难度是排期与
/// 100% 判定里「剩余工作规模」的唯一口径，手写数字漂移不会触发任何门禁。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn triage_matrix_difficulty_and_summary_match_profiles() {
    let plan = plan_text();
    let rows = parse_section(&plan);
    assert!(!rows.is_empty(), "§154.2 归因矩阵为空");
    let profiles = profiles();
    let apis = profiles["apis"].as_object().expect("profiles.apis 对象");

    // 1) 每行难度 == 画像 difficulty，且取值合法。
    let mut dist: BTreeMap<String, u64> = BTreeMap::new();
    for row in &rows {
        assert!(
            matches!(row.difficulty.as_str(), "L1" | "L2" | "L3" | "L4"),
            "{} 的难度列非法：{}",
            row.api,
            row.difficulty
        );
        let profile = apis
            .get(&row.api)
            .unwrap_or_else(|| panic!("§154.2 的 {} 不在画像中", row.api));
        assert_eq!(
            profile["difficulty"].as_str().unwrap_or(""),
            row.difficulty,
            "{} 的难度与画像不一致",
            row.api
        );
        *dist.entry(row.difficulty.clone()).or_insert(0) += 1;
    }

    // 2) §154.4 是独立小节，取到下一个二级标题为止。
    let sec_start = plan.find("## 154.4").expect("plan 中应有 §154.4 数量分布");
    let sec = &plan[sec_start..];
    let sec_end = sec[1..].find("\n## ").map(|i| i + 1).unwrap_or(sec.len());
    let sec = &sec[..sec_end];

    let total_re = regex::Regex::new(r"合计 \*\*(\d+) 项 / (\d+) 站\*\*").unwrap();
    let caps = total_re.captures(sec).expect("§154.4 缺少「合计 N 项 / M 站」");
    let documented_items: u64 = caps[1].parse().unwrap();
    let documented_sites: u64 = caps[2].parse().unwrap();
    let sites_sum: u64 = rows.iter().map(|r| r.sites).sum();
    assert_eq!(documented_items, rows.len() as u64, "§154.4 项数与矩阵行数不一致");
    assert_eq!(documented_sites, sites_sum, "§154.4 站数与矩阵站数和不一致");

    // 3) §154.4 原因分布 == 矩阵实际计数。
    let reason_re = regex::Regex::new(
        r"decompiler_gap (\d+)、engine_dependency (\d+)、webview_dependency (\d+)、site_dead (\d+)",
    )
    .unwrap();
    let caps = reason_re.captures(sec).expect("§154.4 缺少原因分布");
    let documented: Vec<u64> = (1..=4).map(|i| caps[i].parse().unwrap()).collect();
    let actual = [
        rows.iter().filter(|r| r.reason == "decompiler_gap").count() as u64,
        rows.iter().filter(|r| r.reason == "engine_dependency").count() as u64,
        rows.iter().filter(|r| r.reason == "webview_dependency").count() as u64,
        rows.iter().filter(|r| r.reason == "site_dead").count() as u64,
    ];
    assert_eq!(documented, actual, "§154.4 原因分布与矩阵不一致");

    // 4) §154.4 难度分布 == 矩阵重算，且四类合计 == 行数。
    let diff_re = regex::Regex::new(r"难度：L1 (\d+) 项、L2 (\d+) 项、L3 (\d+) 项").unwrap();
    let caps = diff_re.captures(sec).expect("§154.4 缺少难度分布");
    let documented: Vec<u64> = (1..=3).map(|i| caps[i].parse().unwrap()).collect();
    let actual: Vec<u64> = ["L1", "L2", "L3"]
        .iter()
        .map(|k| dist.get(*k).copied().unwrap_or(0))
        .collect();
    assert_eq!(documented, actual, "§154.4 难度分布与画像重算不一致");
    assert_eq!(
        dist.values().sum::<u64>(),
        rows.len() as u64,
        "难度分布合计与矩阵行数不一致"
    );
}

/// 判据 79：§156 正文里的手写计数必须与复评线索矩阵重算值一致。
///
/// 已有测试只校验表格行集合与「维持阻断 14 / 暂缓 2」的**表格重算**；但 §156.3 的
/// 「维持阻断 14 项」「暂缓 2 项」、标题里的「16 项」、§156.6 的「剩余 16 项」
/// 都是独立手写文字。改表不改文案（或反之）时，读者会得到与数据矛盾的说明。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn review_clues_prose_counts_match_table() {
    let plan = plan_text();
    let start = plan.find("\n## 156.").expect("plan 中应有 §156 复评线索矩阵");
    let rest = &plan[start + 1..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    let section = &rest[..end];

    // 只取结论列：表格结构由既有测试负责，这里只做计数对账。
    let mut verdicts: Vec<String> = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        if !line.starts_with("| `csp_") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
        assert!(cells.len() >= 8, "§156 表格行缺列: {line}");
        verdicts.push(cells[6].to_string());
    }
    let total = verdicts.len();
    let held = verdicts.iter().filter(|v| v.as_str() == "维持阻断").count();
    let deferred = verdicts.iter().filter(|v| v.as_str() == "暂缓").count();
    assert!(total > 0, "§156 表格为空");

    let title = section.lines().next().expect("§156 缺标题");
    let re_title = regex::Regex::new(r"（(\d+) 项「解阻条件」实证）").unwrap();
    let documented_total: usize = re_title
        .captures(title)
        .expect("§156 标题缺少「N 项『解阻条件』实证」")
        .get(1)
        .unwrap()
        .as_str()
        .parse()
        .unwrap();
    assert_eq!(
        documented_total, total,
        "§156 标题的项数与表格行数不一致"
    );

    let re_held = regex::Regex::new(r"维持阻断 (\d+) 项").unwrap();
    let documented_held: usize = re_held
        .captures(section)
        .expect("§156.3 缺少「维持阻断 N 项」")
        .get(1)
        .unwrap()
        .as_str()
        .parse()
        .unwrap();
    assert_eq!(
        documented_held, held,
        "§156.3 的维持阻断数与表格重算不一致"
    );

    let re_deferred = regex::Regex::new(r"暂缓 (\d+) 项").unwrap();
    let documented_deferred: usize = re_deferred
        .captures(section)
        .expect("§156.3 缺少「暂缓 N 项」")
        .get(1)
        .unwrap()
        .as_str()
        .parse()
        .unwrap();
    assert_eq!(
        documented_deferred, deferred,
        "§156.3 的暂缓数与表格重算不一致"
    );

    assert_eq!(
        held + deferred,
        total,
        "§156 表格存在未归类的结论（既非维持阻断也非暂缓）"
    );

    let re_left = regex::Regex::new(r"剩余 (\d+) 项").unwrap();
    let tail = section
        .find("### 156.6")
        .map(|i| &section[i..])
        .expect("§156 缺少 §156.6 结论与后续");
    let documented_left: usize = re_left
        .captures(tail)
        .expect("§156.6 缺少「剩余 N 项」")
        .get(1)
        .unwrap()
        .as_str()
        .parse()
        .unwrap();
    assert_eq!(documented_left, total, "§156.6 的剩余项数与表格行数不一致");
}

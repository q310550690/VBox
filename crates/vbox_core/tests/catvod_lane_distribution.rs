//! 判据 42：`docs/审计报告/站点覆盖报告.md` 的「分派车道分布」必须与策略画像的站点计数**三方互锁**。
//!
//! 为什么需要这条判据：
//! 第 158 轮发现 11 个 jar 站点（`csp_*`）因 `ext` 是 URL 被误送 Python 车道，用户看到的是
//! Python 侧错误而不是分类错误码。修复（`script_lane_allowed`）只能保证 `csp_*` 不再进脚本
//! 车道，但「分派结果整体是否与画像一致」仍需要一个可审计的账本：
//!
//! - 站点覆盖报告中的车道表由 `_rea/tools/site-coverage.mjs` 按运行时同一套优先级生成；
//! - 本判据只读**已入库**的报告与画像（不依赖 `_rea/`），在 CI 里校验：
//!   1. 六个车道的站点数之和 == 画像站点总数（612）；
//!   2. 报告合计行 == 六车道之和；
//!   3. native 车道 == rulepack 策略的站点数；
//!   4. classified 车道 == unported + unsupported 策略的站点数；
//!   5. drpy+js+py+source == script 策略的站点数（script 中 type=3 走三脚本车道，type≠3 走 cms/xml 源）；
//!   6. 「type=3 未分派：0 站」「csp_ 误入脚本车道：0 站」两条结论必须存在且为 0。
//!
//! 任何一项漂移都说明分派逻辑、画像或报告三者之一失配，按顺序重新生成报告并复核修复。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("定位仓库根目录失败")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("读取 {} 失败: {err}", path.display()))
}

fn profiles() -> Value {
    serde_json::from_str(&read("crates/vbox_core/src/spider/catvod/profiles.json"))
        .expect("profiles.json 不是合法 JSON")
}

/// 站点覆盖报告里的「分派车道分布」小节（从标题到文末）。
fn lane_section(doc: &str) -> &str {
    let head = "## 五、分派车道分布";
    let start = doc.find(head).unwrap_or_else(|| panic!("站点覆盖报告缺少小节：{head}"));
    &doc[start..]
}

/// 解析小节内的车道表：`| 车道 | 站点 | 说明 |`。
fn lane_table(section: &str) -> BTreeMap<String, u64> {
    let mut table = BTreeMap::new();
    for line in section.lines() {
        let Some(rest) = line.strip_prefix("| ") else { continue };
        let cells: Vec<&str> = rest.split('|').map(str::trim).collect();
        if cells.len() < 2 {
            continue;
        }
        let key = cells[0];
        if key == "车道" || key == "---" {
            continue;
        }
        let Ok(count) = cells[1].parse::<u64>() else { continue };
        table.insert(key.to_string(), count);
    }
    table
}

/// 读取 `- <前缀>：N 站` 这类结论行的数字。
fn bullet_number(section: &str, prefix: &str) -> Option<u64> {
    let line = section.lines().find(|line| line.trim_start().starts_with(prefix))?;
    let digit: String = line
        .split_once('：')
        .map(|(_, rest)| rest)
        .unwrap_or(line)
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digit.parse().ok()
}

/// 按策略聚合画像中的站点数。
fn strategy_sites(profiles: &Value) -> BTreeMap<String, u64> {
    let mut map: BTreeMap<String, u64> = BTreeMap::new();
    let apis = profiles["apis"].as_object().expect("profiles.apis 不是对象");
    for (_, item) in apis {
        let Some(strategy) = item["strategy"].as_str() else { continue };
        let sites = item["sites"].as_u64().unwrap_or(0);
        *map.entry(strategy.to_string()).or_default() += sites;
    }
    map
}

#[test]
fn lane_distribution_matches_profiles() {
    let doc = read("docs/审计报告/站点覆盖报告.md");
    let profiles = profiles();
    let section = lane_section(&doc);
    let table = lane_table(section);

    for key in ["native", "classified", "drpy", "js", "py", "source", "合计"] {
        assert!(table.contains_key(key), "车道表缺少行：{key}（表内容 {table:?}）");
    }

    let lane_sum: u64 = ["native", "classified", "drpy", "js", "py", "source"]
        .iter()
        .map(|key| table[*key])
        .sum();
    let totals_sites = profiles["totals"]["sites"].as_u64().expect("profiles.totals.sites 缺失");
    assert_eq!(lane_sum, totals_sites, "六车道站点之和与画像站点总数不一致");
    assert_eq!(table["合计"], lane_sum, "合计行与六车道之和不一致");

    let sites = strategy_sites(&profiles);
    let rulepack = *sites.get("rulepack").unwrap_or(&0);
    let unported = *sites.get("unported").unwrap_or(&0);
    let unsupported = *sites.get("unsupported").unwrap_or(&0);
    let script = *sites.get("script").unwrap_or(&0);

    assert_eq!(table["native"], rulepack, "native 车道与 rulepack 站点数不一致");
    assert_eq!(
        table["classified"],
        unported + unsupported,
        "classified 车道与 unported+unsupported 站点数不一致"
    );
    assert_eq!(
        table["drpy"] + table["js"] + table["py"] + table["source"],
        script,
        "脚本四车道（drpy+js+py+source）与 script 站点数不一致"
    );

    assert!(table["drpy"] > 0 && table["js"] > 0, "脚本车道计数异常（drpy/js 为 0）");
    assert_eq!(
        bullet_number(section, "- type=3 未分派："),
        Some(0),
        "type=3 存在未分派站点，请检查分派优先级"
    );
    assert_eq!(
        bullet_number(section, "- csp_ 误入脚本车道："),
        Some(0),
        "csp_ 站点误入脚本车道（第 158 轮修复被回退？见 catvod_lane_precedence）"
    );
}

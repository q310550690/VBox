//! 阶段0 命令行验证工具
//!
//! 用法示例：
//!   vbox_cli load     --config https://example.com/config.json
//!   vbox_cli categories --config <x> --site demo
//!   vbox_cli list     --config <x> --site demo --category 1 --page 1
//!   vbox_cli detail   --config <x> --site demo --id 123
//!   vbox_cli resolve  --url https://x.com/1.m3u8
//!
//! 说明：库内不硬编码任何源，`--config` 由调用方提供（公开演示 API 或本地 JSON）。

use clap::{Parser, Subcommand};
use vbox_core::config::{inspect_config, LoadOptions};
use vbox_core::{get_detail, list_categories, list_videos, load_config, resolve_play_url};

#[derive(Parser)]
#[command(name = "vbox_cli", about = "TVBox 核心引擎命令行验证")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 加载并打印配置摘要（站点数、直播数、解析器数）
    Load {
        #[arg(long)]
        config: String,
    },
    /// 检查订阅结构（单仓/线路合集 + 线路列表）
    Inspect {
        #[arg(long)]
        config: String,
    },
    /// 列出某站点的分类
    Categories {
        #[arg(long)]
        config: String,
        #[arg(long)]
        site: String,
    },
    /// 列出某分类下的视频
    List {
        #[arg(long)]
        config: String,
        #[arg(long)]
        site: String,
        #[arg(long)]
        category: String,
        #[arg(long, default_value_t = 1)]
        page: u32,
    },
    /// 获取某影视详情（含选集）
    Detail {
        #[arg(long)]
        config: String,
        #[arg(long)]
        site: String,
        #[arg(long)]
        id: String,
    },
    /// 单源搜索
    Search {
        #[arg(long)]
        config: String,
        #[arg(long)]
        site: String,
        #[arg(long)]
        keyword: String,
    },
    /// 播放解析：先取站点播放信息，再跑 parses 链
    Play {
        #[arg(long)]
        config: String,
        #[arg(long)]
        site: String,
        #[arg(long, default_value = "")]
        flag: String,
        #[arg(long)]
        url: String,
        #[arg(long, default_value = "")]
        id: String,
        #[arg(long)]
        parse: Option<String>,
    },
    /// 仅解析直链/播放页（无站点上下文）
    Resolve {
        #[arg(long)]
        url: String,
    },
    /// 检查 JVM 兼容层运行时（P0 验收：装配 + 冷启动耗时）
    #[command(name = "jvm-ping")]
    JvmPing {
        /// 重复探测次数（超过 1 次时给出中位数/最大值，用于冷启动验收）
        #[arg(long, default_value_t = 1)]
        runs: u32,
        /// 以 JSON 输出（给 CI/脚本用）
        #[arg(long)]
        json: bool,
        /// 指定运行时目录（默认按 exe 旁/workspace/VBOX_JVM_ROOT 自动定位）
        #[arg(long)]
        root: Option<String>,
    },
    /// 查看 JVM 宿主池与会话统计（P2 诊断）
    #[command(name = "jvm-pool")]
    JvmPool {
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// `jvm-convert`：把 dex/伪装 jar 转成可加载的 class jar（顺带打印转换进度）
    #[command(name = "jvm-convert")]
    JvmConvert {
        /// dex jar / 伪装扩展名的 dex 包
        #[arg(long)]
        jar: String,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// `jvm-soak`：20 次切源压测（宿主池复用 / 无进程泄漏 / 无端口泄漏）
    #[command(name = "jvm-soak")]
    JvmSoak {
        /// 夹具/真实蜘蛛 jar（可重复；两个以上会交替切换，模拟切源）
        #[arg(long = "jar", required = true)]
        jars: Vec<String>,
        /// 每个 jar 对应的 api（csp_Xxx；顺序与 --jar 对齐；缺省按 csp_TestSpider 猜）
        #[arg(long = "api")]
        apis: Vec<String>,
        /// 轮数
        #[arg(long, default_value_t = 20)]
        runs: usize,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// `jvm-shell`：查看某个 jar 的加固壳载荷状态（sha256 / index.json / 解析出的 classpath）
    JvmShell {
        /// 本地 jar 路径
        #[arg(long)]
        jar: String,
        /// 安装解密后的载荷 jar（加固壳跑一次原生桥后得到的 runtime-dex/<md5>.r2.jar）
        #[arg(long)]
        payload: Option<String>,
        /// 目标类名（可选；给了就按类解析，不给只看 any）
        #[arg(long)]
        class: Option<String>,
        /// 只打「加固壳家族指纹」：判断这个 jar 属于哪个壳家族、要不要走 shim
        #[arg(long)]
        fingerprint: bool,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 直接调用 jar 蜘蛛的一个方法（P1：验证常驻宿主协议与真实 jar）
    #[command(name = "jvm-call")]
    JvmCall {
        /// 蜘蛛 jar（可重复；多个 jar 由同一宿主加载）
        #[arg(long = "jar", required = true)]
        jars: Vec<String>,
        /// 蜘蛛类全名，如 com.github.catvod.spider.Xxx
        #[arg(long)]
        class: String,
        /// homeContent / categoryContent / detailContent / searchContent / playerContent / liveContent
        #[arg(long, default_value = "homeContent")]
        method: String,
        /// 站点 ext（原样传给 init(Context, ext)；空串表示无）
        #[arg(long, default_value = "")]
        ext: String,
        /// categoryContent 的分类 id
        #[arg(long, default_value = "")]
        filter: String,
        /// categoryContent 页码
        #[arg(long, default_value = "1")]
        page: String,
        /// categoryContent 的 extend（k=v&k2=v2）
        #[arg(long, default_value = "")]
        extend: String,
        /// detailContent 的 id 列表（逗号分隔）
        #[arg(long, default_value = "")]
        ids: String,
        /// searchContent 关键词
        #[arg(long, default_value = "")]
        keyword: String,
        /// playerContent 的 flag
        #[arg(long, default_value = "")]
        flag: String,
        /// playerContent 的播放 id / liveContent 的 url
        #[arg(long = "vid", default_value = "")]
        vid: String,
        /// playerContent 的 vipFlags（逗号分隔）
        #[arg(long, default_value = "")]
        vip_flags: String,
        /// 单次调用超时（秒）
        #[arg(long, default_value_t = 30)]
        timeout: u64,
        /// 先 __warm__ 预建实例，再调用
        #[arg(long)]
        warm: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Load { config } => {
            let cfg = load_config(&config)?;
            println!("站点数: {}", cfg.sites.len());
            for s in &cfg.sites {
                println!(
                    "  - [{}] {} (type={}, api={})",
                    s.key, s.name, s.kind, s.api
                );
            }
            println!("直播源数: {}", cfg.lives.len());
            println!("解析器数: {}", cfg.parses.len());
        }
        Cmd::Categories { config, site } => {
            let cfg = load_config(&config)?;
            let cats = list_categories(&cfg, &site)?;
            println!("{}", serde_json::to_string_pretty(&cats)?);
        }
        Cmd::List {
            config,
            site,
            category,
            page,
        } => {
            let cfg = load_config(&config)?;
            let items = list_videos(&cfg, &site, &category, page)?;
            println!("{}", serde_json::to_string_pretty(&items)?);
        }
        Cmd::Detail { config, site, id } => {
            let cfg = load_config(&config)?;
            let detail = get_detail(&cfg, &site, &id)?;
            println!("{}", serde_json::to_string_pretty(&detail)?);
        }
        Cmd::Inspect { config } => {
            let inspection = inspect_config(&config, &LoadOptions::default())?;
            println!(
                "kind: {:?}, cacheHit: {}, lines: {}",
                inspection.kind,
                inspection.cache_hit,
                inspection.lines.len()
            );
            for line in &inspection.lines {
                println!("  - {} {}", line.name.clone().unwrap_or_default(), line.url);
            }
        }
        Cmd::Search {
            config,
            site,
            keyword,
        } => {
            let cfg = load_config(&config)?;
            let site_obj = cfg
                .sites
                .iter()
                .find(|s| s.key == site)
                .ok_or_else(|| format!("站点不存在: {site}"))?;
            let items = vbox_core::search::search_site(&cfg, site_obj, &keyword, false)?;
            println!("{}", serde_json::to_string_pretty(&items)?);
        }
        Cmd::Play {
            config,
            site,
            flag,
            url,
            id,
            parse,
        } => {
            let cfg = load_config(&config)?;
            let result = vbox_core::source::get_play(
                &cfg,
                &site,
                &flag,
                if url.is_empty() { &id } else { &url },
                &url,
                parse.as_deref(),
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Cmd::Resolve { url } => {
            // resolve 不依赖配置，仅做直链判定
            let cfg = vbox_core::model::TvConfig::empty();
            match resolve_play_url(&cfg, &url) {
                Ok(r) => println!("{}", serde_json::to_string_pretty(&r)?),
                Err(e) => {
                    eprintln!("解析失败: {}", e);
                    std::process::exit(2);
                }
            }
        }
        Cmd::JvmPing { runs, json, root } => {
            jvm_ping(runs.max(1), json, root.as_deref())?;
        }
        Cmd::JvmConvert { jar, json } => {
            jvm_convert(&jar, json)?;
        }
        Cmd::JvmSoak { jars, apis, runs, json } => {
            jvm_soak(&jars, &apis, runs, json)?;
        }
        Cmd::JvmPool { json } => {
            jvm_pool(json)?;
        }
        Cmd::JvmShell { jar, payload, class, fingerprint, json } => {
            jvm_shell(&jar, payload.as_deref(), class.as_deref(), fingerprint, json)?;
        }
        Cmd::JvmCall { jars, class, method, ext, filter, page, extend, ids, keyword, flag, vid, vip_flags, timeout, warm } => {
            jvm_call(jvm_call_args(jars, class, method, ext, filter, page, extend, ids, keyword, flag, vid, vip_flags, timeout, warm))?;
        }
    }
    Ok(())
}

/// `jvm-pool`：打印宿主池与会话统计（进程内视角；常驻场景由应用进程提供）。
/// `jvm-convert`：验证 dex → jar 转换链路，并打印进度注册表的最终状态。
fn jvm_convert(jar: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let rt = vbox_jvm::Runtime::locate().ok_or("没有找到 JVM 运行时（先 npm run build:jvm && npm run fetch:jre）")?;
    let opts = vbox_jvm::DexOptions {
        cache_dir: vbox_jvm::DexOptions::default_cache(),
        timeout: std::time::Duration::from_secs(900),
    };
    let src = std::path::PathBuf::from(jar);
    let out = vbox_jvm::ensure_loadable(&rt, &src, &opts)?;
    let progress = vbox_jvm::convert_progress();
    if json {
        let value = serde_json::json!({
            "source": src.display().to_string(),
            "output": out.display().to_string(),
            "bytes": std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
            "progress": progress,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("转换完成：{}", out.display());
        if let Some(p) = progress.first() {
            println!(
                "进度：{}/{} 个 class，耗时 {} ms，ok={}{}",
                p.classes_done,
                p.total_classes,
                p.elapsed_ms,
                p.ok,
                p.error.as_ref().map(|e| format!("，错误：{e}")).unwrap_or_default()
            );
        }
    }
    Ok(())
}

/// `jvm-soak`：交替调用多个站点的 `list_categories`，验证宿主池复用与「无泄漏」。
///
/// 判据（计划 §7 P2 退出条件）：
/// - 每轮都能拿到结果（或错误但宿主没死）；
/// - 结束后 `hosts` 不超过「jar 数量」（同 jar 同 ext 必须复用，不重复 spawn）；
/// - 进程表里不留 java 子进程（`shutdown_all` 之后再查一次）。
fn jvm_soak(
    jars: &[String],
    apis: &[String],
    runs: usize,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::json;
    use vbox_core::model::{Site, TvConfig};
    use vbox_core::spider::catvod::jvm;

    if jars.is_empty() {
        return Err("--jar 至少给一个".into());
    }
    let cfg = TvConfig::default();
    let mut sites: Vec<Site> = Vec::new();
    for (i, jar) in jars.iter().enumerate() {
        let api = apis
            .get(i)
            .cloned()
            .unwrap_or_else(|| "csp_TestSpider".to_string());
        let abs = std::fs::canonicalize(jar).unwrap_or_else(|_| std::path::PathBuf::from(jar));
        // Windows canonicalize 会带 `\\?\` verbatim 前缀；放进 file:// URL 前必须剥掉，
        // 否则 JarStore 会读到 `///?/E:/...` 这种非法路径（os error 161）。
        let text = abs.display().to_string();
        let text = text
            .strip_prefix(r"\\?\UNC\")
            .map(|rest| format!("//{rest}"))
            .or_else(|| text.strip_prefix(r"\\?\").map(|rest| rest.to_string()))
            .unwrap_or(text);
        let url = format!("file:///{}", text.replace('\\', "/"));
        sites.push(Site {
            key: format!("soak{i}"),
            name: format!("soak{i}"),
            api,
            kind: 3,
            jar: url,
            ext: Some(json!("")),
            ..Default::default()
        });
    }
    let started = std::time::Instant::now();
    let mut rounds: Vec<serde_json::Value> = Vec::new();
    let mut failures = 0usize;
    for round in 0..runs.max(1) {
        let site = &sites[round % sites.len()];
        let t0 = std::time::Instant::now();
        let result = jvm::list_categories(&cfg, site);
        let ms = t0.elapsed().as_millis();
        let (ok, detail) = match &result {
            Ok(cats) => (true, format!("{} 个分类", cats.len())),
            Err(e) => (false, format!("{e:?}")),
        };
        if !ok {
            failures += 1;
        }
        rounds.push(json!({
            "round": round + 1,
            "site": site.key,
            "ok": ok,
            "ms": ms as u64,
            "detail": detail,
        }));
    }
    let elapsed = started.elapsed().as_millis();
    let stats = jvm::stats();
    let sessions = jvm::session_count();
    // 池上限判据：同 jar+ext 复用，hosts 不应超过不同 (jar,ext) 组合数。
    let hosts_ok = stats.hosts <= sites.len();
    jvm::shutdown_all();
    let hosts_after = jvm::stats().hosts;

    if json {
        let value = json!({
            "runs": runs,
            "failures": failures,
            "elapsedMs": elapsed as u64,
            "hostsOk": hosts_ok,
            "hostsAfterShutdown": hosts_after,
            "pool": {
                "hosts": stats.hosts,
                "spawns": stats.spawns,
                "hits": stats.hits,
                "calls": stats.calls,
                "failures": stats.failures,
                "sessions": sessions,
            },
            "rounds": rounds,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        for r in &rounds {
            println!(
                "#{:<3} {:<8} {:>6} ms  {}  {}",
                r["round"], r["site"], r["ms"], if r["ok"].as_bool().unwrap_or(false) { "ok" } else { "FAIL" }, r["detail"]
            );
        }
        println!(
            "压测完成：{} 轮 / {} 失败 / 总耗时 {} ms；池 hosts={}（上限判据 ≤{}：{}），spawn={} 命中={} 调用={}",
            runs,
            failures,
            elapsed,
            stats.hosts,
            sites.len(),
            if hosts_ok { "通过" } else { "不通过" },
            stats.spawns,
            stats.hits,
            stats.calls
        );
        println!("shutdown_all 之后 hosts={hosts_after}（应为 0）");
    }
    if failures > 0 || !hosts_ok || hosts_after != 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn jvm_pool(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    use vbox_core::spider::catvod::jvm;

    let stats = jvm::stats();
    let sessions = jvm::session_count();
    if json {
        let value = serde_json::json!({
            "available": jvm::available(),
            "mode": format!("{:?}", jvm::mode()),
            "maxHosts": stats.max_hosts,
            "hosts": stats.hosts,
            "spawns": stats.spawns,
            "hits": stats.hits,
            "reaped": stats.reaped,
            "evicted": stats.evicted,
            "calls": stats.calls,
            "failures": stats.failures,
            "sessions": sessions,
            "activeConversions": vbox_jvm::active_conversions(),
            "convertProgress": vbox_jvm::convert_progress()
                .into_iter()
                .take(8)
                .collect::<Vec<_>>(),
            "missingClasses": vbox_jvm::missing_classes()
                .into_iter()
                .take(20)
                .map(|m| serde_json::json!({ "class": m.class, "hits": m.hits }))
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("JVM 车道：{}（模式 {:?}）", if jvm::available() { "就绪" } else { "不可用" }, jvm::mode());
        println!(
            "宿主池：{}/{}（spawn {}，命中 {}，回收 {}，淘汰 {}）",
            stats.hosts, stats.max_hosts, stats.spawns, stats.hits, stats.reaped, stats.evicted
        );
        println!("调用：{}（失败 {}）；播放会话：{}", stats.calls, stats.failures, sessions);
        println!("调用：{}（失败 {}）；播放会话：{}", stats.calls, stats.failures, sessions);
        let missing = vbox_jvm::missing_classes();
        if missing.is_empty() {
            println!("缺类诊断：暂无（尚未出现 class_not_found）");
        } else {
            println!("缺类诊断：{} 个类待补（按命中次数）", missing.len());
            for m in missing.iter().take(10) {
                println!("  - {} ×{}", m.class, m.hits);
            }
        }
    }
    Ok(())
}

/// `jvm-ping`：定位运行时 → 跑 N 次 `SpiderHost --probe` → 打印装配与冷启动耗时。
/// `jvm-shell`：加固壳载荷诊断。
fn jvm_shell(
    jar: &str,
    payload_jar: Option<&str>,
    class: Option<&str>,
    fingerprint: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = std::path::Path::new(jar);
    if fingerprint {
        let fp = vbox_jvm::shell_fingerprint(path)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&fp)?);
        } else {
            println!("jar      : {}", path.display());
            println!("{}", fp.summary());
            if !fp.markers.is_empty() {
                println!("markers  : {}", fp.markers.join(", "));
            }
            for n in &fp.notes {
                println!("提示     : {n}");
            }
        }
        return Ok(());
    }
    let sha = vbox_jvm::sha256_file(path)?;
    let cache = vbox_jvm::DexOptions::default_cache();
    if let Some(payload) = payload_jar {
        let installed = vbox_jvm::install_payload(&cache, &sha, std::path::Path::new(payload))?;
        if !json {
            println!("已安装载荷 : {}", installed.display());
        }
    }
    let dir = cache.join("payloads").join(&sha);
    let index = dir.join("index.json");
    let payload = vbox_jvm::resolve_payload(&cache, &sha, class);

    if json {
        let out = serde_json::json!({
            "jar": path.display().to_string(),
            "sha256": sha,
            "payloadDir": dir.display().to_string(),
            "installedPayload": dir.join("inner.jar").display().to_string(),
            "hasIndex": index.exists(),
            "class": class,
            "resolved": payload.as_ref().map(|p| p.paths.iter().map(|x| x.display().to_string()).collect::<Vec<_>>()),
            "hint": if payload.is_none() { Some(vbox_jvm::unsupported_hint(&sha)) } else { None },
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("jar      : {}", path.display());
    println!("sha256   : {sha}");
    println!("payload  : {}", dir.display());
    println!("index    : {}", if index.exists() { "有" } else { "无" });
    match payload {
        Some(p) => {
            println!("resolved : {} 个条目", p.paths.len());
            for x in p.paths {
                println!("  - {}", x.display());
            }
        }
        None => println!("resolved : 无\n提示     : {}", vbox_jvm::unsupported_hint(&sha)),
    }
    Ok(())
}

fn jvm_ping(runs: u32, json: bool, root: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let rt = match root {
        Some(dir) => vbox_jvm::Runtime::at(dir),
        None => vbox_jvm::Runtime::locate().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "找不到 JVM 运行时：先跑 `node scripts/build-jvm.mjs` 与 `scripts/fetch-jre.ps1`，或用 --root 指定",
            )
        })?,
    };

    let mut samples: Vec<u128> = Vec::with_capacity(runs as usize);
    let mut probe = None;
    for i in 0..runs {
        let started = std::time::Instant::now();
        let p = rt.probe()?;
        let ms = started.elapsed().as_millis();
        samples.push(ms);
        if !json {
            println!("run {}: {} ms", i + 1, ms);
        }
        if probe.is_none() {
            probe = Some(p);
        }
    }
    let probe = probe.expect("runs >= 1");
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2];
    let max = *sorted.last().unwrap();

    if json {
        let payload = serde_json::json!({
            "runtime": rt.root().display().to_string(),
            "healthy": probe.is_healthy(),
            "missing": probe.missing(),
            "probe": probe,
            "samplesMs": samples,
            "medianMs": median,
            "maxMs": max,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("运行时: {}", rt.root().display());
        println!("java: {}（{}）", probe.java, probe.vm);
        println!(
            "class 版本: {} {}  stubs: {}  宿主: {}",
            probe.class_version,
            if probe.class_version == 61 { "✓" } else { "✗ 期望 61" },
            if probe.stubs { "✓" } else { "✗" },
            probe.host_jar
        );
        let libs: Vec<String> = probe
            .libs
            .iter()
            .map(|(k, v)| format!("{}{}", k, if *v { "" } else { "(缺)" }))
            .collect();
        println!("libs: {}", libs.join(" "));
        if !probe.is_healthy() {
            println!("缺项: {:?}", probe.missing());
        }
        println!(
            "冷启动: {} 次，中位 {} ms / 最大 {} ms（目标 < 2500 ms）",
            runs, median, max
        );
    }
    if !probe.is_healthy() {
        return Err("JVM 运行时装配不完整".into());
    }
    Ok(())
}

struct JvmCallArgs {
    jars: Vec<String>,
    class: String,
    method: String,
    ext: String,
    filter: String,
    page: String,
    extend: String,
    ids: String,
    keyword: String,
    flag: String,
    vid: String,
    vip_flags: String,
    timeout: u64,
    warm: bool,
}

#[allow(clippy::too_many_arguments)]
fn jvm_call_args(
    jars: Vec<String>,
    class: String,
    method: String,
    ext: String,
    filter: String,
    page: String,
    extend: String,
    ids: String,
    keyword: String,
    flag: String,
    vid: String,
    vip_flags: String,
    timeout: u64,
    warm: bool,
) -> JvmCallArgs {
    JvmCallArgs { jars, class, method, ext, filter, page, extend, ids, keyword, flag, vid, vip_flags, timeout, warm }
}

/// `jvm-call`：拉起常驻宿主 → （可选预热）→ 调用一个方法 → 打印结果。
fn jvm_call(a: JvmCallArgs) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;

    let rt = vbox_jvm::Runtime::locate().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "找不到 JVM 运行时：先跑 `node scripts/build-jvm.mjs` 与 `scripts/fetch-jre.ps1`",
        )
    })?;
    let jars: Vec<std::path::PathBuf> = a.jars.iter().map(std::path::PathBuf::from).collect();
    let mut opts = vbox_jvm::HostOptions::new(jars).with_timeout(Duration::from_secs(a.timeout));
    // 诊断/调优逃生门（与 vbox_core 车道保持同一套环境变量）
    if let Ok(v) = std::env::var("VBOX_JVM_ARGS") {
        opts.extra_jvm_args.extend(v.split_whitespace().map(|s| s.to_string()));
    }
    if let Ok(v) = std::env::var("VBOX_JVM_HEAP_MB") {
        if let Ok(mb) = v.trim().parse::<u32>() {
            opts.heap_mb = mb;
        }
    }
    let started = std::time::Instant::now();
    let mut host = vbox_jvm::HostProcess::spawn(&rt, &opts)?;
    let startup_ms = started.elapsed().as_millis();
    if a.warm {
        host.warm(&a.class, &a.ext, Duration::from_secs(a.timeout))?;
    }
    let ids: Vec<String> = a.ids.split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    let vips: Vec<String> = a.vip_flags.split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    let call = match a.method.as_str() {
        "homeContent" => vbox_jvm::Call::home_content(a.class.clone(), &a.ext),
        "categoryContent" => {
            vbox_jvm::Call::category_content(a.class.clone(), &a.ext, &a.filter, &a.page, &a.extend)
        }
        "detailContent" => vbox_jvm::Call::detail_content(a.class.clone(), &a.ext, &ids),
        "searchContent" => vbox_jvm::Call::search_content(a.class.clone(), &a.ext, &a.keyword, "1"),
        "playerContent" => vbox_jvm::Call::player_content(a.class.clone(), &a.ext, &a.flag, &a.vid, &vips),
        "liveContent" => vbox_jvm::Call::new(a.class.clone(), "liveContent", vec![a.ext.clone(), a.vid.clone()]),
        other => return Err(format!("不支持的方法：{other}").into()),
    };
    let reply = host.call(&call, Duration::from_secs(a.timeout))?;
    // 加固壳首次解密成功后：auto 安装内层载荷（同 jar 下次进源走纯 Java shim）。
    if let Some(inner) = host.harvest_payload() {
        println!("已自动安装内层载荷：{}", inner.display());
    }
    println!("宿主启动: {} ms（pp={:?}）", startup_ms, host.pp());
    println!("调用: {} {} → {} ms, ok={}", a.class, a.method, reply.elapsed.as_millis(), reply.ok);
    if std::env::var("VBOX_RAW").is_ok() {
        println!("{}", reply.data);
    } else if reply.data.chars().count() > 4000 {
        let head: String = reply.data.chars().take(4000).collect();
        println!("{}…（共 {} 字符，已截断）", head, reply.data.chars().count());
    } else {
        println!("{}", reply.data);
    }
    let show_stderr = !reply.ok || std::env::var("VBOX_HOST_STDERR").is_ok();
    if show_stderr {
        for line in host.stderr_tail(if reply.ok { 25 } else { 15 }) {
            eprintln!("stderr| {line}");
        }
    }
    host.shutdown(Duration::from_secs(5));
    Ok(())
}

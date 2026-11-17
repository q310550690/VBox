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
    }
    Ok(())
}

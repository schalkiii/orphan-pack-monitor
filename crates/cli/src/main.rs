//! orphan-audit CLI —— 审计全局 CLI 工具的使用情况，找出长期未用的孤儿包。
//! 主路线 B（最近调用追踪），辅助路线 A 由 core 的静态审计接口提供。

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use orphan_audit_core::model::Manager;
use orphan_audit_core::{actions, audit, model::AuditConfig};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "orphan-audit",
    version,
    about = "Find global CLI tools you installed and stopped using — with evidence",
    long_about = None
)]
struct Cli {
    /// 子命令（默认 audit）
    #[command(subcommand)]
    command: Option<Commands>,

    /// 「多久内执行过仍算在用」：30d / 6w / 6mo / 1y / 纯数字天数
    #[arg(long, global = true, default_value = "30d")]
    since: String,

    /// 覆盖 active 判定阈值（默认按历史规模自适应）
    #[arg(long, global = true)]
    active_hits: Option<u32>,

    /// 忽略的包名或二进制名（可重复）
    #[arg(long, global = true)]
    ignore: Vec<String>,

    /// 只审计指定管理器（可重复）
    #[arg(long = "manager", global = true, value_enum)]
    managers: Vec<ManagerArg>,

    /// 指定数据库路径
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// JSON 输出
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 执行一次完整审计（默认）
    Audit,
    /// 显示统计摘要
    Status,
    /// 生成可审阅的卸载脚本（默认输出到 stdout）
    Clean {
        /// 写入文件
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// 对比两次快照的漂移
    Drift {
        #[arg(default_value = "")]
        old: String,
        #[arg(default_value = "")]
        new: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ManagerArg {
    Cargo,
    Npm,
    Pip,
    Uv,
    Pipx,
    Go,
    Brew,
    Scoop,
    Winget,
}

impl From<ManagerArg> for Manager {
    fn from(m: ManagerArg) -> Self {
        match m {
            ManagerArg::Cargo => Manager::Cargo,
            ManagerArg::Npm => Manager::Npm,
            ManagerArg::Pip => Manager::Pip,
            ManagerArg::Uv => Manager::Uv,
            ManagerArg::Pipx => Manager::Pipx,
            ManagerArg::Go => Manager::Go,
            ManagerArg::Brew => Manager::Brew,
            ManagerArg::Scoop => Manager::Scoop,
            ManagerArg::Winget => Manager::Winget,
        }
    }
}

/// 解析 "30d" / "6w" / "6mo" / "1y" / "45" 为天数。
fn parse_since(s: &str) -> Result<u32> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        anyhow::bail!("empty --since value");
    }
    // 用后缀匹配，先匹配多字符单位 "mo"，再匹配单字符；纯数字默认按天。
    let (num, multiplier) = if let Some(rest) = s.strip_suffix("mo") {
        (rest.trim(), 30u32)
    } else if let Some(rest) = s.strip_suffix('d') {
        (rest.trim(), 1)
    } else if let Some(rest) = s.strip_suffix('w') {
        (rest.trim(), 7)
    } else if let Some(rest) = s.strip_suffix('y') {
        (rest.trim(), 365)
    } else {
        (s.as_str(), 1) // 纯数字 -> 天
    };
    let n: u32 = num.parse()?;
    Ok((n * multiplier).max(1))
}

fn default_db() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("orphan-audit")
        .join("orphan-audit.db")
}

/// 快照漂移：返回（新增, 移除）包名集合。a=旧快照，b=新快照。
fn diff_names(
    a: &std::collections::BTreeSet<String>,
    b: &std::collections::BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    (
        b.difference(a).cloned().collect(),
        a.difference(b).cloned().collect(),
    )
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let since_days = parse_since(&cli.since)?;
    let config = AuditConfig {
        since_days,
        active_hits: cli.active_hits,
        ignore: cli.ignore.clone(),
        managers: cli.managers.iter().map(|m| Manager::from(*m)).collect(),
    };
    let db = cli.db.clone().unwrap_or_else(default_db);

    match cli.command.unwrap_or(Commands::Audit) {
        Commands::Audit => {
            let report = audit(&config)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report.results)?);
            } else {
                print!("{}", actions::generate_report(&report.results, since_days));
                if !report.warnings.is_empty() {
                    eprintln!("\nWarnings:");
                    for w in &report.warnings {
                        eprintln!("  - {w}");
                    }
                }
                let sources = if report.history_sources.is_empty() {
                    "none".to_string()
                } else {
                    report.history_sources.join(", ")
                };
                eprintln!(
                    "\nHistory sources: {} | entries: {} | ETW: {}",
                    sources,
                    report.history_total,
                    if report.tracker.available {
                        "available"
                    } else {
                        "unavailable (degraded)"
                    }
                );
            }
        }
        Commands::Status => {
            let report = audit(&config)?;
            let mut counts: std::collections::BTreeMap<&str, u32> =
                std::collections::BTreeMap::new();
            for r in &report.results {
                *counts.entry(r.verdict.as_str()).or_default() += 1;
            }
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&counts)?);
            } else {
                println!("Packages inspected: {}", report.results.len());
                for (k, v) in counts {
                    println!("  {:<12} {}", k, v);
                }
                println!("History entries : {}", report.history_total);
                println!(
                    "ETW tracker     : {}",
                    if report.tracker.available {
                        "available"
                    } else {
                        "unavailable (degraded)"
                    }
                );
                println!("Degraded mode   : {}", report.degraded);
            }
        }
        Commands::Clean { output } => {
            let report = audit(&config)?;
            let script = actions::generate_script(&report.results, since_days);
            match output {
                Some(p) => {
                    std::fs::write(&p, &script)?;
                    eprintln!(
                        "Wrote cleanup script to {} (review before running)",
                        p.display()
                    );
                }
                None => print!("{script}"),
            }
        }
        Commands::Drift { old, new } => {
            let st = orphan_audit_core::store::Store::open(&db)?;
            let ids = st.snapshot_ids()?;
            if old.is_empty() || new.is_empty() {
                println!("Available snapshots: {}", ids.join(", "));
                return Ok(());
            }
            // 不存在的快照名按空集 diff 会把所有包显示为 removed，必须先校验
            for s in [&old, &new] {
                if !ids.contains(s) {
                    anyhow::bail!(
                        "snapshot '{s}' not found; available snapshots: {}",
                        ids.join(", ")
                    );
                }
            }
            use orphan_audit_core::model::Verdict;
            for v in [Verdict::Active, Verdict::Unused] {
                let a: std::collections::BTreeSet<_> =
                    st.verdict_names(&old, v)?.into_iter().collect();
                let b: std::collections::BTreeSet<_> =
                    st.verdict_names(&new, v)?.into_iter().collect();
                let (added, removed) = diff_names(&a, &b);
                println!("{}: +{} -{}", v.as_str(), added.len(), removed.len());
                for x in &added {
                    println!("  + {x}");
                }
                for x in &removed {
                    println!("  - {x}");
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{diff_names, parse_since};

    #[test]
    fn since_units() {
        assert_eq!(parse_since("30d").unwrap(), 30);
        assert_eq!(parse_since("6w").unwrap(), 42);
        assert_eq!(parse_since("6mo").unwrap(), 180);
        assert_eq!(parse_since("1y").unwrap(), 365);
        assert_eq!(parse_since("45").unwrap(), 45);
        assert_eq!(parse_since("0d").unwrap(), 1);
        assert!(parse_since("10x").is_err());
    }

    #[test]
    fn diff_names_reports_added_and_removed() {
        let a: std::collections::BTreeSet<String> =
            ["keep", "gone"].iter().map(|s| s.to_string()).collect();
        let b: std::collections::BTreeSet<String> =
            ["keep", "new"].iter().map(|s| s.to_string()).collect();
        let (added, removed) = diff_names(&a, &b);
        assert_eq!(added, vec!["new".to_string()]);
        assert_eq!(removed, vec!["gone".to_string()]);
    }

    #[test]
    fn json_output_shape_has_key_fields() {
        // --json 输出的最小形状：verdict 小写、package.name 保留
        let mut p = orphan_audit_core::model::Package::new(
            "dusty",
            orphan_audit_core::model::Manager::Cargo,
        );
        p.binaries = vec!["dusty".into()];
        let r = orphan_audit_core::model::VerdictResult {
            package: p,
            verdict: orphan_audit_core::model::Verdict::Unused,
            interactive_hits: 0,
            last_exec_days_ago: None,
            in_shell_config: false,
            newly_installed: false,
            evidence: vec![],
        };
        let v: serde_json::Value = serde_json::to_value(vec![r]).unwrap();
        assert_eq!(
            v[0]["package"]["name"].as_str(),
            Some("dusty"),
            "package.name must serialize"
        );
        assert_eq!(v[0]["verdict"].as_str(), Some("unused"));
        assert_eq!(v[0]["interactive_hits"].as_u64(), Some(0));
    }
}

//! orphan-audit CLI —— 审计全局安装包的使用情况，找出长期未用的孤儿包。
//! 主路线 B（最近调用追踪），辅助路线 A 由 core 的静态审计接口提供。

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use orphan_audit_core::hooks;
use orphan_audit_core::model::Manager;
use orphan_audit_core::{actions, audit_with_paths, model::AuditConfig};
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

    /// 指定数据库路径（audit/status/clean/drift 均生效）
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// JSON 输出（audit/status 生效——drift 只输出文本；audit 的 stdout 只含
    /// results，warnings 经 stderr 另行输出，ETW 降级状态不在 stdout JSON 中）
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
    /// 对比两次快照，列出新增与移除的包（只对比 active/unused 两档：
    /// Rare 等中间档随窗口参数波动，列入会淹没真实增删）
    Drift {
        #[arg(default_value = "")]
        old: String,
        #[arg(default_value = "")]
        new: String,
    },
    /// 管理使用钩子（python .pth / node NODE_OPTIONS，捕捉 import 级使用证据）
    Hooks {
        /// install / uninstall / status
        #[arg(value_enum)]
        action: HookAction,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum HookAction {
    Install,
    Uninstall,
    Status,
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
    // 乘法必须显式检查：溢出在 release 下回绕成小值，观察窗口静默失真
    let days = n
        .checked_mul(multiplier)
        .ok_or_else(|| anyhow::anyhow!("--since window too large: {s}"))?;
    Ok(days.max(1))
}

/// `hooks status` 的文本渲染（数据来自 core `hooks::status_info`，与 GUI 面板
/// 同一数据源）。日志状态三分支与审计警告同一语义（R14：缺失 ≠ 被隔离）。
fn hooks_status() -> Result<()> {
    let st = hooks::status_info();
    let mut with_pth = 0usize;
    for (interp, has) in &st.interpreters {
        if *has {
            with_pth += 1;
        }
        println!(
            "  {:<50} {}",
            interp.display(),
            if *has { "hooked" } else { "not hooked" }
        );
    }
    println!(
        "python interpreters hooked : {with_pth}/{}",
        st.interpreters.len()
    );
    println!(
        "tool venvs hooked          : {}/{}",
        st.tool_venvs.0, st.tool_venvs.1
    );
    println!(
        "node NODE_OPTIONS          : {}",
        if st.node_hooked {
            "hooked"
        } else {
            "not hooked"
        }
    );
    if let Some((log, state)) = &st.log {
        match state {
            hooks::ImportLogState::Present(n) => println!(
                "import log                 : {} ({} bytes)",
                log.display(),
                n
            ),
            hooks::ImportLogState::Quarantined(shadow) => println!(
                "import log                 : {} (quarantined by security software: {})",
                log.display(),
                shadow.display()
            ),
            hooks::ImportLogState::Missing => println!(
                "import log                 : {} (not created yet)",
                log.display()
            ),
        }
    }
    Ok(())
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
    let db = cli
        .db
        .clone()
        .unwrap_or_else(orphan_audit_core::default_db_path);

    match cli.command.unwrap_or(Commands::Audit) {
        Commands::Hooks { action } => match action {
            HookAction::Install => println!("{}", hooks::install()?),
            HookAction::Uninstall => println!("{}", hooks::uninstall()?),
            HookAction::Status => hooks_status()?,
        },
        Commands::Audit => {
            let report = audit_with_paths(&config, &db)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report.results)?);
                // 机器消费方也要看到降级（R12/R13 的「证据源死亡必须显式」）：
                // stdout 保持纯 results（help 文档化的形状，jq 可解析），
                // warnings 走 stderr——与文本模式同一通道、同一内容
                for w in &report.warnings {
                    eprintln!("warning: {w}");
                }
            } else {
                print!(
                    "{}",
                    actions::generate_report(&report.results, &report.orphan_shims, since_days)
                );
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
                if !report.detected_envs.is_empty() {
                    // 多环境检测：让用户看到审计覆盖了哪些 node prefix / python
                    eprintln!("Detected environments:");
                    for env in &report.detected_envs {
                        eprintln!("  - {}", env.label());
                    }
                }
            }
        }
        Commands::Status => {
            let report = audit_with_paths(&config, &db)?;
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
            let report = audit_with_paths(&config, &db)?;
            let script =
                actions::generate_script(&report.results, &report.orphan_shims, since_days);
            match output {
                Some(p) => {
                    // write_script_file 带 UTF-8 BOM：PowerShell 5.1 对无 BOM 的
                    // UTF-8 按 ANSI 解析，脚本里的中文路径会乱码
                    actions::write_script_file(&p, &script)?;
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
            for diff in st.drift_diff(&old, &new)? {
                println!(
                    "{}: +{} -{}",
                    diff.verdict.as_str(),
                    diff.added.len(),
                    diff.removed.len()
                );
                for x in &diff.added {
                    println!("  + {x}");
                }
                for x in &diff.removed {
                    println!("  - {x}");
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_since;

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
    fn since_overflow_is_error_not_wraparound() {
        // 溢出必须报错：release 下回绕会把观察窗口静默变成错误的小值
        assert!(parse_since("99999999y").is_err());
        assert!(parse_since("4294967295w").is_err()); // ×7 回绕 u32
        assert!(parse_since("4294967295d").is_ok()); // ×1 不溢出，合法边界
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

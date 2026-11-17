//! orphan-audit-core —— 孤儿 CLI 工具检测引擎。
//!
//! 分层：
//!   inventory  采集全局包清单与包->二进制映射
//!   signals    shell 历史 / shell 配置 / atime / ETW 归一化
//!   verdict    五档判定引擎（核心知识产权）
//!   store      SQLite 持久化与历史快照
//!   actions    卸载脚本与报告生成
//!   etw        Windows 进程执行追踪（核心差异化）
//!
//! 设计约束：core 不依赖任何 GUI 框架，可独立作为 CLI / 库使用。

pub mod actions;
pub mod etw;
pub mod inventory;
pub mod model;
pub mod signals;
pub mod store;
pub mod verdict;

use anyhow::Result;
use chrono::{Utc};
use model::{AuditConfig, UsageEvent, VerdictResult};
use std::path::{Path, PathBuf};

/// 一次完整审计的编排结果与诊断信息。
#[derive(Debug, Clone)]
pub struct AuditReport {
    pub results: Vec<VerdictResult>,
    pub config: AuditConfig,
    /// 采集/解析过程的非致命警告（如某管理器不可用）。
    pub warnings: Vec<String>,
    /// 实际使用的历史数据源。
    pub history_sources: Vec<String>,
    /// 历史命令总数（自适应阈值依据）。
    pub history_total: u32,
    /// ETW 追踪器状态。
    pub tracker: etw::TrackerStatus,
    /// 是否已降级（无 ETW，仅用弱信号）。
    pub degraded: bool,
}

/// 执行一次完整审计（默认数据目录）。
pub fn audit(config: &AuditConfig) -> Result<AuditReport> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("orphan-audit");
    audit_with_paths(config, &data_dir.join("orphan-audit.db"))
}

/// 可指定数据库路径的审计入口（便于测试与 GUI 复用）。
pub fn audit_with_paths(config: &AuditConfig, db_path: &Path) -> Result<AuditReport> {
    let mut warnings = Vec::new();

    // 1. 采集包清单
    let (packages, collect_warnings) = inventory::collect_all();
    warnings.extend(collect_warnings);

    // 2. 采集信号
    let history = signals::collect_history();
    let shell_config_index = signals::collect_shell_config();

    // 2b. ETW 事件（若可用）
    let tracker = etw::probe_status();
    let mut all_events: Vec<UsageEvent> = history.events.clone();
    let degraded = !tracker.available;
    if let Some(log) = etw::default_log_path() {
        match etw::read_etw_log(&log) {
            Ok(evs) => all_events.extend(evs),
            Err(e) => warnings.push(format!("[etw] log read failed: {e:#}")),
        }
    }

    // 2c. atime 探测（弱信号）—— 对每个包的可执行文件取最新访问时间
    if let Some(atime_on) = signals::atime_enabled() {
        if atime_on {
            let bin_paths: Vec<PathBuf> = packages
                .iter()
                .filter_map(|p| {
                    p.install_path.as_ref().and_then(|dir| {
                        p.binaries
                            .first()
                            .map(|b| dir.join(format!("{b}.exe")))
                            .or_else(|| p.binaries.first().map(|b| dir.join(b)))
                    })
                })
                .collect();
            for p in bin_paths {
                if let Some(ev) = signals::probe_atime(std::slice::from_ref(&p)) {
                    // probe_atime 返回的 binary 占位为 "atime"，需绑定到具体命令名
                    let mut ev = ev;
                    ev.binary = crate::inventory::strip_exe(
                        &p.file_name().unwrap_or_default().to_string_lossy(),
                    )
                    .to_string();
                    all_events.push(ev);
                }
            }
        } else {
            warnings.push(
                "[atime] NTFS last-access updates are disabled; atime signal skipped".into(),
            );
        }
    } else {
        warnings.push("[atime] could not determine last-access state; atime signal skipped".into());
    }

    // 3. 判定
    let results = verdict::evaluate_all(
        &packages,
        &all_events,
        &shell_config_index,
        config,
        history.total_entries,
        Utc::now(),
    );

    // 4. 持久化（失败不致命，仅警告）
    match store::Store::open(db_path) {
        Ok(st) => {
            let rows: Vec<store::UsageEventRow> = all_events.iter().map(store::UsageEventRow::from).collect();
            if let Err(e) = st.replace_inventory(&packages, &rows) {
                warnings.push(format!("[store] inventory save failed: {e:#}"));
            }
            let snapshot = Utc::now().format("%Y-%m").to_string();
            if let Err(e) = st.save_snapshot(&snapshot, &results) {
                warnings.push(format!("[store] snapshot save failed: {e:#}"));
            }
            let _ = st.sync_ignores(config);
        }
        Err(e) => warnings.push(format!("[store] open failed: {e:#}")),
    }

    if degraded {
        warnings.push(
            "[signal] ETW unavailable — verdicts rely on shell history + atime only; run elevated for precise execution tracking".into(),
        );
    }

    Ok(AuditReport {
        results,
        config: config.clone(),
        warnings,
        history_sources: history.sources_used,
        history_total: history.total_entries,
        tracker,
        degraded,
    })
}

/// 仅从已持久化的库生成报告（离线模式，GUI 可用）。
pub fn report_from_store(db_path: &Path, config: &AuditConfig) -> Result<AuditReport> {
    let st = store::Store::open(db_path)?;
    let packages = st.packages()?;
    let empty_events: Vec<UsageEvent> = Vec::new();
    let empty_index = std::collections::HashMap::new();
    let results = verdict::evaluate_all(
        &packages,
        &empty_events,
        &empty_index,
        config,
        1000,
        Utc::now(),
    );
    Ok(AuditReport {
        results,
        config: config.clone(),
        warnings: vec!["[offline] loaded from store; re-run audit to refresh signals".into()],
        history_sources: vec![],
        history_total: 0,
        tracker: etw::probe_status(),
        degraded: true,
    })
}

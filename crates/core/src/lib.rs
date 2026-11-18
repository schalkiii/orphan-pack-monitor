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
use chrono::Utc;
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

    // 1. 采集包清单 + 兜底填充安装时间（采集器拿不到 installed_at 的管理器，
    //    用二进制文件创建时间近似；新装观察期依赖该字段）
    let (mut packages, collect_warnings) = inventory::collect_all();
    warnings.extend(collect_warnings);
    fill_installed_at(&mut packages);

    // 2. 采集信号
    let mut history = signals::collect_history();
    let shell_config_index = signals::collect_shell_config();

    // 2b. ETW 事件（消费端视角）：先读日志——窗口内读到事件（如常驻采集在写、
    // 当前终端未提权）即视为可用；读不到才回退环境探测（未提权/非 Windows）
    // 事件所有权移入 all_events，避免整表克隆；history.total_entries 仍可用于阈值。
    let mut all_events: Vec<UsageEvent> = std::mem::take(&mut history.events);
    let mut etw_events = 0usize;
    if let Some(log) = etw::default_log_path() {
        // 只取 since 窗口内事件（放宽 1 天容差）：常驻采集后日志无限增长，
        // 窗口外 ETW 事件不影响判定，全量读入只会拖慢审计并撑大 usage_events 表
        let since = Utc::now() - chrono::Duration::days(config.since_days as i64 + 1);
        match etw::read_etw_log_since(&log, Some(since)) {
            Ok(evs) => {
                etw_events = evs.len();
                all_events.extend(evs);
            }
            Err(e) => warnings.push(format!("[etw] log read failed: {e:#}")),
        }
    }
    let tracker = tracker_from_log_events(etw_events);
    let degraded = !tracker.available;

    // 2c. atime 探测（弱信号）—— 按管理器推断 bin 路径，逐 bin 取最新访问时间
    if let Some(atime_on) = signals::atime_enabled() {
        if atime_on {
            for p in &packages {
                for b in &p.binaries {
                    let candidates = atime_candidate_paths(p, b);
                    if let Some(ev) = signals::probe_atime(&candidates, b) {
                        all_events.push(ev);
                    }
                }
            }
        } else {
            warnings
                .push("[atime] NTFS last-access updates are disabled; atime signal skipped".into());
        }
    } else {
        warnings.push("[atime] could not determine last-access state; atime signal skipped".into());
    }

    // 3. 判定
    let mut results = verdict::evaluate_all(
        &packages,
        &all_events,
        &shell_config_index,
        config,
        history.total_entries,
        Utc::now(),
    );

    // 4. 持久化（失败不致命，仅警告）。漂移富化在 save_snapshot 之前：
    // 快照里也要带上漂移证据，且查询只看更早的快照（当前快照被排除）
    match store::Store::open(db_path) {
        Ok(st) => {
            let rows: Vec<store::UsageEventRow> =
                all_events.iter().map(store::UsageEventRow::from).collect();
            if let Err(e) = st.replace_inventory(&packages, &rows) {
                warnings.push(format!("[store] inventory save failed: {e:#}"));
            }
            let snapshot = Utc::now().format("%Y-%m").to_string();
            enrich_with_drift(&mut results, &st, &snapshot);
            if let Err(e) = st.save_snapshot(&snapshot, &results) {
                warnings.push(format!("[store] snapshot save failed: {e:#}"));
            }
        }
        Err(e) => warnings.push(format!("[store] open failed: {e:#}")),
    }

    if degraded {
        warnings.push(
            "[signal] no ETW events in the audit window — verdicts rely on shell history + \
             atime only (run the bundled etw-capture as administrator, or `etw-capture \
             install` for resident capture; see README)"
                .into(),
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

/// 消费端视角的 ETW 可用性：窗口内读到事件即视为可用——常驻采集写入日志后，
/// **未提权**的审计也能消费事件（旧逻辑只看 probe 的提权检查，会在这种情况下
/// 误报「no ETW events available」）。读不到事件才回退环境探测（提权/平台检查）。
fn tracker_from_log_events(etw_events: usize) -> etw::TrackerStatus {
    if etw_events > 0 {
        etw::TrackerStatus {
            available: true,
            reason: None,
        }
    } else {
        etw::probe_status()
    }
}

/// 仅从已持久化的库生成报告（离线模式，GUI 可用）。
/// 用库中真实事件重建判定——若用空事件集，所有包必然判 Unused（误导清理决策）。
pub fn report_from_store(db_path: &Path, config: &AuditConfig) -> Result<AuditReport> {
    let st = store::Store::open(db_path)?;
    let packages = st.packages()?;
    let events = st.events()?;
    // shell 配置索引不做持久化（其价值在实时采集），离线重估只依赖事件信号
    let empty_index = std::collections::HashMap::new();
    // history_total 未单独持久化，用库中历史事件数近似（只影响自适应 active 阈值）
    let history_total: u32 = events
        .iter()
        .filter(|e| e.source == model::SignalSource::History)
        .map(|e| e.count)
        .sum();
    let results = verdict::evaluate_all(
        &packages,
        &events,
        &empty_index,
        config,
        history_total,
        Utc::now(),
    );
    Ok(AuditReport {
        results,
        config: config.clone(),
        warnings: vec!["[offline] loaded from store; re-run audit to refresh signals".into()],
        history_sources: vec![],
        history_total,
        tracker: etw::probe_status(),
        degraded: true,
    })
}

/// 组装某包某 bin 的 atime 探测候选路径。
/// 各管理器 install_path 语义不同（cargo 即 bin 目录、npm 是 node_modules/<pkg>、
/// pip 是 site-packages、uv/pipx 是 venv 根），bin 目录须按管理器推断；
/// 不存在的路径由 probe_atime 静默跳过。
fn atime_candidate_paths(p: &model::Package, bin: &str) -> Vec<PathBuf> {
    let Some(root) = p.install_path.as_ref() else {
        return Vec::new();
    };
    let dirs: Vec<PathBuf> = match p.manager {
        // cargo: install_path 即 ~/.cargo/bin
        model::Manager::Cargo => vec![root.clone()],
        // npm: install_path 是 <prefix>/node_modules/<pkg>；shim 在 prefix 根（Windows）或 <prefix>/bin（Unix）
        model::Manager::Npm => root
            .parent()
            .and_then(Path::parent)
            .map(|prefix| vec![prefix.to_path_buf(), prefix.join("bin")])
            .unwrap_or_default(),
        // pip: install_path 是 <venv>/Lib/site-packages（Unix 为 <venv>/lib/pythonX.Y/site-packages）；
        // 入口在 venv 根的 Scripts/（Windows）或 bin/（Unix）
        model::Manager::Pip => root
            .parent()
            .and_then(Path::parent)
            .map(|venv| vec![venv.join("Scripts"), venv.join("bin")])
            .unwrap_or_default(),
        // uv/pipx: install_path 即 venv 根（uv 来自 `uv tool list` 的 (path) 段，
        // pipx 按 <PIPX_HOME>/venvs/<name> 推断）；入口同样在 Scripts/ 与 bin/
        model::Manager::Uv | model::Manager::Pipx => {
            vec![root.join("Scripts"), root.join("bin")]
        }
        // 其余管理器（go/brew/scoop/winget）采集层未记录 install_path 或无固定布局，跳过
        _ => Vec::new(),
    };
    dirs.into_iter()
        .flat_map(|d| {
            [
                d.join(format!("{bin}.exe")),
                d.join(format!("{bin}.cmd")),
                d.join(bin),
            ]
        })
        .collect()
}

/// 兜底填充安装时间：采集器未提供 installed_at 的包，用其任一二进制文件的
/// 最早创建时间近似（安装即写文件）。只填空缺，不覆盖采集器的精确值；
/// 拿不到创建时间的包保持 None（不视为新装）。
fn fill_installed_at(packages: &mut [model::Package]) {
    for p in packages.iter_mut() {
        if p.installed_at.is_some() {
            continue;
        }
        let earliest = p
            .binaries
            .iter()
            .flat_map(|b| atime_candidate_paths(p, b))
            .filter_map(|path| std::fs::metadata(&path).ok()?.created().ok())
            .min();
        if let Some(c) = earliest {
            p.installed_at = Some(c.into());
        }
    }
}

/// 漂移富化：连续 `DRIFT_MIN_SNAPSHOTS` 个历史快照均判 unused 的包，
/// 在证据链追加漂移证据。判定档位不变（unused 仍是 unused），但把
/// 「本月恰好没跑」与「长期没人用」区分开——后者清理置信度显著更高。
fn enrich_with_drift(
    results: &mut [model::VerdictResult],
    st: &store::Store,
    current_snapshot: &str,
) {
    /// 触发漂移证据所需的连续 unused 快照数。
    const DRIFT_MIN_SNAPSHOTS: u32 = 3;
    for r in results.iter_mut() {
        if r.verdict != model::Verdict::Unused {
            continue;
        }
        let Ok(streak) = st.unused_streak_months(
            &r.package.name,
            r.package.manager.as_str(),
            current_snapshot,
        ) else {
            continue;
        };
        if streak >= DRIFT_MIN_SNAPSHOTS {
            r.evidence.push(model::Evidence::new(
                model::SignalSource::Drift,
                format!("unused in {streak} consecutive snapshots"),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use model::{Manager, Package, SignalSource, Verdict};

    #[test]
    fn atime_candidates_per_manager() {
        // cargo: install_path 即 bin 目录
        let mut cargo = Package::new("rg", Manager::Cargo);
        cargo.install_path = Some(PathBuf::from(r"C:\u\.cargo\bin"));
        assert_eq!(
            atime_candidate_paths(&cargo, "rg"),
            vec![
                PathBuf::from(r"C:\u\.cargo\bin\rg.exe"),
                PathBuf::from(r"C:\u\.cargo\bin\rg.cmd"),
                PathBuf::from(r"C:\u\.cargo\bin\rg"),
            ]
        );

        // npm: node_modules/<pkg> -> prefix 根（Windows shim）与 prefix/bin（Unix）
        let mut npm = Package::new("t", Manager::Npm);
        npm.install_path = Some(PathBuf::from(r"C:\npm\node_modules\t"));
        let c = atime_candidate_paths(&npm, "t");
        assert!(c.contains(&PathBuf::from(r"C:\npm\t.exe")));
        assert!(c.contains(&PathBuf::from(r"C:\npm\bin\t.exe")));

        // pip: site-packages -> 同级 venv Scripts/
        let mut pip = Package::new("p", Manager::Pip);
        pip.install_path = Some(PathBuf::from(r"C:\py\Lib\site-packages"));
        let c = atime_candidate_paths(&pip, "p");
        assert!(c.contains(&PathBuf::from(r"C:\py\Scripts\p.exe")));

        // uv/pipx: install_path 即 venv 根 -> Scripts/ 与 bin/
        let mut uv = Package::new("u", Manager::Uv);
        uv.install_path = Some(PathBuf::from(r"C:\uv\tools\u"));
        let c = atime_candidate_paths(&uv, "u");
        assert!(c.contains(&PathBuf::from(r"C:\uv\tools\u\Scripts\u.exe")));
        assert!(c.contains(&PathBuf::from(r"C:\uv\tools\u\bin\u.exe")));
        let mut pipx = Package::new("x", Manager::Pipx);
        pipx.install_path = Some(PathBuf::from(r"C:\pipx\venvs\x"));
        assert!(atime_candidate_paths(&pipx, "x")
            .contains(&PathBuf::from(r"C:\pipx\venvs\x\Scripts\x.exe")));

        // 采集层未记录 install_path -> 无候选（而非拼出错误路径）
        let bare = Package::new("u", Manager::Uv);
        assert!(atime_candidate_paths(&bare, "u").is_empty());
    }

    #[test]
    fn tracker_from_log_events_prefers_consumed_events_over_probe() {
        // 窗口内读到事件 → 可用（常驻采集写入日志后，未提权的审计也能消费；
        // 旧逻辑只看 probe 的提权检查，会在此误报降级）
        let t = tracker_from_log_events(3);
        assert!(t.available);
        assert!(t.reason.is_none());
        // 读不到事件 → 与环境探测结果一致（提权与否因环境而异，断言等值即可）
        assert_eq!(tracker_from_log_events(0), etw::probe_status());
    }

    #[test]
    fn enrich_with_drift_appends_evidence_after_three_unused_snapshots() {
        let dir = std::env::temp_dir().join(format!("oa_drift_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let st = store::Store::open(&dir.join("t.db")).unwrap();
        let mk = |name: &str, v: Verdict| VerdictResult {
            package: Package::new(name, Manager::Cargo),
            verdict: v,
            interactive_hits: 0,
            last_exec_days_ago: None,
            in_shell_config: false,
            newly_installed: false,
            evidence: vec![],
        };
        // 历史：06 active，07/08 unused —— 截至 2026-09 连续 streak=2，不触发
        st.save_snapshot("2026-06", &[mk("dusty", Verdict::Active)])
            .unwrap();
        st.save_snapshot("2026-07", &[mk("dusty", Verdict::Unused)])
            .unwrap();
        st.save_snapshot("2026-08", &[mk("dusty", Verdict::Unused)])
            .unwrap();
        let mut results = vec![mk("dusty", Verdict::Unused)];
        enrich_with_drift(&mut results, &st, "2026-09");
        assert!(results[0].evidence.is_empty(), "streak=2 must not trigger");

        // 09 也 unused —— 截至 2026-10 streak=3，触发；档位不变，非 unused 不加
        st.save_snapshot("2026-09", &[mk("dusty", Verdict::Unused)])
            .unwrap();
        let mut results = vec![mk("dusty", Verdict::Unused), mk("busy", Verdict::Active)];
        enrich_with_drift(&mut results, &st, "2026-10");
        assert_eq!(results[0].verdict, Verdict::Unused); // 档位不被富化改变
        assert!(results[0]
            .evidence
            .iter()
            .any(|e| e.source == SignalSource::Drift && e.detail.contains("3 consecutive")));
        assert!(
            results[1].evidence.is_empty(),
            "non-unused gets no drift evidence"
        );
        drop(st); // 释放 SQLite 连接后才能删目录（Windows WAL 占用）
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fill_installed_at_uses_binary_creation_and_keeps_existing() {
        let dir = std::env::temp_dir().join(format!("oa_fia_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("tool.exe"), "bin").unwrap();

        let mut p = Package::new("tool", Manager::Cargo);
        p.install_path = Some(dir.join("bin"));
        p.binaries = vec!["tool".into()];
        let mut pkgs = vec![p];
        fill_installed_at(&mut pkgs);
        assert!(
            pkgs[0].installed_at.is_some(),
            "creation time must be filled"
        );

        // 已有精确值不被兜底覆盖
        let preset = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let mut q = Package::new("keep", Manager::Cargo);
        q.install_path = Some(dir.join("bin"));
        q.binaries = vec!["tool".into()];
        q.installed_at = Some(preset);
        let mut pkgs = vec![q];
        fill_installed_at(&mut pkgs);
        assert_eq!(pkgs[0].installed_at, Some(preset));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn installed_at_survives_store_roundtrip() {
        let store = store::Store::in_memory().unwrap();
        let when = Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap();
        let mut p = Package::new("dusty", Manager::Cargo);
        p.installed_at = Some(when);
        store.replace_inventory(&[p], &[]).unwrap();
        let back = store.packages().unwrap();
        assert_eq!(back[0].installed_at, Some(when));
    }

    #[test]
    fn report_from_store_uses_persisted_events() {
        let dir = std::env::temp_dir().join(format!("oa_store_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("t.db");

        let st = store::Store::open(&db).unwrap();
        let mut p = Package::new("dusty", Manager::Cargo);
        p.binaries = vec!["dusty".into()];
        let mut ev = UsageEvent::new("dusty", Utc::now(), SignalSource::History);
        ev.count = 5;
        st.replace_inventory(&[p], &[store::UsageEventRow::from(&ev)])
            .unwrap();

        // 5 次历史命中 >= 自适应阈值（3）-> active；若误用空事件集会判 unused
        let report = report_from_store(&db, &AuditConfig::default()).unwrap();
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].verdict, model::Verdict::Active);
        drop(st); // 先释放 SQLite 连接；WAL 文件被占用时 Windows 无法删除目录
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

//! GUI 状态层：把 core 的审计结果映射成 UI 可直接渲染的扁平视图。
//! 遵循 core 与 GUI 解耦原则：这里只持有渲染所需数据，不重新实现判定逻辑。

use orphan_audit_core::model::{Manager, Verdict, VerdictResult};

/// 单个包在表格里的一行。
#[derive(Debug, Clone)]
pub struct Row {
    pub index: usize,
    pub name: String,
    pub manager: String,
    pub verdict: Verdict,
    pub typed: String,
    pub last_run: String,
    pub size: String,
    pub evidence: Vec<String>,
    pub confidence: String,
}

/// 排序维度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Verdict,
    Name,
    Manager,
    LastRun,
}

/// 仪表盘统计。
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub total: usize,
    pub active: usize,
    pub indirect: usize,
    pub rare: usize,
    pub integration: usize,
    pub unused: usize,
}

impl Summary {
    pub fn from_results(results: &[VerdictResult]) -> Self {
        let mut s = Self {
            total: results.len(),
            ..Default::default()
        };
        for r in results {
            match r.verdict {
                Verdict::Active => s.active += 1,
                Verdict::Indirect => s.indirect += 1,
                Verdict::Rare => s.rare += 1,
                Verdict::Integration => s.integration += 1,
                Verdict::Unused => s.unused += 1,
            }
        }
        s
    }
}

/// 判定强度排序权重（unused 排最前，便于清理决策）。
fn verdict_rank(v: Verdict) -> u8 {
    match v {
        Verdict::Unused => 0,
        Verdict::Integration => 1,
        Verdict::Rare => 2,
        Verdict::Indirect => 3,
        Verdict::Active => 4,
    }
}

/// 把审计结果转换为表格行，应用筛选与排序。
pub fn build_rows(
    results: &[VerdictResult],
    only_removable: bool,
    manager_filter: Option<Manager>,
    sort: SortKey,
) -> Vec<Row> {
    let mut rows: Vec<Row> = results
        .iter()
        .enumerate()
        .filter(|(_, r)| !only_removable || r.verdict.is_removable())
        .filter(|(_, r)| manager_filter.is_none_or(|m| r.package.manager == m))
        .map(|(i, r)| Row {
            index: i,
            name: r.package.name.clone(),
            manager: r.package.manager.as_str().to_string(),
            verdict: r.verdict,
            typed: if r.interactive_hits > 0 {
                format!("{}x", r.interactive_hits)
            } else {
                "-".into()
            },
            last_run: r
                .last_exec_days_ago
                .map(|d| format!("{d}d ago"))
                .unwrap_or_else(|| "never".into()),
            size: r
                .package
                .size_bytes
                .map(|b| format!("{:.1} MB", b as f64 / 1024.0 / 1024.0))
                .unwrap_or_else(|| "-".into()),
            evidence: if r.evidence.is_empty() {
                vec!["no signal from any source".into()]
            } else {
                r.evidence
                    .iter()
                    .map(|e| match e.last_seen {
                        Some(t) => format!("[{}] {} · seen {}", e.source.as_str(), e.detail, t.format("%Y-%m-%d")),
                        None => format!("[{}] {}", e.source.as_str(), e.detail),
                    })
                    .collect()
            },
            confidence: format!("{:?}", r.package.confidence),
        })
        .collect();

    match sort {
        SortKey::Verdict => rows.sort_by_key(|r| verdict_rank(r.verdict)),
        SortKey::Name => rows.sort_by(|a, b| a.name.cmp(&b.name)),
        SortKey::Manager => rows.sort_by(|a, b| a.manager.cmp(&b.manager).then(a.name.cmp(&b.name))),
        SortKey::LastRun => rows.sort_by(|a, b| {
            // "never" 排最前（最值得清理），其次按天数降序
            let da = a.last_run.strip_suffix("d ago").and_then(|s| s.parse::<u32>().ok());
            let db = b.last_run.strip_suffix("d ago").and_then(|s| s.parse::<u32>().ok());
            match (da, db) {
                (None, None) => a.name.cmp(&b.name),
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(x), Some(y)) => y.cmp(&x),
            }
        }),
    }
    rows
}

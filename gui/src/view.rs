//! GUI 状态层：把 core 的审计结果映射成 UI 可直接渲染的扁平视图。
//! 遵循 core 与 GUI 解耦原则：这里只持有渲染所需数据，不重新实现判定逻辑。
//! 界面文案中文化：core 产出的英文证据细节在此做展示层翻译（格式由 verdict.rs 固定）。

use orphan_audit_core::model::{Confidence, Manager, SignalSource, Verdict, VerdictResult};

/// 单个包在表格里的一行。
#[derive(Debug, Clone)]
pub struct Row {
    pub index: usize,
    pub name: String,
    pub manager: String,
    pub verdict: Verdict,
    pub typed: String,
    /// 排序用的交互命中数（与 typed 展示文案对应；不能按 "10 次" 这类文案排序）。
    pub typed_hits: u32,
    /// 展示用文案（"N 天前" / "从未"）。
    pub last_run: String,
    /// 排序用原始天数；None = 无执行记录，排最前。
    pub last_run_days: Option<u32>,
    pub size: String,
    /// 排序用原始字节数；None = 未知。
    pub size_bytes: Option<u64>,
    pub evidence: Vec<String>,
    pub confidence: String,
}

/// 排序维度（与表头一一对应，点击表头切换）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Verdict,
    Name,
    Manager,
    Typed,
    LastRun,
    Size,
}

/// 排序方向。Asc = 各维度的正向：判定=未用优先、名称=字典序、
/// 次数/大小=从小到大、最近运行=从未优先（最值得清理的在前）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
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

/// 判定档位的中文名（帮助面板与表格共用；英文原名在证据面板副标题保留）。
pub fn verdict_cn(v: Verdict) -> &'static str {
    match v {
        Verdict::Active => "活跃",
        Verdict::Indirect => "间接",
        Verdict::Rare => "偶用",
        Verdict::Integration => "集成",
        Verdict::Unused => "未用",
    }
}

/// 信号来源的中文名。
pub fn source_cn(s: SignalSource) -> &'static str {
    match s {
        SignalSource::History => "历史",
        SignalSource::Etw => "ETW",
        SignalSource::Atime => "访问时间",
        SignalSource::ShellConfig => "配置",
        SignalSource::Alias => "别名",
        SignalSource::Wrapped => "还原命令",
        SignalSource::Drift => "漂移",
    }
}

/// 二进制名映射置信度的中文名。
pub fn confidence_cn(c: Confidence) -> &'static str {
    match c {
        Confidence::Exact => "精确",
        Confidence::Inferred => "推断",
        Confidence::None => "未知",
    }
}

/// verdict.rs 产出的固定格式证据细节 → 中文（展示层翻译；未知格式原样返回）。
fn detail_cn(detail: &str) -> String {
    if let Some(rest) = detail.strip_prefix("typed ") {
        if let Some(n) = rest.strip_suffix('x') {
            return format!("历史输入 {n} 次");
        }
    }
    if let Some(rest) = detail.strip_prefix("executed ") {
        if let Some((n, via)) = rest.split_once(" via ") {
            return format!("执行 {} 次（经 {} 触发）", n.trim_end_matches('x'), via);
        }
        return format!("执行 {} 次", rest.trim_end_matches('x'));
    }
    if let Some(rest) = detail.strip_prefix("in shell config: ") {
        return format!("shell 配置引用：{rest}");
    }
    if let Some(rest) = detail.strip_prefix("unused in ") {
        if let Some(n) = rest.strip_suffix(" consecutive snapshots") {
            return format!("连续 {n} 个月快照均判 unused");
        }
    }
    match detail {
        "binary access time touched" => "二进制访问时间近期有更新".to_string(),
        "no signal from any source" => "五路信号均无记录".to_string(),
        other => other.to_string(),
    }
}

/// 一条证据的整行展示文案（来源中文名 + 细节中文 + 最近时间）。
fn evidence_line(e: &orphan_audit_core::model::Evidence) -> String {
    let detail = detail_cn(&e.detail);
    match e.last_seen {
        Some(t) => format!(
            "［{}］{} · 最近 {}",
            source_cn(e.source),
            detail,
            t.format("%Y-%m-%d")
        ),
        None => format!("［{}］{}", source_cn(e.source), detail),
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

/// 把审计结果转换为表格行，应用筛选与排序（排序维度 + 方向）。
pub fn build_rows(
    results: &[VerdictResult],
    only_removable: bool,
    manager_filter: Option<Manager>,
    sort: SortKey,
    dir: SortDir,
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
                format!("{} 次", r.interactive_hits)
            } else {
                "-".into()
            },
            typed_hits: r.interactive_hits,
            last_run: r
                .last_exec_days_ago
                .map(|d| format!("{d} 天前"))
                .unwrap_or_else(|| "从未".into()),
            last_run_days: r.last_exec_days_ago,
            size: r
                .package
                .size_bytes
                .map(|b| format!("{:.1} MB", b as f64 / 1024.0 / 1024.0))
                .unwrap_or_else(|| "-".into()),
            size_bytes: r.package.size_bytes,
            evidence: {
                let mut lines: Vec<String> = if r.evidence.is_empty() {
                    vec!["五路信号均无记录".into()]
                } else {
                    r.evidence.iter().map(evidence_line).collect()
                };
                // 新装观察期提示：该包虽判 unused，但清理预览会跳过它
                if r.newly_installed {
                    lines.push("新装（观察期），暂不进清理候选".into());
                }
                lines
            },
            confidence: confidence_cn(r.package.confidence).to_string(),
        })
        .collect();

    match sort {
        SortKey::Verdict => rows.sort_by_key(|r| verdict_rank(r.verdict)),
        SortKey::Name => rows.sort_by(|a, b| a.name.cmp(&b.name)),
        SortKey::Manager => {
            rows.sort_by(|a, b| a.manager.cmp(&b.manager).then(a.name.cmp(&b.name)))
        }
        // 次数/大小按数值排序（展示文案 "10 次" 的字符串序是错的）
        SortKey::Typed => {
            rows.sort_by(|a, b| a.typed_hits.cmp(&b.typed_hits).then(a.name.cmp(&b.name)))
        }
        SortKey::LastRun => rows.sort_by(|a, b| {
            // 无执行记录（从未）排最前（最值得清理），其次按天数降序（越久没跑越靠前）
            match (a.last_run_days, b.last_run_days) {
                (None, None) => a.name.cmp(&b.name),
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(x), Some(y)) => y.cmp(&x),
            }
        }),
        SortKey::Size => rows.sort_by(|a, b| {
            a.size_bytes
                .unwrap_or(0)
                .cmp(&b.size_bytes.unwrap_or(0))
                .then(a.name.cmp(&b.name))
        }),
    }
    if dir == SortDir::Desc {
        rows.reverse();
    }
    rows
}

/// 按 results 索引解析当前视图中的行。
/// `selected` 存的是 results 中的索引（即 `Row.index`），而渲染顺序由
/// 排序/筛选决定，两者位置不对应——必须按 index 查找，不能用 `rows.get(i)`
/// 按位置取（否则排序后证据面板会指向另一个包）。
pub fn row_by_index(rows: &[Row], results_index: usize) -> Option<&Row> {
    rows.iter().find(|r| r.index == results_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orphan_audit_core::model::{Manager, Package};

    fn result(name: &str, bin: &str, verdict: Verdict, days_ago: Option<u32>) -> VerdictResult {
        let mut p = Package::new(name, Manager::Cargo);
        p.binaries = vec![bin.into()];
        VerdictResult {
            package: p,
            verdict,
            interactive_hits: 0,
            last_exec_days_ago: days_ago,
            in_shell_config: false,
            newly_installed: false,
            evidence: vec![],
        }
    }

    /// 带 interactive_hits 的构造（输入次数排序用）。
    fn result_hits(name: &str, hits: u32) -> VerdictResult {
        VerdictResult {
            interactive_hits: hits,
            ..result(name, name, Verdict::Active, None)
        }
    }

    #[test]
    fn selection_resolves_by_results_index_after_sort() {
        // results=[A(active), B(unused)]，Verdict 排序后 rows=[B,A]：
        // selected=0（A）必须解析回 A，而不是按位置取到 B（索引错位回归测试）
        let results = vec![
            result("alpha", "alpha", Verdict::Active, None),
            result("beta", "beta", Verdict::Unused, None),
        ];
        let rows = build_rows(&results, false, None, SortKey::Verdict, SortDir::Asc);
        assert_eq!(rows[0].name, "beta");
        let sel = row_by_index(&rows, 0).expect("alpha must stay resolvable after sort");
        assert_eq!(sel.name, "alpha");
        assert!(row_by_index(&rows, 2).is_none());
    }

    #[test]
    fn only_removable_filter_drops_non_unused() {
        let results = vec![
            result("alpha", "alpha", Verdict::Active, None),
            result("beta", "beta", Verdict::Unused, None),
            result("gamma", "gamma", Verdict::Rare, None),
        ];
        let rows = build_rows(&results, true, None, SortKey::Verdict, SortDir::Asc);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "beta");
    }

    #[test]
    fn last_run_sort_puts_never_first_then_oldest() {
        let results = vec![
            result("recent", "recent", Verdict::Indirect, Some(2)),
            result("stale", "stale", Verdict::Indirect, Some(90)),
            result("never", "never", Verdict::Unused, None),
        ];
        let rows = build_rows(&results, false, None, SortKey::LastRun, SortDir::Asc);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["never", "stale", "recent"]);
    }

    #[test]
    fn summary_counts_all_tiers() {
        let results = vec![
            result("a", "a", Verdict::Active, None),
            result("b", "b", Verdict::Unused, None),
            result("c", "c", Verdict::Unused, None),
            result("d", "d", Verdict::Rare, None),
        ];
        let s = Summary::from_results(&results);
        assert_eq!(s.total, 4);
        assert_eq!(s.active, 1);
        assert_eq!(s.unused, 2);
        assert_eq!(s.rare, 1);
        assert_eq!(s.integration, 0);
    }

    #[test]
    fn sort_dir_desc_reverses_order() {
        let results = vec![
            result("alpha", "alpha", Verdict::Active, None),
            result("beta", "beta", Verdict::Unused, None),
        ];
        let asc = build_rows(&results, false, None, SortKey::Name, SortDir::Asc);
        let desc = build_rows(&results, false, None, SortKey::Name, SortDir::Desc);
        let asc_names: Vec<&str> = asc.iter().map(|r| r.name.as_str()).collect();
        let desc_names: Vec<&str> = desc.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(asc_names, vec!["alpha", "beta"]);
        assert_eq!(desc_names, vec!["beta", "alpha"]);
    }

    #[test]
    fn typed_sort_uses_numeric_hits_not_display_text() {
        // 展示文案 "10 次" 的字符串序在 "2 次" 之前；排序必须按数值 2 < 10
        let results = vec![result_hits("ten", 10), result_hits("two", 2)];
        let rows = build_rows(&results, false, None, SortKey::Typed, SortDir::Asc);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["two", "ten"]);
    }

    #[test]
    fn evidence_details_are_translated_to_chinese() {
        // detail_cn 覆盖 verdict.rs 的全部固定格式（格式变化时此测试会失败提醒同步）
        assert_eq!(detail_cn("typed 3x"), "历史输入 3 次");
        assert_eq!(detail_cn("executed 2x"), "执行 2 次");
        assert_eq!(
            detail_cn("executed 1x via make.exe"),
            "执行 1 次（经 make.exe 触发）"
        );
        assert_eq!(
            detail_cn("binary access time touched"),
            "二进制访问时间近期有更新"
        );
        assert_eq!(
            detail_cn("in shell config: .bashrc:12"),
            "shell 配置引用：.bashrc:12"
        );
        assert_eq!(detail_cn("no signal from any source"), "五路信号均无记录");
        assert_eq!(
            detail_cn("unused in 4 consecutive snapshots"),
            "连续 4 个月快照均判 unused"
        );
        // 未知格式原样返回（core 未来新增格式时的兜底）
        assert_eq!(detail_cn("some future format"), "some future format");
    }

    #[test]
    fn newly_installed_rows_carry_observation_note() {
        let mut r = result("fresh-tool", "fresh-tool", Verdict::Unused, None);
        r.newly_installed = true;
        let rows = build_rows(&[r], false, None, SortKey::Name, SortDir::Asc);
        assert_eq!(rows.len(), 1);
        assert!(rows[0]
            .evidence
            .contains(&"新装（观察期），暂不进清理候选".to_string()));
        // 非新装行不加标注
        let rows = build_rows(
            &[result("old-tool", "old-tool", Verdict::Unused, None)],
            false,
            None,
            SortKey::Name,
            SortDir::Asc,
        );
        assert!(!rows[0]
            .evidence
            .contains(&"新装（观察期），暂不进清理候选".to_string()));
    }
}

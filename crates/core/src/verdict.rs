//! 五档判定引擎：纯函数式，输入包 + 使用事件集合，输出判定与证据链。
//!
//! 核心信条（继承自 shelfware 的经验教训）：
//! 1. 规则顺序就是正确性。active -> indirect -> rare -> integration -> unused，
//!    每一条规则的存在都是为了把包从错误的 unused 中「救出来」。
//!    重排规则会立刻开始产生误报。
//! 2. 一个包只有在五路信号全部静默时才能判 unused。
//! 3. 每行判定必须携带证据，让用户可以反驳判定，而不是盲目相信。

use crate::model::{
    basename, AuditConfig, Evidence, Package, SignalSource, UsageEvent, Verdict, VerdictResult,
};
use chrono::{DateTime, Utc};

/// 自适应活动阈值：每 1000 条历史命令计 1 次，钳制到 [3, 50]。
/// 新机器历史少，不能用和多年老机器一样的门槛。
pub fn adaptive_active_hits(total_history_entries: u32) -> u32 {
    (total_history_entries / 1000).clamp(3, 50)
}

/// 判定单个包。
///
/// `events` 是与该包任一二进制关联的所有事件；调用方（verdict 层的
/// `evaluate_all`）负责先做 bin 名匹配过滤。
/// `history_total` 是全机器的历史命令总数，用于自适应阈值。
/// `now` 由调用方注入，便于测试固定时间。
pub fn evaluate(
    package: &Package,
    events: &[UsageEvent],
    shell_config_mentions: &[String],
    config: &AuditConfig,
    history_total: u32,
    now: DateTime<Utc>,
) -> VerdictResult {
    let mut evidence: Vec<Evidence> = Vec::new();

    // 按来源归类事件，同时统计最近时间与命中数
    let mut interactive_hits: u32 = 0;
    let mut last_exec: Option<DateTime<Utc>> = None;
    let mut history_hits: u32 = 0;
    let mut etw_hits: u32 = 0;
    let mut atime_hit = false;
    let mut indirect_parent: Option<String> = None;

    for ev in events {
        let count = ev.count.max(1);
        match ev.source {
            SignalSource::History | SignalSource::Wrapped => {
                history_hits += count;
                interactive_hits += count;
            }
            SignalSource::Etw => {
                etw_hits += count;
                // 执行证据取所有执行类事件的最新时间
                if last_exec.is_none_or(|t| ev.timestamp > t) {
                    last_exec = Some(ev.timestamp);
                }
                // 父进程不是交互 shell -> 间接调用线索
                if let Some(parent) = &ev.parent {
                    if !is_interactive_shell(parent) && indirect_parent.is_none() {
                        indirect_parent = Some(parent.clone());
                    }
                }
            }
            SignalSource::Atime => {
                atime_hit = true;
                if last_exec.is_none_or(|t| ev.timestamp > t) {
                    last_exec = Some(ev.timestamp);
                }
            }
            SignalSource::ShellConfig | SignalSource::Alias => {
                // 这两类单独走 shell_config_mentions，此处仅计数展示
            }
        }
    }

    // 组织证据链（按强度排序，UI 直接展示）
    if history_hits > 0 {
        evidence.push(
            // last_seen 恒为 None：纯文本历史无逐条时间戳（mtime 是整批近似值），
            // 展示具体日期会误导，故 history 证据只报次数不报时间。
            Evidence::new(SignalSource::History, format!("typed {history_hits}x"))
                .with_last(None)
                .with_hits(history_hits),
        );
    }
    if etw_hits > 0 {
        let detail = match &indirect_parent {
            Some(p) => format!("executed {etw_hits}x via {p}"),
            None => format!("executed {etw_hits}x"),
        };
        evidence.push(
            Evidence::new(SignalSource::Etw, detail)
                .with_last(last_exec)
                .with_hits(etw_hits),
        );
    }
    if atime_hit {
        evidence.push(
            Evidence::new(SignalSource::Atime, "binary access time touched").with_last(last_exec),
        );
    }
    if !shell_config_mentions.is_empty() {
        evidence.push(Evidence::new(
            SignalSource::ShellConfig,
            format!("in shell config: {}", shell_config_mentions.join(", ")),
        ));
    }

    let threshold = config
        .active_hits
        .unwrap_or_else(|| adaptive_active_hits(history_total));
    let in_window = |t: DateTime<Utc>| {
        let age = now.signed_duration_since(t).num_days();
        age >= 0 && age <= config.since_days as i64
    };
    let has_recent_exec = last_exec.is_some_and(in_window);
    let in_shell_config = !shell_config_mentions.is_empty();

    // === 规则按优先级顺序判定，命中即停 ===

    // 规则 1: active —— 交互次数足够多，毫无疑问
    let verdict = if interactive_hits >= threshold {
        Verdict::Active
    }
    // 规则 2: indirect —— 近期执行过（含 ETW/atime），即使是脚本/agent 触发的
    // 注意：这条放在 history 计数判断之前，因为「昨天真的跑了」比「你很少敲」更强
    else if has_recent_exec {
        Verdict::Indirect
    }
    // 规则 3: rare —— 历史里出现过，但最近窗口内没跑过
    else if history_hits > 0 {
        Verdict::Rare
    }
    // 规则 4: integration —— 没跑过，但被 shell 配置引用
    else if in_shell_config {
        Verdict::Integration
    }
    // 规则 5: unused —— 五路信号全静默
    else {
        Verdict::Unused
    };

    let last_exec_days_ago =
        last_exec.map(|t| now.signed_duration_since(t).num_days().max(0) as u32);

    VerdictResult {
        package: package.clone(),
        verdict,
        interactive_hits,
        last_exec_days_ago,
        in_shell_config,
        evidence,
    }
}

/// 批量判定：把事件按包归并，一次算出全部结果。
pub fn evaluate_all(
    packages: &[Package],
    events: &[UsageEvent],
    shell_config_index: &std::collections::HashMap<String, Vec<String>>,
    config: &AuditConfig,
    history_total: u32,
    now: DateTime<Utc>,
) -> Vec<VerdictResult> {
    packages
        .iter()
        // 依赖包永不进入清理候选（brew leaves 之类已在采集层排除，这里是第二道防线）
        .filter(|p| !p.is_dependency)
        // 忽略列表：包名或任一 bin 名匹配即跳过
        .filter(|p| {
            !config.ignore.iter().any(|ig| {
                let ig = basename(ig);
                basename(&p.name) == ig || p.binaries.iter().any(|b| basename(b) == ig)
            })
        })
        .filter(|p| config.managers.is_empty() || config.managers.contains(&p.manager))
        .map(|p| {
            let matched_events: Vec<UsageEvent> = events
                .iter()
                .filter(|e| p.owns_binary(&e.binary))
                .cloned()
                .collect();

            // shell 配置引用：任一 bin 名或包名在索引里
            let mut mentions: Vec<String> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for key in p.binaries.iter().chain(std::iter::once(&p.name)) {
                if let Some(locs) = shell_config_index.get(basename(key)) {
                    for loc in locs {
                        if seen.insert(loc.clone()) {
                            mentions.push(loc.clone());
                        }
                    }
                }
            }
            mentions.sort();

            evaluate(p, &matched_events, &mentions, config, history_total, now)
        })
        .collect()
}

/// 是否为交互式 shell（这些父进程下的 ETW 事件算「可能是用户直接调用」）。
fn is_interactive_shell(parent: &str) -> bool {
    let p = basename(parent).to_ascii_lowercase();
    matches!(
        p.as_str(),
        "powershell.exe"
            | "pwsh.exe"
            | "cmd.exe"
            | "bash.exe"
            | "zsh"
            | "bash"
            | "fish"
            | "sh"
            | "windowsterminal.exe"
            | "conhost.exe"
            | "explorer.exe"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Manager;
    use chrono::TimeZone;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 18, 0, 0, 0).unwrap()
    }

    fn pkg(name: &str, bin: &str) -> Package {
        let mut p = Package::new(name, Manager::Cargo);
        p.binaries.push(bin.into());
        p
    }

    #[test]
    fn adaptive_threshold_is_clamped() {
        assert_eq!(adaptive_active_hits(0), 3);
        assert_eq!(adaptive_active_hits(1000), 3);
        assert_eq!(adaptive_active_hits(10_000), 10);
        assert_eq!(adaptive_active_hits(1_000_000), 50);
    }

    #[test]
    fn all_silent_is_unused() {
        let r = evaluate(
            &pkg("dusty-tool", "dusty"),
            &[],
            &[],
            &AuditConfig::default(),
            1000,
            fixed_now(),
        );
        assert_eq!(r.verdict, Verdict::Unused);
        assert!(r.verdict.is_removable());
        assert!(r.evidence.is_empty());
    }

    #[test]
    fn heavy_history_is_active() {
        let now = fixed_now();
        let evs: Vec<UsageEvent> = (0..10)
            .map(|_| UsageEvent::new("dusty", now, SignalSource::History))
            .collect();
        let r = evaluate(
            &pkg("dusty-tool", "dusty"),
            &evs,
            &[],
            &AuditConfig::default(),
            1000, // 阈值=3
            now,
        );
        assert_eq!(r.verdict, Verdict::Active);
        assert_eq!(r.interactive_hits, 10);
    }

    #[test]
    fn recent_etw_from_make_is_indirect_not_unused() {
        let now = fixed_now();
        let mut ev = UsageEvent::new("dusty", now, SignalSource::Etw);
        ev.parent = Some("make.exe".into());
        ev.count = 1;
        let r = evaluate(
            &pkg("dusty-tool", "dusty"),
            &[ev],
            &[],
            &AuditConfig::default(),
            1000,
            now,
        );
        // 近期执行过 -> indirect，即使不是用户敲的
        assert_eq!(r.verdict, Verdict::Indirect);
        assert!(r.evidence.iter().any(|e| e.detail.contains("via make.exe")));
    }

    #[test]
    fn old_history_without_recent_exec_is_rare() {
        let now = fixed_now();
        let old = now - chrono::Duration::days(120);
        let ev = UsageEvent::new("dusty", old, SignalSource::History);
        let r = evaluate(
            &pkg("dusty-tool", "dusty"),
            &[ev],
            &[],
            &AuditConfig::default(),
            1000,
            now,
        );
        assert_eq!(r.verdict, Verdict::Rare);
        assert!(!r.in_shell_config);
    }

    #[test]
    fn shell_config_only_is_integration() {
        let r = evaluate(
            &pkg("direnv", "direnv"),
            &[],
            &[".bashrc:42".into()],
            &AuditConfig::default(),
            1000,
            fixed_now(),
        );
        assert_eq!(r.verdict, Verdict::Integration);
        assert!(r.in_shell_config);
    }

    #[test]
    fn precedence_recent_exec_beats_shell_config() {
        // 同时有 shell 配置和近期执行 -> indirect 优先于 integration
        let now = fixed_now();
        let ev = UsageEvent::new("direnv", now, SignalSource::Etw);
        let r = evaluate(
            &pkg("direnv", "direnv"),
            &[ev],
            &[".bashrc:42".into()],
            &AuditConfig::default(),
            1000,
            now,
        );
        assert_eq!(r.verdict, Verdict::Indirect);
    }

    #[test]
    fn since_window_controls_indirect_boundary() {
        let now = fixed_now();
        let old = now - chrono::Duration::days(60);
        let ev = UsageEvent::new("dusty", old, SignalSource::Etw);
        // 默认 30 天窗口：60 天前的执行不算近期 -> 无 history -> 无 config -> unused
        let r30 = evaluate(
            &pkg("dusty-tool", "dusty"),
            std::slice::from_ref(&ev),
            &[],
            &AuditConfig::default(),
            1000,
            now,
        );
        assert_eq!(r30.verdict, Verdict::Unused);
        // 放宽到 90 天：变成 indirect
        let cfg = AuditConfig {
            since_days: 90,
            ..Default::default()
        };
        let r90 = evaluate(&pkg("dusty-tool", "dusty"), &[ev], &[], &cfg, 1000, now);
        assert_eq!(r90.verdict, Verdict::Indirect);
    }

    #[test]
    fn dependency_and_ignore_are_filtered() {
        let mut dep = pkg("libfoo", "libfoo");
        dep.is_dependency = true;
        let mut ignored = pkg("keepme", "keepme");
        ignored.binaries = vec!["keep-bin".into()];
        let normal = pkg("normal", "normal");
        let cfg = AuditConfig {
            ignore: vec!["keep-bin".into()],
            ..Default::default()
        };
        let map = std::collections::HashMap::new();
        let results = evaluate_all(&[dep, ignored, normal], &[], &map, &cfg, 1000, fixed_now());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].package.name, "normal");
    }
}

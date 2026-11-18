//! 后台执行追踪：Windows 用 ETW（Event Tracing for Windows）精确捕获进程启动。
//!
//! 这是本项目相较竞品的核心信号优势：
//! - atime 只能证明「跑过」，无法归因谁运行、运行几次
//! - shell 历史会被脚本 / CI / coding agent 绕过
//! - ETW 直接监听内核进程创建事件，含父进程、时间戳、精确计数
//!   （命令行字段在较新 Windows 构建已从该事件移除，见 etw-capture/DESIGN.md）
//!
//! 权限：Microsoft-Windows-Kernel-Process provider 通常需要管理员权限。
//! 无权限时追踪器返回不可用状态，上层自动降级到 atime + 历史并在报告中明示。
//!
//! 实现现状：本模块是事件日志的**消费端**（读取外部采集器写入的 JSONL），
//! 不自带 ETW session / 守护进程；采集器见仓库根 `etw-capture/`（S17 已交付）。

use crate::model::{SignalSource, UsageEvent};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

/// 追踪器状态，供 UI 展示（是否降级及原因）。采集到的量级等数据
/// 不在此维护（曾有 events_captured/uptime_secs 字段，全程无消费方且
/// 迫使 probe_status 全量解析日志，已删）。
#[derive(Debug, Clone, PartialEq)]
pub struct TrackerStatus {
    pub available: bool,
    pub reason: Option<String>,
}

impl Default for TrackerStatus {
    fn default() -> Self {
        Self {
            available: false,
            reason: Some("not started".into()),
        }
    }
}

/// 从落盘的事件日志文件读取 ETW 采集到的 UsageEvent（不过滤时间）。
/// 守护进程把每条进程创建写成一行 JSONL，避免跨进程共享内存复杂度。
/// 格式：{"binary":"rg","ts":"2026-11-18T00:00:00Z","parent":"pwsh.exe","cmd":"rg foo"}
pub fn read_etw_log(path: &Path) -> anyhow::Result<Vec<UsageEvent>> {
    read_etw_log_since(path, None)
}

/// 带窗口过滤的日志读取：只保留 `ts >= since` 的事件（None = 不过滤）。
/// 常驻采集（etw-capture install）后日志无限增长，而窗口外 ETW 事件对判定
/// 无用（indirect 要求窗口内执行），审计只需取 since 窗口内事件。
pub fn read_etw_log_since(
    path: &Path,
    since: Option<DateTime<Utc>>,
) -> anyhow::Result<Vec<UsageEvent>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)?;
    let mut events = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let Some(bin) = v.get("binary").and_then(|x| x.as_str()) else {
                continue;
            };
            let ts = v
                .get("ts")
                .and_then(|x| x.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);
            if since.is_some_and(|s| ts < s) {
                continue;
            }
            let mut ev = UsageEvent::new(bin, ts, SignalSource::Etw);
            ev.parent = v.get("parent").and_then(|x| x.as_str()).map(str::to_string);
            ev.command_line = v.get("cmd").and_then(|x| x.as_str()).map(str::to_string);
            events.push(ev);
        }
    }
    Ok(events)
}

/// 把一条进程事件追加为 JSONL（守护进程侧使用）。
pub fn append_etw_event(
    path: &Path,
    binary: &str,
    parent: Option<&str>,
    cmd: Option<&str>,
) -> anyhow::Result<()> {
    use std::io::Write;
    if let Some(parent_dir) = path.parent() {
        std::fs::create_dir_all(parent_dir)?;
    }
    let rec = serde_json::json!({
        "binary": binary,
        "ts": Utc::now().to_rfc3339(),
        "parent": parent,
        "cmd": cmd,
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{rec}")?;
    Ok(())
}

/// 默认 ETW 事件日志路径：数据目录下 etw/events.jsonl。
pub fn default_log_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("orphan-audit").join("etw").join("events.jsonl"))
}

/// 检查当前进程是否具备管理员权限（ETW 内核 provider 的前提）。
#[cfg(windows)]
pub fn is_elevated() -> bool {
    // 通过 fsutil 这种需要管理员的只读命令间接判断，避免引入 windows-sys 复杂 FFI
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    let mut c = Command::new("net");
    c.args(["session"]);
    c.creation_flags(crate::inventory::CREATE_NO_WINDOW);
    c.output().map(|o| o.status.success()).unwrap_or(false)
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

/// 根据环境推断追踪器可用性（不实际启动守护，也不读事件日志——
/// 事件由 audit 主流程按 since 窗口读取，这里只回答「能不能用」）。
pub fn probe_status() -> TrackerStatus {
    if !cfg!(windows) {
        return TrackerStatus {
            available: false,
            reason: Some("ETW requires Windows; running on non-Windows host".into()),
        };
    }
    if !is_elevated() {
        return TrackerStatus {
            available: false,
            reason: Some("administrator privileges required for Kernel-Process provider".into()),
        };
    }
    TrackerStatus {
        available: true,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etw_jsonl_roundtrip() {
        let dir = std::env::temp_dir().join(format!("oa_etw_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("events.jsonl");
        append_etw_event(&p, "rg", Some("pwsh.exe"), Some("rg fix")).unwrap();
        append_etw_event(&p, "make", Some("pwsh.exe"), Some("make build")).unwrap();
        let events = read_etw_log(&p).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].binary, "rg");
        assert_eq!(events[0].source, SignalSource::Etw);
        assert_eq!(events[0].parent.as_deref(), Some("pwsh.exe"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_log_is_empty_not_error() {
        let p = std::env::temp_dir().join(format!("oa_missing_{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        assert!(read_etw_log(&p).unwrap().is_empty());
    }

    #[test]
    fn read_etw_log_since_drops_events_before_window() {
        let dir = std::env::temp_dir().join(format!("oa_etw_win_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("events.jsonl");
        let old_ts = "2020-01-01T00:00:00+00:00";
        let new_ts = Utc::now().to_rfc3339();
        std::fs::write(
            &p,
            format!(
                "{{\"binary\":\"old\",\"ts\":\"{old_ts}\",\"parent\":null,\"cmd\":null}}\n\
                 {{\"binary\":\"new\",\"ts\":\"{new_ts}\",\"parent\":\"pwsh.exe\",\"cmd\":null}}\n"
            ),
        )
        .unwrap();
        // 30 天窗口：old 被剔除，new 保留（含 parent 归因字段）
        let since = Utc::now() - chrono::Duration::days(30);
        let evs = read_etw_log_since(&p, Some(since)).unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].binary, "new");
        assert_eq!(evs[0].parent.as_deref(), Some("pwsh.exe"));
        // None = 不过滤（兼容旧行为）
        assert_eq!(read_etw_log_since(&p, None).unwrap().len(), 2);
        // 窗口早于两事件：全部保留
        let earlier = Utc::now() - chrono::Duration::days(4000);
        assert_eq!(read_etw_log_since(&p, Some(earlier)).unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

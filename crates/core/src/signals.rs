//! 信号层：把各种来源归一化成 UsageEvent。
//!
//! 三类信号：
//! 1. shell 历史（最强交互证据）—— PowerShell PSReadLine / bash / zsh / Atuin
//! 2. shell 配置与 alias（integration 证据，偏保守）
//! 3. 文件访问时间 atime（弱执行证据，无法归因）
//!
//! 关键解析规则（继承 shelfware 的经验）：
//! - 一行历史可能含多个命令：按 | && ; || 拆分
//! - 跳过 wrapper：sudo / env / xargs / time / command
//! - 忽略 FOO=bar 前置赋值
//! - 路径取 basename，去 .exe 后缀
//! - Atuin 存在时优先于纯文本历史文件，避免双计

use crate::inventory::strip_exe;
use crate::model::{basename, SignalSource, UsageEvent};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 历史采集结果：事件列表 + 全机器历史命令总数（用于自适应阈值）。
#[derive(Debug, Default)]
pub struct HistoryResult {
    pub events: Vec<UsageEvent>,
    pub total_entries: u32,
    pub sources_used: Vec<String>,
}

/// 采集所有可用的 shell 历史。
pub fn collect_history() -> HistoryResult {
    let mut result = HistoryResult::default();

    // Atuin 优先（若存在则跳过纯文本，避免双计）
    if let Some(atuin) = atuin_db_path() {
        match parse_atuin(&atuin) {
            Ok(r) if !r.events.is_empty() => {
                result
                    .sources_used
                    .push(format!("atuin:{}", atuin.display()));
                merge(&mut result, r);
                return result;
            }
            _ => {}
        }
    }

    // PowerShell PSReadLine 历史：C:\Users\<u>\AppData\Roaming\Microsoft\Windows\PowerShell\PSReadLine\ConsoleHost_history.txt
    if let Some(dir) = psreadline_dir() {
        for f in ["ConsoleHost_history.txt", "PowerShellCore_host_history.txt"] {
            let p = dir.join(f);
            if p.exists() {
                if let Ok(r) = parse_plain_history(&p, SignalSource::History) {
                    result.sources_used.push(format!("psreadline:{f}"));
                    merge(&mut result, r);
                }
            }
        }
    }

    // bash / zsh 历史（Git Bash / MSYS2 / WSL 挂载）
    for p in [
        dirs::home_dir().map(|h| h.join(".bash_history")),
        dirs::home_dir().map(|h| h.join(".zsh_history")),
    ]
    .into_iter()
    .flatten()
    {
        if p.exists() {
            if let Ok(r) = parse_plain_history(&p, SignalSource::History) {
                result.sources_used.push(format!(
                    "plain:{}",
                    p.file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
                merge(&mut result, r);
            }
        }
    }

    result
}

fn merge(dst: &mut HistoryResult, src: HistoryResult) {
    dst.events.extend(src.events);
    dst.total_entries += src.total_entries;
}

fn atuin_db_path() -> Option<PathBuf> {
    let p = dirs::data_dir()?.join("atuin").join("history.db");
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

fn psreadline_dir() -> Option<PathBuf> {
    let base = dirs::home_dir()?.join("AppData/Roaming/Microsoft/Windows/PowerShell/PSReadLine");
    if base.exists() {
        Some(base)
    } else {
        None
    }
}

/// 解析纯文本历史。PowerShell 的 PSReadLine 默认每行一条命令、无时间戳，
/// 因此用文件修改时间作为整批事件的近似时间（保守：视为较旧，不会误判为 active）。
pub fn parse_plain_history(path: &Path, source: SignalSource) -> Result<HistoryResult> {
    let raw = std::fs::read_to_string(path)?;
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now);
    let mut events = Vec::new();
    let mut total = 0u32;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        total += 1;
        for cmd in split_commands(line) {
            if let Some(bin) = real_command_name(&cmd) {
                let mut ev = UsageEvent::new(bin, mtime, source);
                ev.command_line = Some(cmd.to_string());
                events.push(ev);
            }
        }
    }
    Ok(HistoryResult {
        events,
        total_entries: total,
        sources_used: vec![],
    })
}

/// 解析 Atuin SQLite 历史库。只在 rusqlite 可用时工作；表结构随版本变化，
/// 失败时返回空并让上层降级到纯文本。
fn parse_atuin(path: &Path) -> Result<HistoryResult> {
    use rusqlite::OpenFlags;
    let conn = rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn
        .prepare("SELECT command, timestamp FROM history ORDER BY timestamp DESC LIMIT 100000")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut events = Vec::new();
    let mut total = 0u32;
    for row in rows.flatten() {
        let (cmd, ts) = row;
        total += 1;
        // Atuin timestamp 是 Unix 秒（部分版本用纳秒）
        let ts_secs = if ts > 100_000_000_000 {
            ts / 1_000_000_000
        } else {
            ts
        };
        let when = Utc
            .timestamp_opt(ts_secs, 0)
            .single()
            .unwrap_or_else(Utc::now);
        for part in split_commands(&cmd) {
            if let Some(bin) = real_command_name(&part) {
                let mut ev = UsageEvent::new(bin, when, SignalSource::History);
                ev.command_line = Some(part.to_string());
                events.push(ev);
            }
        }
    }
    Ok(HistoryResult {
        events,
        total_entries: total,
        sources_used: vec![],
    })
}

/// 按 shell 操作符拆分一行命令：| || && ; 以及换行。
/// 保留引号内的分隔符（简单状态机，不解析转义的极端情况）。
pub fn split_commands(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            cur.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                cur.push(c);
            }
            '|' | ';' | '&' => {
                // 处理 || && 双字符：跳过第二个字符
                if matches!(chars.peek(), Some(&'|') if c == '|')
                    || matches!(chars.peek(), Some(&'&') if c == '&')
                {
                    chars.next();
                }
                if !cur.trim().is_empty() {
                    parts.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts
}

const WRAPPERS: &[&str] = &[
    "sudo", "env", "xargs", "time", "command", "nohup", "exec", "doas",
];

/// 从一段命令中还原真实命令名：剥离前置赋值与 wrapper，取 basename。
pub fn real_command_name(segment: &str) -> Option<String> {
    // Windows 路径里的反斜杠会被 shell_words 当转义符吞掉（C:\tools\fd.exe -> C:toolsfd），
    // 先把 \ 统一成 / 再分词；这只影响命令名提取，不改动原始 command_line。
    let normalized = segment.replace('\\', "/");
    let mut words = shell_words::split(&normalized).ok()?;
    // 去掉 FOO=bar 前置赋值
    while words
        .first()
        .is_some_and(|w| w.contains('=') && !w.starts_with('/'))
    {
        words.remove(0);
    }
    // 剥离 wrapper（可能连续多个，如 sudo env FOO=bar cmd）
    loop {
        match words.first() {
            Some(first) if WRAPPERS.contains(&first.to_ascii_lowercase().as_str()) => {
                words.remove(0);
                while words.first().is_some_and(|w| w.contains('=')) {
                    words.remove(0);
                }
            }
            _ => break,
        }
    }
    let first = words.first()?;
    let name = strip_exe(basename(first));
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// 扫描 shell 启动配置，构建 命令名 -> [引用位置] 索引。
/// 只做宽松的裸词匹配（shelfware 的 integration 判定即如此，宁可保守保留）。
pub fn collect_shell_config() -> HashMap<String, Vec<String>> {
    let mut index: HashMap<String, Vec<String>> = HashMap::new();
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        for f in [
            ".bashrc",
            ".bash_profile",
            ".zshrc",
            ".zshenv",
            ".profile",
            ".config/fish/config.fish",
        ] {
            files.push(home.join(f));
        }
        // PowerShell profile
        if let Some(doc) = dirs::document_dir() {
            files.push(doc.join("WindowsPowerShell/Microsoft.PowerShell_profile.ps1"));
            files.push(doc.join("PowerShell/Microsoft.PowerShell_profile.ps1"));
        }
    }

    for path in files {
        if !path.exists() {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let label = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        for (i, line) in raw.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            // alias cat='bat --paging=never' -> 记录 bat，同时记录 cat->bat 映射
            if let Some(rest) = trimmed.strip_prefix("alias ") {
                if let Some(eq) = rest.find('=') {
                    let alias_name = rest[..eq].trim();
                    let target = rest[eq + 1..].trim().trim_matches(['\'', '"']);
                    if let Some(bin) = real_command_name(target) {
                        index
                            .entry(bin)
                            .or_default()
                            .push(format!("{label}:{} (alias {alias_name})", i + 1));
                    }
                    continue;
                }
            }
            // PowerShell: Set-Alias cat bat / function ...
            if let Some(rest) = trimmed.strip_prefix("Set-Alias ") {
                let words: Vec<&str> = rest.split_whitespace().collect();
                if words.len() >= 2 {
                    let target = strip_exe(basename(words[1])).to_string();
                    index
                        .entry(target)
                        .or_default()
                        .push(format!("{label}:{} (alias)", i + 1));
                }
                continue;
            }
            // 普通行：提取每个 token 作为可能的命令名（宽松匹配，偏保守）
            for tok in trimmed.split_whitespace() {
                let clean = tok.trim_matches(['\'', '"', '(', ')', ';', '|']);
                let bin = strip_exe(basename(clean));
                if bin.len() >= 2
                    && bin
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    index
                        .entry(bin.to_ascii_lowercase())
                        .or_default()
                        .push(format!("{label}:{}", i + 1));
                }
            }
        }
    }
    index
}

/// atime 探测：给定包提供的二进制路径，取最新的访问时间作为弱执行证据。
/// 调用方需自行判断 NTFS atime 是否启用（见 atime_enabled）。
pub fn probe_atime(binaries: &[PathBuf]) -> Option<UsageEvent> {
    let mut latest: Option<DateTime<Utc>> = None;
    for p in binaries {
        if let Ok(md) = std::fs::metadata(p) {
            if let Ok(t) = md.accessed() {
                let dt = DateTime::<Utc>::from(t);
                if latest.is_none_or(|l| dt > l) {
                    latest = Some(dt);
                }
            }
        }
    }
    latest.map(|t| UsageEvent::new("atime", t, SignalSource::Atime))
}

/// 查询 NTFS LastAccessTime 是否启用。
/// Windows 10 v1803+ 默认为 System Managed（值 2），实际启用但粒度 60 分钟。
#[cfg(windows)]
pub fn atime_enabled() -> Option<bool> {
    use std::process::Command;
    let mut cmd = Command::new("fsutil");
    cmd.args(["behavior", "query", "disablelastaccess"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    // 输出形如 "DisableLastAccess = 2" 或 "= 0x00000002"
    let val = s.split('=').nth(1)?.trim();
    let n = u32::from_str_radix(val.trim_start_matches("0x"), 16).ok()?;
    // 0=启用(用户管理) 1=禁用 2=系统管理(启用) 3=系统管理(禁用)
    Some(!matches!(n, 1 | 3))
}

#[cfg(not(windows))]
pub fn atime_enabled() -> Option<bool> {
    // 非 Windows：检查 /proc/mounts 的 noatime（简化：返回 None 表示未知）
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_commands_respects_operators_and_quotes() {
        let parts = split_commands("git log | rg fix && make build ; echo \"a;b\"");
        assert_eq!(
            parts,
            vec!["git log", "rg fix", "make build", "echo \"a;b\""]
        );
    }

    #[test]
    fn real_command_name_strips_assignments_and_wrappers() {
        assert_eq!(
            real_command_name("sudo env FOO=bar rg pattern").as_deref(),
            Some("rg")
        );
        assert_eq!(
            real_command_name("/opt/homebrew/bin/bat file").as_deref(),
            Some("bat")
        );
        assert_eq!(
            real_command_name("FOO=1 make build").as_deref(),
            Some("make")
        );
        assert_eq!(
            real_command_name("C:\\tools\\fd.exe .").as_deref(),
            Some("fd")
        );
    }

    #[test]
    fn real_command_name_handles_empty() {
        assert_eq!(real_command_name(""), None);
        assert_eq!(real_command_name("FOO=bar"), None);
    }

    #[test]
    fn parse_plain_history_counts_and_parses() {
        let dir = std::env::temp_dir().join(format!("oa_hist_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("history.txt");
        std::fs::write(&p, "rg foo\nmake build | rg test\nsudo bat x\n# comment\n").unwrap();
        let r = parse_plain_history(&p, SignalSource::History).unwrap();
        assert_eq!(r.total_entries, 3);
        let names: Vec<&str> = r.events.iter().map(|e| e.binary.as_str()).collect();
        assert!(names.contains(&"rg"));
        assert!(names.contains(&"make"));
        assert!(names.contains(&"bat"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shell_config_parses_alias() {
        // 直接验证 real_command_name 对 alias 目标的解析
        assert_eq!(
            real_command_name("bat --paging=never").as_deref(),
            Some("bat")
        );
    }
}

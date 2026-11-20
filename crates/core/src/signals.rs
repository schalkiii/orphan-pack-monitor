//! 信号层：把各种来源归一化成 UsageEvent。
//!
//! 三类信号：
//! 1. shell 历史（最强交互证据）—— PowerShell PSReadLine / bash / zsh / Atuin
//! 2. shell 配置与 alias（integration 证据，偏保守）
//! 3. 文件访问时间 atime（弱执行证据，无法归因）
//!
//! 关键解析规则（继承 shelfware 的经验）：
//! - 一行历史可能含多个命令：按 | && ; || 拆分
//! - 跳过 wrapper（连同其参数）：sudo / env / xargs / time / command / nohup /
//!   exec / doas / watch / nice / stdbuf / timeout
//! - 忽略 FOO=bar 前置赋值
//! - 路径取 basename，去 .exe 后缀
//! - zsh EXTENDED_HISTORY 行自带时间戳，优先于文件 mtime
//! - Atuin 存在时优先于纯文本历史文件，避免双计

use crate::inventory::strip_exe;
use crate::model::{basename, SignalSource, UsageEvent};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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

    // PowerShell PSReadLine 历史：目录下所有 host 的 *_history.txt
    // （ConsoleHost / PowerShellCore / VSCode 宿主各有独立文件，固定名单会漏）
    if let Some(dir) = psreadline_dir() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| is_psreadline_history_file(&n.to_string_lossy()))
                })
                .collect();
            files.sort();
            for p in files {
                if let Ok(r) = parse_plain_history(&p, SignalSource::History) {
                    let label = p
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    result.sources_used.push(format!("psreadline:{label}"));
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

/// PSReadLine 历史文件按 `*_history.txt` 约定命名（ConsoleHost_history.txt /
/// PowerShellCore_host_history.txt / `Visual Studio Code_host_history.txt` 等），
/// 通配识别以免新宿主漏采。
fn is_psreadline_history_file(name: &str) -> bool {
    name.ends_with("_history.txt")
}

/// 解析纯文本历史。PowerShell 的 PSReadLine 默认每行一条命令、无时间戳——
/// 但历史文件是**按时间顺序追加**的，用「文件创建时间 → 修改时间」的均匀速率
/// 按行位置推算每行的近似时间（比整批都用 mtime 真实得多：翻到 4096 行历史的
/// 第一行，那条命令几乎必然不是今天敲的）；推算不出（创建时间缺失/单行文件）
/// 回退整批 mtime。zsh EXTENDED_HISTORY 行自带真实时间戳，优先用行内时间。
pub fn parse_plain_history(path: &Path, source: SignalSource) -> Result<HistoryResult> {
    let raw = std::fs::read_to_string(path)?;
    let md = std::fs::metadata(path).ok();
    let mtime = md
        .as_ref()
        .and_then(|m| m.modified().ok())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now);
    let created = md
        .as_ref()
        .and_then(|m| m.created().ok())
        .map(DateTime::<Utc>::from);
    let lines: Vec<&str> = raw
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    // 均匀速率假设下的每行时间步长（span 均摊到 total-1 个间隔）
    let per_line = estimate_per_line_gap(created, mtime, lines.len());
    let mut events = Vec::new();
    let mut total = 0u32;
    let n = lines.len();
    for (i, line) in lines.iter().enumerate() {
        let line = *line;
        // 行位置推算：越靠后的行越新；无创建时间时回退 mtime（旧行为）
        let estimated = per_line
            .map(|gap| mtime - chrono::Duration::seconds((gap * (n - 1 - i) as f64) as i64));
        let (when, line_cmd) =
            parse_zsh_history_line(line).unwrap_or((estimated.unwrap_or(mtime), line));
        total += 1;
        for cmd in split_commands(line_cmd) {
            if let Some(bin) = real_command_name(&cmd) {
                let mut ev = UsageEvent::new(bin, when, source);
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

/// 均匀速率假设的每行时间步长（秒）：历史按时间顺序追加，span 均摊到
/// total-1 个间隔。created 缺失 / 晚于 mtime / 行数不足 2 时返回 None
/// （调用方回退整批 mtime）。
fn estimate_per_line_gap(
    created: Option<DateTime<Utc>>,
    mtime: DateTime<Utc>,
    total: usize,
) -> Option<f64> {
    let c = created.filter(|c| *c < mtime)?;
    if total < 2 {
        return None;
    }
    Some((mtime - c).num_seconds() as f64 / (total - 1) as f64)
}

/// 解析 zsh EXTENDED_HISTORY 格式行：`: <unix秒>:<耗时>;<命令>`。
/// 返回 None 表示不是该格式（普通 PSReadLine / bash 行，回退 mtime 近似）。
fn parse_zsh_history_line(line: &str) -> Option<(DateTime<Utc>, &str)> {
    let rest = line.strip_prefix(": ")?;
    let (meta, cmd) = rest.split_once(';')?;
    let ts = meta.split(':').next()?.trim().parse::<i64>().ok()?;
    let when = Utc.timestamp_opt(ts, 0).single()?;
    Some((when, cmd))
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
    // `cmd 2>&1` 在 & 处切分会产出伪命令段 "1"（fd 重定向目标）；纯数字段
    // 不可能是真实命令名，丢弃以免产生 binary="1" 的垃圾事件
    parts.retain(|p| !p.chars().all(|c| c.is_ascii_digit()));
    parts
}

const WRAPPERS: &[&str] = &[
    "sudo", "env", "xargs", "time", "command", "nohup", "exec", "doas", "watch", "nice", "stdbuf",
    "timeout",
];

/// timeout/nice/watch 这类 wrapper 的时长或间隔参数形态：纯数字、数字+时间单位
/// （10 / 10s / 1m30s / 1:30）。首个字符必须是数字，故不会误吞 7z 这类真实命令。
fn is_duration_arg(w: &str) -> bool {
    let mut chars = w.chars();
    match chars.next() {
        Some(c) if c.is_ascii_digit() => {}
        _ => return false,
    }
    w.chars()
        .skip(1)
        .all(|c| c.is_ascii_digit() || matches!(c, 's' | 'm' | 'h' | ':'))
}

/// 从一段命令中还原真实命令名：剥离前置赋值与 wrapper（含 wrapper 自己的
/// flag / 赋值 / 时长参数），取 basename。
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
    // 剥离 wrapper（可能连续多个，如 sudo env FOO=bar cmd）；每个 wrapper
    // 后面跟的是它自己的参数（-n 5 / 10s / FOO=bar），也要一并剥掉，
    // 否则 `timeout 10 make` 会被记成名为 "10" 的命令
    loop {
        match words.first() {
            Some(first) if WRAPPERS.contains(&first.to_ascii_lowercase().as_str()) => {
                words.remove(0);
                while words
                    .first()
                    .is_some_and(|w| w.contains('=') || w.starts_with('-') || is_duration_arg(w))
                {
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
            index_config_line(&mut index, &label, i + 1, trimmed);
        }
    }
    index
}

/// 把一行 shell 配置中识别到的命令引用并入索引。
/// 键统一 `to_ascii_lowercase()`：Windows 命令大小写不敏感，配置里
/// `Set-Alias Cat BAT`、裸词 `Bat` 与包的 `binaries=["Bat"]` 必须能对上，
/// 否则本应判 integration 的包会漏报成 unused（误入清理候选）。
fn index_config_line(
    index: &mut HashMap<String, Vec<String>>,
    label: &str,
    line_no: usize,
    trimmed: &str,
) {
    // alias cat='bat --paging=never' -> 记录 bat，同时记录 cat->bat 映射
    if let Some(rest) = trimmed.strip_prefix("alias ") {
        if let Some(eq) = rest.find('=') {
            let alias_name = rest[..eq].trim();
            let target = rest[eq + 1..].trim().trim_matches(['\'', '"']);
            if let Some(bin) = real_command_name(target) {
                index
                    .entry(bin.to_ascii_lowercase())
                    .or_default()
                    .push(format!("{label}:{line_no} (alias {alias_name})"));
            }
            return;
        }
    }
    // PowerShell: Set-Alias cat bat / function ...
    if let Some(rest) = trimmed.strip_prefix("Set-Alias ") {
        let words: Vec<&str> = rest.split_whitespace().collect();
        if words.len() >= 2 {
            let target = strip_exe(basename(words[1])).to_ascii_lowercase();
            index
                .entry(target)
                .or_default()
                .push(format!("{label}:{line_no} (alias)"));
        }
        return;
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
                .push(format!("{label}:{line_no}"));
        }
    }
}

/// atime 探测：给定包提供的二进制路径，取最新的访问时间作为弱执行证据，
/// 直接以 `binary_name`（命令名）构造事件。调用方需自行判断 NTFS atime 是否启用（见 atime_enabled）。
/// 安装/升级动作本身会触碰二进制文件，创建后 [ATIME_INSTALL_TOUCH_WINDOW] 内的
/// 访问无法与安装区分，不计入证据（见 atime_is_evidence）。
pub fn probe_atime(binaries: &[PathBuf], binary_name: &str) -> Option<UsageEvent> {
    let mut latest: Option<DateTime<Utc>> = None;
    for p in binaries {
        let Ok(md) = std::fs::metadata(p) else {
            continue;
        };
        let Ok(accessed) = md.accessed() else {
            continue;
        };
        if !atime_is_evidence(accessed, md.created().ok()) {
            continue;
        }
        let dt = DateTime::<Utc>::from(accessed);
        if latest.is_none_or(|l| dt > l) {
            latest = Some(dt);
        }
    }
    latest.map(|t| UsageEvent::new(binary_name, t, SignalSource::Atime))
}

/// 安装触碰窗口：文件创建后该时长内的访问更可能是包管理器写文件而非用户执行。
const ATIME_INSTALL_TOUCH_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// 一次 atime 是否可当作执行证据：
/// - 创建时间缺失（老文件系统/挂载方式）：无从判断，保守计入；
/// - 访问时间距创建不足安装触碰窗口：视为安装动作本身，剔除。
fn atime_is_evidence(accessed: SystemTime, created: Option<SystemTime>) -> bool {
    match created {
        None => true,
        Some(c) => accessed
            .duration_since(c)
            .is_ok_and(|d| d >= ATIME_INSTALL_TOUCH_WINDOW),
    }
}

/// 扫盘日判定阈值（日期层）：同一 UTC 日期内出现 atime 证据的**不同命令数**。
/// 实机取证（2026-11-20）：安全代理扫盘把 11 个包的 atime 聚在同一天且无其他
/// 信号；真实单日用 5 个全局 CLI 且零历史 + 零 ETW + 零 import 的情形，在
/// ETW 常驻机器上不成立——ETW 看到一切进程创建，atime 本就是冗余弱信号。
/// 日期层兜住跨小时慢扫盘；小规模秒级聚类由分钟层（阈值更低）负责。
const ATIME_SWEEP_MIN_BINS: usize = 5;

/// 扫盘分钟判定阈值（分钟层）：同一日历分钟内出现 atime 证据的不同命令数。
/// 实机取证：扫盘聚类为同秒/秒级（09-12 同秒 12 文件、11-05 同秒 4 命令、
/// 11-06 七秒 4 命令），日期层阈值 5 会漏掉 4 命令的小聚类——真实使用不会
/// 在同一分钟碰 4 个全局 CLI。
const ATIME_SWEEP_MIN_BINS_PER_MINUTE: usize = 4;

/// 扫盘折扣 v2：剔除「扫盘」atime 事件，返回剔除条数。两层判定 + 保护规则：
/// - 分钟层：同一日历分钟 ≥ ATIME_SWEEP_MIN_BINS_PER_MINUTE 个不同命令 →
///   该分钟的 atime 是扫盘（失败方向是拆簇漏剔，保守保证据）；
/// - 日期层：同一 UTC 日期 ≥ ATIME_SWEEP_MIN_BINS 个不同命令 → 整日是扫盘；
/// - 保护：窗口内还有任何非 atime 证据的命令，其 atime 一律保留——剔除只
///   影响「仅 atime」的包，正是被扫盘误判成 indirect 的那批。
pub fn discount_swept_atimes(events: &mut Vec<UsageEvent>) -> usize {
    use chrono::Timelike;
    use std::collections::{HashMap, HashSet};
    let mut per_day: HashMap<chrono::NaiveDate, HashSet<String>> = HashMap::new();
    let mut per_minute: HashMap<(chrono::NaiveDate, u32, u32), HashSet<String>> = HashMap::new();
    for ev in events.iter() {
        if ev.source != SignalSource::Atime {
            continue;
        }
        let t = ev.timestamp;
        per_day
            .entry(t.date_naive())
            .or_default()
            .insert(ev.binary.clone());
        per_minute
            .entry((t.date_naive(), t.hour(), t.minute()))
            .or_default()
            .insert(ev.binary.clone());
    }
    let swept_days: HashSet<chrono::NaiveDate> = per_day
        .into_iter()
        .filter(|(_, bins)| bins.len() >= ATIME_SWEEP_MIN_BINS)
        .map(|(d, _)| d)
        .collect();
    let swept_minutes: HashSet<(chrono::NaiveDate, u32, u32)> = per_minute
        .into_iter()
        .filter(|(_, bins)| bins.len() >= ATIME_SWEEP_MIN_BINS_PER_MINUTE)
        .map(|(m, _)| m)
        .collect();
    if swept_days.is_empty() && swept_minutes.is_empty() {
        return 0;
    }
    let in_sweep = |ev: &UsageEvent| -> bool {
        let t = ev.timestamp;
        swept_days.contains(&t.date_naive())
            || swept_minutes.contains(&(t.date_naive(), t.hour(), t.minute()))
    };
    // 保护规则：有历史/import/ETW 执行证据的命令 atime 保留。ETW 只认**折扣后
    // 仍存活**的事件——调用方必须先跑 discount_probe_bursts 再跑本函数（探测
    // 爆发的 ETW 事件与它触碰出的 atime 同源，不能互为保护；实机 11-18 取证：
    // 执行 06:09:24、atime 惰性刷新 06:16:23，互保曾让 7 个包躲过两层折扣）
    let protected: HashSet<String> = events
        .iter()
        .filter(|ev| ev.source != SignalSource::Atime)
        .map(|ev| ev.binary.clone())
        .collect();
    let before = events.len();
    events.retain(|ev| {
        ev.source != SignalSource::Atime || !in_sweep(ev) || protected.contains(&ev.binary)
    });
    before - events.len()
}

/// 探测爆发判定阈值：同一**非交互**父进程在同一日历分钟内拉起的**不同命令数**。
/// 实机取证（2026-11-18T06:09 UTC）：某 agent 用 GNU timeout 逐个冒烟测试
/// CLI 工具，6 秒内 9 个命令全部按普通 ETW 事件入档，8 个包被误判 indirect
/// （「executed 1x via timeout.exe」）。分钟级分组的失败方向是「拆簇漏剔」＝
/// 保守保证据，安全。
const PROBE_BURST_MIN_BINS: usize = 5;

/// 探测爆发折扣：剔除「自动化冒烟爆发」的 ETW 事件，返回剔除条数。
/// 爆发 = 同一非交互父进程（timeout.exe 之类包装器；交互 shell 下的快速连打
/// 是真实使用，不参与判定）在同一日历分钟拉起 ≥ PROBE_BURST_MIN_BINS 个不同
/// 命令，且该命令在窗口内**没有任何其他事件**（保护规则——有真实证据的命令
/// 一律保留；构建流水线真实使用的工具因此不受影响）。
pub fn discount_probe_bursts(events: &mut Vec<UsageEvent>) -> usize {
    use std::collections::{HashMap, HashSet};
    // epoch 分钟 + 父进程 → 不同命令集（只收非交互父进程的 ETW 事件）
    let mut clusters: HashMap<(i64, String), HashSet<String>> = HashMap::new();
    for ev in events.iter() {
        let Some(parent) = ev.parent.as_deref() else {
            continue;
        };
        if ev.source != SignalSource::Etw || crate::verdict::is_interactive_shell(parent) {
            continue;
        }
        clusters
            .entry((ev.timestamp.timestamp() / 60, parent.to_string()))
            .or_default()
            .insert(ev.binary.clone());
    }
    // 命中的爆发簇键（分钟+父进程）与被点名的命令集
    let mut burst_clusters: HashSet<(i64, String)> = HashSet::new();
    let mut burst_bins: HashSet<String> = HashSet::new();
    for ((minute, parent), bins) in &clusters {
        if bins.len() >= PROBE_BURST_MIN_BINS {
            burst_clusters.insert((*minute, parent.clone()));
            burst_bins.extend(bins.iter().cloned());
        }
    }
    if burst_bins.is_empty() {
        return 0;
    }
    // 爆发成员：ETW + 非交互父进程 +（分钟,父进程）恰为命中簇
    let is_member = |ev: &UsageEvent| -> bool {
        let Some(parent) = ev.parent.as_deref() else {
            return false;
        };
        ev.source == SignalSource::Etw
            && !crate::verdict::is_interactive_shell(parent)
            && burst_bins.contains(&ev.binary)
            && burst_clusters.contains(&(ev.timestamp.timestamp() / 60, parent.to_string()))
    };
    // 保护规则：有历史/import 证据，或爆发簇之外还有 ETW 执行（真实使用的
    // 命令一律保留）。atime **不算**保护——探测执行本身会触碰文件，atime 与
    // 爆发事件同源（实机 11-18：执行在 06:09，atime 惰性刷新落在 06:16），
    // 互为保护会让爆发与扫盘两个折扣互相救活对方。
    let protected: HashSet<String> = events
        .iter()
        .filter(|ev| {
            matches!(ev.source, SignalSource::History | SignalSource::Import)
                || (ev.source == SignalSource::Etw && !is_member(ev))
        })
        .map(|ev| ev.binary.clone())
        .collect();
    let before = events.len();
    events.retain(|ev| !is_member(ev) || protected.contains(&ev.binary));
    before - events.len()
}

/// 解析 `fsutil behavior query disablelastaccess` 输出（纯函数，便于单测）。
/// 英文输出形如 "DisableLastAccess = 2" 或 "= 0x00000002"；本地化系统还会带
/// 说明后缀（中文实测："= 2  (托管的系统，上次访问时间更新已启用)"）——
/// 只取 `=` 后**第一个空白分隔 token** 再做数值解析。
/// 0=启用(用户管理) 1=禁用 2=系统管理(启用) 3=系统管理(禁用)；无法解析返回 None。
pub fn parse_disablelastaccess(out: &str) -> Option<bool> {
    let val = out.split('=').nth(1)?.trim();
    let token = val.split_whitespace().next()?;
    let n = u32::from_str_radix(token.trim_start_matches("0x"), 16).ok()?;
    Some(!matches!(n, 1 | 3))
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
        cmd.creation_flags(crate::inventory::CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_disablelastaccess(&String::from_utf8_lossy(&out.stdout))
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
    fn split_commands_drops_redirect_fd_fragments() {
        // `cmd 2>&1` 不得产出名为 "1" 的伪命令段
        let parts = split_commands("cargo build 2>&1");
        assert_eq!(parts, vec!["cargo build 2>"]);
        assert_eq!(real_command_name(&parts[0]).as_deref(), Some("cargo"));
        let parts = split_commands("make 1>&2");
        assert_eq!(parts, vec!["make 1>"]);
        // 单 & 分隔的正常命令不受影响
        assert_eq!(split_commands("job_a & job_b"), vec!["job_a", "job_b"]);
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
    fn per_line_gap_estimation_and_fallbacks() {
        let now = Utc::now();
        let eight_days_ago = now - chrono::Duration::days(8);
        // 5 行、跨度 8 天 → 每行 2 天（172800 秒）；首行 ≈8 天前，末行 ≈现在
        let gap = estimate_per_line_gap(Some(eight_days_ago), now, 5).unwrap();
        assert!((gap - 172_800.0).abs() < 0.01);
        let n = 5;
        let first_est = now - chrono::Duration::seconds((gap * (n - 1) as f64) as i64);
        assert!(first_est >= eight_days_ago - chrono::Duration::minutes(1));
        assert!(first_est <= eight_days_ago + chrono::Duration::minutes(1));

        // 回退分支：created 缺失 / created 晚于 mtime / 单行
        assert!(estimate_per_line_gap(None, now, 5).is_none());
        assert!(estimate_per_line_gap(Some(now + chrono::Duration::days(1)), now, 5).is_none());
        assert!(estimate_per_line_gap(Some(eight_days_ago), now, 1).is_none());
    }

    #[test]
    fn shell_config_parses_alias() {
        // 直接验证 real_command_name 对 alias 目标的解析
        assert_eq!(
            real_command_name("bat --paging=never").as_deref(),
            Some("bat")
        );
    }

    #[test]
    fn config_index_keys_are_lowercased() {
        // Windows 命令大小写不敏感：三种建键路径产出的键都必须是小写，
        // 才能与 verdict 查询侧的小写键匹配（integration 漏报回归测试）
        let mut index = HashMap::new();
        index_config_line(&mut index, "profile.ps1", 3, "Set-Alias Cat BAT");
        assert!(
            index.contains_key("bat"),
            "Set-Alias target key: {:?}",
            index
        );
        index_config_line(&mut index, ".bashrc", 12, "alias cat='Bat --paging=never'");
        assert!(index.contains_key("bat"), "alias target key: {:?}", index);
        index_config_line(&mut index, ".bashrc", 20, "BAT file.txt");
        assert!(index.contains_key("bat"), "bare word key: {:?}", index);
    }

    #[test]
    fn parse_disablelastaccess_handles_localized_output() {
        // 英文标准输出（0/2=启用，1/3=禁用）
        assert_eq!(parse_disablelastaccess("DisableLastAccess = 2"), Some(true));
        assert_eq!(parse_disablelastaccess("DisableLastAccess = 0"), Some(true));
        assert_eq!(
            parse_disablelastaccess("DisableLastAccess = 1"),
            Some(false)
        );
        assert_eq!(
            parse_disablelastaccess("DisableLastAccess = 3"),
            Some(false)
        );
        // 十六进制形态
        assert_eq!(
            parse_disablelastaccess("DisableLastAccess = 0x00000002"),
            Some(true)
        );
        // 中文本地化带括号说明后缀（实测 Win11 中文版输出，回归审计 A1：
        // 旧实现对整段后缀做数值解析 -> None -> atime 信号被静默跳过）
        assert_eq!(
            parse_disablelastaccess("DisableLastAccess = 2  (托管的系统，上次访问时间更新已启用)"),
            Some(true)
        );
        assert_eq!(
            parse_disablelastaccess(
                "DisableLastAccess = 1  (用户管理的系统，上次访问时间更新已禁用)"
            ),
            Some(false)
        );
        // 无法解析（注意：垃圾值必须不是合法十六进制，"abc" 反而是合法的）
        assert_eq!(parse_disablelastaccess(""), None);
        assert_eq!(parse_disablelastaccess("DisableLastAccess"), None);
        assert_eq!(parse_disablelastaccess("DisableLastAccess = zz"), None);
    }

    #[test]
    fn zsh_extended_history_lines_use_line_timestamp() {
        let dir = std::env::temp_dir().join(format!("oa_zsh_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(".zsh_history");
        std::fs::write(&p, ": 1731987200:0;rg foo\nplain_cmd\n").unwrap();
        let r = parse_plain_history(&p, SignalSource::History).unwrap();
        assert_eq!(r.total_entries, 2);
        // EXTENDED_HISTORY 行用行内时间戳（1731987200 = 2024-11-19T03:33:20Z），
        // 不再落到文件 mtime 近似
        let rg = r.events.iter().find(|e| e.binary == "rg").unwrap();
        assert_eq!(rg.timestamp.to_rfc3339(), "2024-11-19T03:33:20+00:00");
        // 普通（bash/PSReadLine）行回退 mtime，仍正常解析
        assert!(r.events.iter().any(|e| e.binary == "plain_cmd"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn non_history_lines_fall_back_cleanly() {
        // 非 EXTENDED_HISTORY 格式（无 ": " 前缀 / 无分号 / 时间戳非数字）都返回 None
        assert!(parse_zsh_history_line("rg foo").is_none());
        assert!(parse_zsh_history_line(": no-ts;cmd").is_none());
        assert!(parse_zsh_history_line(": 1731987200:0 no semicolon").is_none());
    }

    #[test]
    fn real_command_name_strips_wrapper_arguments() {
        // 新增 wrapper 连同其 flag / 时长参数一起剥离，否则 `timeout 10 make`
        // 会被记成名为 "10" 的命令
        assert_eq!(
            real_command_name("timeout 10 make test").as_deref(),
            Some("make")
        );
        assert_eq!(
            real_command_name("timeout 10s cargo build").as_deref(),
            Some("cargo")
        );
        assert_eq!(
            real_command_name("watch -n 5 cargo build").as_deref(),
            Some("cargo")
        );
        assert_eq!(
            real_command_name("nice -n 10 rg pattern").as_deref(),
            Some("rg")
        );
        assert_eq!(
            real_command_name("stdbuf -oL bat file").as_deref(),
            Some("bat")
        );
        assert_eq!(real_command_name("time -p make").as_deref(), Some("make"));
        // 无参数 wrapper、嵌套 wrapper 不回退
        assert_eq!(
            real_command_name("watch cargo build").as_deref(),
            Some("cargo")
        );
        assert_eq!(
            real_command_name("sudo env FOO=bar rg pattern").as_deref(),
            Some("rg")
        );
        // 数字开头的真实命令（如 7z）不能被时长参数规则误吞
        assert_eq!(real_command_name("sudo 7z x a.zip").as_deref(), Some("7z"));
    }

    #[test]
    fn psreadline_history_files_are_matched_by_suffix() {
        // 只认 *_history.txt：新宿主文件自动覆盖，无关文件排除
        assert!(is_psreadline_history_file("ConsoleHost_history.txt"));
        assert!(is_psreadline_history_file(
            "PowerShellCore_host_history.txt"
        ));
        assert!(is_psreadline_history_file(
            "Visual Studio Code_host_history.txt"
        ));
        assert!(!is_psreadline_history_file("ConsoleHost_history.txt.bak"));
        assert!(!is_psreadline_history_file("settings.json"));
    }

    #[test]
    fn probe_burst_via_wrapper_is_discounted() {
        // 实机形态：timeout.exe 6 秒内逐个拉起工具（5 个不同命令即阈值）
        let minute = Utc.with_ymd_and_hms(2026, 11, 18, 6, 9, 24).unwrap();
        let mut evs = Vec::new();
        for i in 0..5 {
            let mut ev = UsageEvent::new(format!("tool{i}"), minute, SignalSource::Etw);
            ev.parent = Some("timeout.exe".into());
            evs.push(ev);
        }
        let removed = discount_probe_bursts(&mut evs);
        assert_eq!(removed, 5);
        assert!(evs.is_empty());
    }

    #[test]
    fn interactive_parent_burst_is_not_discounted() {
        // 用户在交互 shell 里快速连打 5 个命令：真实使用，不参与爆发判定
        let minute = Utc.with_ymd_and_hms(2026, 11, 18, 6, 9, 24).unwrap();
        let mut evs = Vec::new();
        for i in 0..5 {
            let mut ev = UsageEvent::new(format!("tool{i}"), minute, SignalSource::Etw);
            ev.parent = Some("pwsh.exe".into());
            evs.push(ev);
        }
        assert_eq!(discount_probe_bursts(&mut evs), 0);
        assert_eq!(evs.len(), 5);
    }

    #[test]
    fn protected_binary_keeps_its_burst_events() {
        // 爆发命令若有任何其他事件（此处的历史记录）→ 保护：整组保留；
        // 其余 4 个无保护命令照常剔除
        let minute = Utc.with_ymd_and_hms(2026, 11, 18, 6, 9, 24).unwrap();
        let mut evs = Vec::new();
        for i in 0..5 {
            let mut ev = UsageEvent::new(format!("tool{i}"), minute, SignalSource::Etw);
            ev.parent = Some("timeout.exe".into());
            evs.push(ev);
        }
        evs.push(UsageEvent::new("tool0", minute, SignalSource::History));
        let removed = discount_probe_bursts(&mut evs);
        assert_eq!(removed, 4);
        assert!(evs
            .iter()
            .any(|e| e.binary == "tool0" && e.source == SignalSource::Etw));
    }

    #[test]
    fn small_cluster_and_spread_usage_are_untouched() {
        let t0 = Utc.with_ymd_and_hms(2026, 11, 18, 6, 9, 0).unwrap();
        let mut evs = Vec::new();
        // 4 个命令同分钟（低于阈值）：保留
        for i in 0..4 {
            let mut ev = UsageEvent::new(format!("few{i}"), t0, SignalSource::Etw);
            ev.parent = Some("timeout.exe".into());
            evs.push(ev);
        }
        // 同一 agent 父进程的长期使用（实机 headroom-ai 78× 形态）：分散在
        // 不同分钟，每分钟 1 个命令，无爆发
        for i in 0..6 {
            let mut ev = UsageEvent::new(
                "headroom",
                t0 + chrono::Duration::minutes(i),
                SignalSource::Etw,
            );
            ev.parent = Some("DeepSeek Harness.exe".into());
            evs.push(ev);
        }
        assert_eq!(discount_probe_bursts(&mut evs), 0);
        assert_eq!(evs.len(), 10);
    }

    #[test]
    fn atime_does_not_protect_burst_events() {
        // 实机 11-18 形态的回归测试：探测执行（ETW 爆发）与文件触碰（atime）
        // 同源——atime 不得作为爆发的保护，否则爆发与扫盘两个折扣互相救活
        let exec = Utc.with_ymd_and_hms(2026, 11, 18, 6, 9, 24).unwrap();
        let touched = Utc.with_ymd_and_hms(2026, 11, 18, 6, 16, 23).unwrap();
        let mut evs = Vec::new();
        for b in ["pytest", "tqdm", "typer", "jsonschema", "playwright"] {
            let mut ev = UsageEvent::new(b, exec, SignalSource::Etw);
            ev.parent = Some("timeout.exe".into());
            evs.push(ev);
            evs.push(UsageEvent::new(b, touched, SignalSource::Atime));
        }
        // 先爆发折扣：atime 不构成保护 → 5 条爆发 ETW 全剔
        assert_eq!(discount_probe_bursts(&mut evs), 5);
        // 再扫盘折扣：atime 同秒聚类（≥4）且 ETW 已不在（无保护）→ 全剔
        assert_eq!(discount_swept_atimes(&mut evs), 5);
        assert!(evs.is_empty());
    }

    #[test]
    fn swept_atime_days_are_discounted() {
        let swept_day = Utc.with_ymd_and_hms(2026, 9, 12, 6, 0, 0).unwrap();
        let normal_day = Utc.with_ymd_and_hms(2026, 10, 1, 6, 0, 0).unwrap();
        let mut evs = Vec::new();
        // 扫盘日：5 个不同命令的 atime（同一命令多条只计一次——再补 2 条重复）
        for i in 0..5 {
            evs.push(UsageEvent::new(
                format!("bin{i}"),
                swept_day,
                SignalSource::Atime,
            ));
        }
        evs.push(UsageEvent::new(
            "bin0",
            swept_day + chrono::Duration::hours(2),
            SignalSource::Atime,
        ));
        evs.push(UsageEvent::new(
            "bin1",
            swept_day + chrono::Duration::hours(3),
            SignalSource::Atime,
        ));
        // 同日的非 atime 事件（真实执行）：不受折扣影响
        evs.push(UsageEvent::new("bin0", swept_day, SignalSource::Etw));
        // 非扫盘日：仅 4 个不同命令、且分散在不同分钟 → 保留
        for i in 0..4 {
            evs.push(UsageEvent::new(
                format!("tool{i}"),
                normal_day + chrono::Duration::hours(i as i64),
                SignalSource::Atime,
            ));
        }
        let removed = discount_swept_atimes(&mut evs);
        assert_eq!(removed, 5, "扫盘日无保护命令的 atime 全剔（bin1 重复也剔）");
        // 保护规则（v2）：bin0 有 ETW 真实执行 → 其扫盘日 atime（6:00 与 8:00）保留
        assert_eq!(
            evs.iter()
                .filter(|e| e.binary == "bin0" && e.source == SignalSource::Atime)
                .count(),
            2
        );
        assert!(
            evs.iter().all(|e| e.source != SignalSource::Atime
                || e.binary == "bin0"
                || e.timestamp.date_naive() == normal_day.date_naive()),
            "非扫盘日 atime 保留；扫盘日仅保护命令的 atime 保留"
        );
        assert!(
            evs.iter().any(|e| e.source == SignalSource::Etw),
            "非 atime 事件不动"
        );
    }

    #[test]
    fn sweep_threshold_is_exactly_five_distinct_bins() {
        let day = Utc.with_ymd_and_hms(2026, 9, 12, 6, 0, 0).unwrap();
        // 恰好阈值：剔除
        let mut evs: Vec<UsageEvent> = (0..ATIME_SWEEP_MIN_BINS)
            .map(|i| UsageEvent::new(format!("b{i}"), day, SignalSource::Atime))
            .collect();
        assert_eq!(discount_swept_atimes(&mut evs), ATIME_SWEEP_MIN_BINS);
        // 日期阈值之下、且分散在不同分钟（分钟层也不触发）：全保留
        let mut evs: Vec<UsageEvent> = (0..ATIME_SWEEP_MIN_BINS - 1)
            .map(|i| {
                UsageEvent::new(
                    format!("b{i}"),
                    day + chrono::Duration::hours(i as i64),
                    SignalSource::Atime,
                )
            })
            .collect();
        assert_eq!(discount_swept_atimes(&mut evs), 0);
        assert_eq!(evs.len(), ATIME_SWEEP_MIN_BINS - 1);
        // 无 atime 事件：零剔除
        let mut evs = vec![UsageEvent::new("x", day, SignalSource::History)];
        assert_eq!(discount_swept_atimes(&mut evs), 0);
    }

    #[test]
    fn same_minute_atime_cluster_is_discounted_even_below_day_threshold() {
        // 实机 11-05/11-06 形态：同秒 4 个命令的 atime（低于日期层阈值 5，
        // 旧实现漏剔）——分钟层负责
        let minute = Utc.with_ymd_and_hms(2026, 11, 5, 16, 54, 41).unwrap();
        let mut evs = Vec::new();
        for i in 0..4 {
            evs.push(UsageEvent::new(
                format!("m{i}"),
                minute,
                SignalSource::Atime,
            ));
        }
        assert_eq!(discount_swept_atimes(&mut evs), 4);
        // 3 个命令同分钟：真实使用可能，保留
        let mut evs = Vec::new();
        for i in 0..3 {
            evs.push(UsageEvent::new(
                format!("s{i}"),
                minute,
                SignalSource::Atime,
            ));
        }
        assert_eq!(discount_swept_atimes(&mut evs), 0);
        // 保护：有历史证据的命令在扫盘分钟的 atime 保留，其余剔除
        let mut evs = vec![
            UsageEvent::new("p0", minute, SignalSource::Atime),
            UsageEvent::new("p1", minute, SignalSource::Atime),
            UsageEvent::new("p2", minute, SignalSource::Atime),
            UsageEvent::new("p3", minute, SignalSource::Atime),
            UsageEvent::new("p0", minute, SignalSource::History),
        ];
        assert_eq!(discount_swept_atimes(&mut evs), 3);
        assert!(evs
            .iter()
            .any(|e| e.binary == "p0" && e.source == SignalSource::Atime));
    }

    #[test]
    fn slow_same_day_sweep_is_discounted_by_day_layer() {
        // 慢扫盘：同日 5 命令分散 5 个小时（每分钟都低于分钟层阈值），日期层兜住
        let day = Utc.with_ymd_and_hms(2026, 9, 12, 6, 0, 0).unwrap();
        let mut evs: Vec<UsageEvent> = (0..5)
            .map(|i| {
                UsageEvent::new(
                    format!("d{i}"),
                    day + chrono::Duration::hours(i as i64),
                    SignalSource::Atime,
                )
            })
            .collect();
        assert_eq!(discount_swept_atimes(&mut evs), 5);
    }

    #[test]
    fn atime_install_touch_window_filters_fresh_files() {
        let now = SystemTime::now();
        // 创建时间缺失：无从判断，保守计入
        assert!(atime_is_evidence(now, None));
        // 创建后马上被访问（安装动作本身）：剔除
        assert!(!atime_is_evidence(now, Some(now - Duration::from_secs(60))));
        // 创建后超过 24h 才访问：执行证据成立
        assert!(atime_is_evidence(
            now,
            Some(now - Duration::from_secs(48 * 60 * 60))
        ));
    }
}

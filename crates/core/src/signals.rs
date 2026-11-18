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

/// 解析纯文本历史。PowerShell 的 PSReadLine 默认每行一条命令、无时间戳，
/// 因此用文件修改时间作为整批事件的近似时间（保守：视为较旧，不会误判为 active）；
/// zsh EXTENDED_HISTORY 行自带时间戳（见 parse_zsh_history_line），优先用行内时间。
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
        let (when, line_cmd) = parse_zsh_history_line(line).unwrap_or((mtime, line));
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

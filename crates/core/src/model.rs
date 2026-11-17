//! 数据模型：整个系统的公共语言。
//! 所有采集层、信号层、判定层、展示层都围绕这四个结构交互。
//! 设计原则：结构只承载事实，不承载结论；判定由 verdict 层独立计算。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 包管理器种类。每个种类对应 inventory 层的一个采集器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Manager {
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

impl Manager {
    /// 稳定短名，用于命令行参数与配置文件。
    pub fn as_str(&self) -> &'static str {
        match self {
            Manager::Cargo => "cargo",
            Manager::Npm => "npm",
            Manager::Pip => "pip",
            Manager::Uv => "uv",
            Manager::Pipx => "pipx",
            Manager::Go => "go",
            Manager::Brew => "brew",
            Manager::Scoop => "scoop",
            Manager::Winget => "winget",
        }
    }

    /// 该管理器安装的二进制目录是否可直接枚举（决定二进制映射置信度）。
    pub fn binary_mapping_confidence(&self) -> Confidence {
        match self {
            // cargo/npm/uv/pipx 都能从清单或 package.json 精确读出 bin 名
            Manager::Cargo | Manager::Npm | Manager::Uv | Manager::Pipx => Confidence::Exact,
            // pip 的 CLI 需从 entry_points / Scripts 目录推断
            Manager::Pip | Manager::Scoop | Manager::Winget | Manager::Brew => {
                Confidence::Inferred
            }
            // go install 不保留包清单，每个二进制自成一条
            Manager::Go => Confidence::None,
        }
    }
}

impl std::fmt::Display for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 二进制名映射的置信度。低置信度在报告中必须显式提示用户复核。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    Exact,
    Inferred,
    None,
}

/// 一个已安装的全局包。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Package {
    /// 包的全名（npm 保留 scope，如 @scope/tool）。
    pub name: String,
    /// 来源管理器。
    pub manager: Manager,
    /// 已安装版本（若可得）。
    pub version: Option<String>,
    /// 该包提供的可执行文件名列表。用户敲的是 bin 名，不是包名，
    /// 所以必须建立包->二进制的映射才能与信号匹配。
    pub binaries: Vec<String>,
    /// 安装根目录（用于体积统计与 atime 探测）。
    pub install_path: Option<PathBuf>,
    /// 安装时间（若文件时间戳可得）。
    pub installed_at: Option<DateTime<Utc>>,
    /// 占用字节数（延迟计算，采集阶段可为 None）。
    pub size_bytes: Option<u64>,
    /// 二进制名映射的置信度。
    pub confidence: Confidence,
    /// 该包是否是被别人依赖的库而非用户直接安装（如 brew leaves 的反向）。
    /// 依赖包永不进入清理候选。
    pub is_dependency: bool,
}

impl Package {
    pub fn new(name: impl Into<String>, manager: Manager) -> Self {
        Self {
            name: name.into(),
            manager,
            version: None,
            binaries: Vec::new(),
            install_path: None,
            installed_at: None,
            size_bytes: None,
            confidence: Confidence::None,
            is_dependency: false,
        }
    }

    /// 判断一个命令名是否属于本包。
    pub fn owns_binary(&self, command: &str) -> bool {
        let command = basename(command);
        self.binaries.iter().any(|b| basename(b) == command)
            || basename(&self.name) == command
    }
}

/// 信号来源。判定引擎按来源区分「用户交互」与「间接调用」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalSource {
    /// shell 历史：用户显式敲过的命令（最强交互证据）。
    History,
    /// ETW 进程事件：系统级精确执行记录，含父进程与命令行。
    Etw,
    /// 文件访问时间：只能证明「跑过」，不能证明「谁跑、跑几次」。
    Atime,
    /// shell 启动配置（.bashrc/.zshrc/PowerShell profile 等）。
    ShellConfig,
    /// alias / function 定义：cat -> bat 这类重定向。
    Alias,
    /// 命令行里被 wrapper（sudo/env/xargs）包裹后还原出的真实命令。
    Wrapped,
}

impl SignalSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SignalSource::History => "history",
            SignalSource::Etw => "etw",
            SignalSource::Atime => "atime",
            SignalSource::ShellConfig => "shell-config",
            SignalSource::Alias => "alias",
            SignalSource::Wrapped => "wrapped",
        }
    }
}

/// 一条使用事件。信号层把各种来源统一归一化为此结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageEvent {
    /// 被执行的命令名（basename，不含路径）。
    pub binary: String,
    /// 事件发生时间。
    pub timestamp: DateTime<Utc>,
    /// 信号来源。
    pub source: SignalSource,
    /// 原始命令行（若可得）。ETW 提供完整命令行，history 提供原始行。
    pub command_line: Option<String>,
    /// 父进程名（ETW 特有）。用于区分用户 shell 直接调用 vs 脚本/CI/agent 调用。
    pub parent: Option<String>,
    /// 该来源的重复次数（history 计数 / ETW 事件数）。
    pub count: u32,
}

impl UsageEvent {
    pub fn new(binary: impl Into<String>, timestamp: DateTime<Utc>, source: SignalSource) -> Self {
        Self {
            binary: binary.into(),
            timestamp,
            source,
            command_line: None,
            parent: None,
            count: 1,
        }
    }

    /// 是否为交互式输入（用户亲手敲）。
    pub fn is_interactive(&self) -> bool {
        matches!(self.source, SignalSource::History | SignalSource::Wrapped)
    }

    /// 是否为系统级执行证据（含 ETW 与 atime）。
    pub fn is_execution(&self) -> bool {
        matches!(self.source, SignalSource::Etw | SignalSource::Atime)
    }
}

/// 五档判定。顺序即检查优先级（从强到弱），数值越大越接近「可删」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// 交互式使用频繁，无需质疑。
    Active = 0,
    /// 近期有执行证据，但非用户直接输入（脚本/CI/agent）。
    Indirect = 1,
    /// 历史里出现过，但 --since 窗口内无执行。
    Rare = 2,
    /// 从未执行，但被 shell 配置 / alias 引用。
    Integration = 3,
    /// 五路信号全静默，唯一进入清理候选的档位。
    Unused = 4,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Active => "active",
            Verdict::Indirect => "indirect",
            Verdict::Rare => "rare",
            Verdict::Integration => "integration",
            Verdict::Unused => "unused",
        }
    }

    /// 该判定是否可以进入清理候选。
    pub fn is_removable(&self) -> bool {
        *self == Verdict::Unused
    }

    /// 面向用户的解释文案。
    pub fn description(&self) -> &'static str {
        match self {
            Verdict::Active => "Typed regularly at your prompt",
            Verdict::Indirect => "Executed recently, but not by you typing it",
            Verdict::Rare => "Typed occasionally, and nothing has run it lately",
            Verdict::Integration => "Never executed, but referenced by shell config",
            Verdict::Unused => "No signal from any source — removal candidate",
        }
    }
}

/// 一条判定证据。每行 verdict 都附带全部命中的证据，供用户反驳判定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub source: SignalSource,
    /// 人类可读的证据细节，如 "typed 3x" / "in ~/.bashrc:12" / "ETW parent=make"。
    pub detail: String,
    /// 最近一次该来源的信号时间。
    pub last_seen: Option<DateTime<Utc>>,
    /// 命中次数（历史条数 / ETW 事件数）。
    pub hits: u32,
}

impl Evidence {
    pub fn new(source: SignalSource, detail: impl Into<String>) -> Self {
        Self {
            source,
            detail: detail.into(),
            last_seen: None,
            hits: 0,
        }
    }

    /// 链式设置最近一次信号时间。
    pub fn with_last(mut self, last_seen: Option<DateTime<Utc>>) -> Self {
        self.last_seen = last_seen;
        self
    }

    /// 链式设置命中次数。
    pub fn with_hits(mut self, hits: u32) -> Self {
        self.hits = hits;
        self
    }
}

/// 一个包的完整判定结果（最终产物）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerdictResult {
    pub package: Package,
    pub verdict: Verdict,
    /// 交互式输入命中次数。
    pub interactive_hits: u32,
    /// 距最近一次执行证据的天数；无任何执行证据时为 None。
    pub last_exec_days_ago: Option<u32>,
    /// 是否出现在 shell 配置中。
    pub in_shell_config: bool,
    /// 完整证据链。
    pub evidence: Vec<Evidence>,
}

/// 审计配置。命令行参数与配置文件最终都归一化为此结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditConfig {
    /// 「多久内执行过仍算在用」，默认 30 天。接受 30d/6w/6mo/1y。
    pub since_days: u32,
    /// 判定 active 所需的交互次数阈值。None 表示按历史规模自适应。
    pub active_hits: Option<u32>,
    /// 永久忽略的包名或二进制名。
    pub ignore: Vec<String>,
    /// 只审计这些管理器；空表示全部。
    pub managers: Vec<Manager>,
    /// 是否在 shell 配置中匹配到裸词就判 integration（宽松模式，偏保守）。
    pub loose_integration: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            since_days: 30,
            active_hits: None,
            ignore: Vec::new(),
            managers: Vec::new(),
            loose_integration: true,
        }
    }
}

/// 去掉路径，只保留命令名。/opt/homebrew/bin/rg -> rg
pub fn basename(s: &str) -> &str {
    // 同时处理 Unix / 与 Windows \ 分隔符
    s.rsplit(['/', '\\']).next().unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_handles_both_separators() {
        assert_eq!(basename("/opt/homebrew/bin/rg"), "rg");
        assert_eq!(basename(r"C:\Users\x\.cargo\bin\bat.exe"), "bat.exe");
        assert_eq!(basename("plain"), "plain");
    }

    #[test]
    fn verdict_ordering_is_weak_to_strong() {
        assert!(Verdict::Active < Verdict::Indirect);
        assert!(Verdict::Indirect < Verdict::Rare);
        assert!(Verdict::Rare < Verdict::Integration);
        assert!(Verdict::Integration < Verdict::Unused);
        assert!(Verdict::Unused.is_removable());
        assert!(!Verdict::Active.is_removable());
    }

    #[test]
    fn package_owns_binary_by_basename() {
        let mut p = Package::new("ripgrep", Manager::Cargo);
        p.binaries.push("rg".into());
        assert!(p.owns_binary("rg"));
        assert!(p.owns_binary("/usr/bin/rg"));
        assert!(!p.owns_binary("ripgrep-all"));
    }

    #[test]
    fn usage_event_interactive_vs_execution() {
        let now = Utc::now();
        let h = UsageEvent::new("rg", now, SignalSource::History);
        assert!(h.is_interactive());
        assert!(!h.is_execution());
        let e = UsageEvent::new("rg", now, SignalSource::Etw);
        assert!(!e.is_interactive());
        assert!(e.is_execution());
        let a = UsageEvent::new("rg", now, SignalSource::Atime);
        assert!(a.is_execution());
    }
}

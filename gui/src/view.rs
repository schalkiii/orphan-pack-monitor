//! GUI 状态层：把 core 的审计结果映射成 UI 可直接渲染的扁平视图。
//! 遵循 core 与 GUI 解耦原则：这里只持有渲染所需数据，不重新实现判定逻辑。
//! 界面文案中文化：core 产出的英文证据细节在此做展示层翻译（格式由 verdict.rs 固定）。

use orphan_audit_core::model::{
    Confidence, DetectedEnv, EnvKind, Manager, SignalSource, Verdict, VerdictResult,
};

/// 单个包在表格里的一行。
#[derive(Debug, Clone)]
pub struct Row {
    pub index: usize,
    pub name: String,
    pub manager: String,
    /// 包所在环境的短名（pip = 解释器环境、npm = 全局 prefix）。
    /// 归属环境短名（npm = prefix 名 /「默认」，pip = 解释器环境短名）；
    /// 无环境语义的管理器（cargo / go / winget）为 None。单环境也标注——
    /// 表格「环境」列据此逐行显示。
    pub env_label: Option<String>,
    /// 安装位置完整路径（证据面板展示，多实例时是归属的最终依据）。
    pub install_path: Option<String>,
    /// 能否由 GUI 直接卸载（core 给得出卸载命令才显示勾选框；go 无清单包不可）。
    pub removable: bool,
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
    /// 证据链（来源 + 细节文案，供证据面板做源色 badge 与悬停说明）。
    pub evidence: Vec<EvidenceLine>,
    /// 新装观察期标注（不是信号证据，单独渲染）。
    pub observation: bool,
    pub confidence: String,
}

/// 一条证据的展示单元：来源（决定 badge 配色与 ？ 说明内容）+ 细节文案。
#[derive(Debug, Clone)]
pub struct EvidenceLine {
    pub source: SignalSource,
    /// 细节（展示层中文）+ 可选「最近时间」后缀。
    pub text: String,
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
    /// 观察期包数（新装不满一个窗口，不进清理候选）。
    pub observing: usize,
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
            if r.newly_installed {
                s.observing += 1;
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
        SignalSource::Import => "import",
        SignalSource::Atime => "访问时间",
        SignalSource::ShellConfig => "配置",
        SignalSource::Alias => "别名",
        SignalSource::Wrapped => "还原命令",
        SignalSource::Drift => "长期未用",
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
    if let Some(rest) = detail.strip_prefix("imported ") {
        if let Some(n) = rest.strip_suffix('x') {
            return format!("导入 {n} 次（import 钩子）");
        }
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
        "no signal from any source" => "窗口内无任何使用记录".to_string(),
        other => other.to_string(),
    }
}

/// 一条证据的展示单元（细节中文 + 可选最近时间；来源另放 badge，见 EvidenceLine）。
fn evidence_line(e: &orphan_audit_core::model::Evidence) -> EvidenceLine {
    let detail = detail_cn(&e.detail);
    EvidenceLine {
        source: e.source,
        text: match e.last_seen {
            Some(t) => format!("{} · 最近 {}", detail, t.format("%Y-%m-%d")),
            None => detail,
        },
    }
}

/// 消息开头的十进制计数："4 orphan shim …" → (4, 余下文本)；无数字 → None。
fn lead_num(s: &str) -> Option<(u64, &str)> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    s[..end]
        .parse::<u64>()
        .ok()
        .map(|n| (n, s[end..].trim_start()))
}

/// 审计警告 → 中文（GUI 警告横幅展示用）。core 的警告是自由文本（含路径、
/// 计数、错误详情等载荷），此处按已知前缀解析重组为中文；识别不了的整条
/// 原样透传（与 detail_cn 同一策略——core 措辞演进时 GUI 不会显示空白）。
pub fn warning_cn(w: &str) -> String {
    let Some((tag, rest)) = w.strip_prefix('[').and_then(|s| s.split_once(']')) else {
        return w.to_string();
    };
    let rest = rest.trim_start();
    let body = match tag {
        "npm" => match lead_num(rest) {
            Some((n, tail)) if tail.starts_with("orphan shim set") => format!(
                "检测到 {n} 组卸载残骸——包已卸载，但启动器文件（X / X.cmd / X.ps1）还留在 PATH 目录，敲命令会报错；clean 脚本附带逐文件删除行"
            ),
            _ => rest.to_string(),
        },
        "etw" => match lead_num(rest) {
            Some((n, tail)) if tail.starts_with("one-shot executions") => format!(
                "{n} 次单次执行疑似自动化批量探测（agent / 安全工具的冒烟测试），已整簇剔除、不计为使用证据"
            ),
            _ if rest.starts_with("newest event is ") => {
                let days = rest["newest event is ".len()..]
                    .split('d')
                    .next()
                    .and_then(|d| d.parse::<u64>().ok());
                match days {
                    Some(d) => {
                        format!("最新 ETW 事件已是 {d} 天前——采集器可能停了，近几天的使用可能没采到")
                    }
                    None => rest.to_string(),
                }
            }
            _ if rest.starts_with("log read failed: ") => {
                format!("ETW 事件日志读取失败：{}", &rest["log read failed: ".len()..])
            }
            _ => rest.to_string(),
        },
        "atime" => match lead_num(rest) {
            Some((n, tail)) if tail.starts_with("access-time hits") => format!(
                "{n} 个访问时间命中疑似安全软件扫盘（同一时段秒级连碰一批文件，非真实使用），已剔除"
            ),
            _ if rest.starts_with("NTFS last-access updates are disabled") => {
                "NTFS 访问时间更新已被禁用——该信号跳过".into()
            }
            _ if rest.starts_with("could not determine last-access state") => {
                "无法判断 NTFS 访问时间状态——该信号跳过".into()
            }
            _ => rest.to_string(),
        },
        "imports" => {
            if rest.starts_with("log was quarantined") {
                let path = rest
                    .split_once('(')
                    .and_then(|(_, r)| r.split_once(')'))
                    .map(|(p, _)| p)
                    .unwrap_or("");
                format!("import 日志被安全软件整文件隔离（{path}）——import 证据暂不可用；你自己的进程下次 import 时会自动重建")
            } else if rest.starts_with("log missing but hooks are installed") {
                "钩子已安装但日志还不存在——尚未记录到任何 import 事件（或日志被移走）".into()
            } else if let Some(size_rest) = rest.strip_prefix("log is ") {
                let mib = size_rest
                    .split_whitespace()
                    .next()
                    .and_then(|x| x.parse::<u64>().ok());
                let path = rest
                    .split_once('(')
                    .and_then(|(_, r)| r.split_once(')'))
                    .map(|(p, _)| p)
                    .unwrap_or("");
                match mib {
                    Some(m) => format!("import 日志已达 {m} MiB（{path}）；可手动清理旧行"),
                    None => rest.to_string(),
                }
            } else if let Some(detail) = rest.strip_prefix("log read failed: ") {
                format!("import 日志读取失败：{detail}")
            } else {
                rest.to_string()
            }
        }
        "signal" if rest.starts_with("no ETW events") => {
            "观察窗口内没有任何 ETW 事件——判定仅依赖 shell 历史 + 访问时间；以管理员运行 `etw-capture install` 可开启常驻采集".into()
        }
        "store" if rest.starts_with("inventory save failed: ") => format!(
            "包清单保存失败：{}",
            &rest["inventory save failed: ".len()..]
        ),
        "store" if rest.starts_with("snapshot save failed: ") => format!(
            "月度快照保存失败：{}",
            &rest["snapshot save failed: ".len()..]
        ),
        "store" if rest.starts_with("open failed: ") => format!(
            "本地数据库打开失败：{}",
            &rest["open failed: ".len()..]
        ),
        // 未知来源的警告整条透传
        _ => return w.to_string(),
    };
    format!("[{tag}] {body}")
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

/// 管理器标签行的数据：审计结果里出现过的管理器及各自包数。
/// 按固定顺序返回（cargo → npm → pip → …），本机未安装的管理器不出现。
pub fn manager_counts(results: &[VerdictResult]) -> Vec<(Manager, usize)> {
    const ORDER: [Manager; 9] = [
        Manager::Cargo,
        Manager::Npm,
        Manager::Pip,
        Manager::Uv,
        Manager::Pipx,
        Manager::Winget,
        Manager::Scoop,
        Manager::Go,
        Manager::Brew,
    ];
    ORDER
        .into_iter()
        .map(|m| (m, results.iter().filter(|r| r.package.manager == m).count()))
        .filter(|(_, n)| *n > 0)
        .collect()
}

/// 检测环境行的数据：npm prefix 与 python 两组短标签（保留检测顺序）。
/// `npm_default` 用于把默认 prefix 标成「默认」而非目录名（调用方传
/// `inventory::npm_default_prefix()`；None 时全部按目录名展示）。
#[derive(Debug, Default, PartialEq)]
pub struct EnvChips {
    pub npm: Vec<EnvChip>,
    pub python: Vec<EnvChip>,
}

/// 一枚环境标签：短名（行内展示）+ 完整路径（悬停提示）。
#[derive(Debug, PartialEq)]
pub struct EnvChip {
    pub label: String,
    pub full_path: String,
}

pub fn env_chips(envs: &[DetectedEnv], npm_default: Option<&std::path::Path>) -> EnvChips {
    let mut chips = EnvChips::default();
    for env in envs {
        let (list, label) = match env.kind {
            EnvKind::NpmPrefix => (&mut chips.npm, npm_env_label(&env.path, npm_default)),
            EnvKind::Python => (&mut chips.python, python_env_label(&env.path)),
        };
        list.push(EnvChip {
            label,
            full_path: path_display(&env.path.to_string_lossy()),
        });
    }
    chips
}

/// npm prefix 短名：默认 prefix →「默认」，其余取末级目录名（如 npm-global / v22.3.0）。
fn npm_env_label(path: &std::path::Path, default: Option<&std::path::Path>) -> String {
    if default.is_some_and(|d| {
        path.to_string_lossy()
            .eq_ignore_ascii_case(&d.to_string_lossy())
    }) {
        "默认".into()
    } else {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "npm".into())
    }
}

/// python 短名：解释器所在目录名；uv 托管的 cpython-X.Y.Z-windows-… 截成 X.Y.Z。
fn python_env_label(path: &std::path::Path) -> String {
    let dir = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    shorten_python_dir(&dir)
}

/// python 环境目录名的短名规则（环境行与表格行共用，保持同一环境同一名字）。
fn shorten_python_dir(dir: &str) -> String {
    if let Some((stem, _)) = dir.split_once("-windows") {
        if let Some(version) = stem.strip_prefix("cpython-") {
            return version.to_string();
        }
    }
    if dir.is_empty() {
        "python".into()
    } else {
        dir.to_string()
    }
}

/// 路径展示文本：在每个反斜杠后插零宽空格（U+200B），给折行提供目录边界的断点。
/// gpui 的断点跟 UAX #14 走，`\` 属字母类不产生断点——长路径会在 `:`、`@` 等
/// 标点后被切成碎段（实测 "C:" 独占一行、`@` 后腰斩）；零宽空格本身零宽不可见，
/// 断行发生在它之后，行尾保留 `\` 表意「路径未完」。
pub fn path_display(path: &str) -> String {
    path.replace('\\', "\\\u{200b}")
}

/// 包所在环境的短名：pip 按解释器环境、npm 按全局 prefix，其余管理器单环境返回 None。
/// install_path 语义见 inventory：pip = site-packages 目录；npm = <prefix>\node_modules\<pkg>。
pub fn package_env_label(
    manager: Manager,
    install_path: Option<&std::path::Path>,
    npm_default: Option<&std::path::Path>,
) -> Option<String> {
    let path = install_path?;
    match manager {
        Manager::Pip => {
            // site-packages 的上一层是环境根（用户级布局）或 Lib（系统/venv 布局）
            let root = path.parent()?;
            let env_dir = match root.file_name().and_then(|n| n.to_str()) {
                Some(lib) if lib.eq_ignore_ascii_case("lib") => root.parent()?,
                _ => root,
            };
            Some(shorten_python_dir(
                &env_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ))
        }
        Manager::Npm => {
            // 沿目录向上找 node_modules，其父目录即 prefix（scoped 包多一层 @scope）
            let mut cur = path.parent()?;
            loop {
                if cur
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case("node_modules"))
                {
                    break;
                }
                cur = cur.parent()?;
            }
            let prefix = cur.parent()?;
            if npm_default.is_some_and(|d| {
                prefix
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&d.to_string_lossy())
            }) {
                Some("默认".into())
            } else {
                Some(
                    prefix
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "npm".into()),
                )
            }
        }
        _ => None,
    }
}

/// 把审计结果转换为表格行，应用筛选与排序（排序维度 + 方向）。
/// `verdict_filter`：Some(v) 只保留该档位的包（分布条/图例点击），None 不过滤。
/// `npm_default`：默认全局 prefix（标「默认」用，与「环境」行同一判定）。
pub fn build_rows(
    results: &[VerdictResult],
    verdict_filter: Option<Verdict>,
    manager_filter: Option<Manager>,
    sort: SortKey,
    dir: SortDir,
    npm_default: Option<&std::path::Path>,
) -> Vec<Row> {
    // 每行都标注归属环境短名（用户指令：单环境也显示那一个）——
    // 无环境语义的管理器（cargo / go / winget）为 None，表格显示「—」
    let mut rows: Vec<Row> = results
        .iter()
        .enumerate()
        .filter(|(_, r)| verdict_filter.is_none_or(|v| r.verdict == v))
        .filter(|(_, r)| manager_filter.is_none_or(|m| r.package.manager == m))
        .map(|(i, r)| Row {
            index: i,
            name: r.package.name.clone(),
            manager: r.package.manager.as_str().to_string(),
            env_label: package_env_label(
                r.package.manager,
                r.package.install_path.as_deref(),
                npm_default,
            ),
            install_path: r
                .package
                .install_path
                .as_ref()
                .map(|p| p.display().to_string()),
            removable: orphan_audit_core::actions::uninstall_command(r).is_some(),
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
            evidence: r.evidence.iter().map(evidence_line).collect(),
            observation: r.newly_installed,
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
        let rows = build_rows(&results, None, None, SortKey::Verdict, SortDir::Asc, None);
        assert_eq!(rows[0].name, "beta");
        let sel = row_by_index(&rows, 0).expect("alpha must stay resolvable after sort");
        assert_eq!(sel.name, "alpha");
        assert!(row_by_index(&rows, 2).is_none());
    }

    #[test]
    fn verdict_filter_keeps_only_that_tier() {
        let results = vec![
            result("alpha", "alpha", Verdict::Active, None),
            result("beta", "beta", Verdict::Unused, None),
            result("gamma", "gamma", Verdict::Rare, None),
        ];
        // 仪表盘点击「未用」卡片 → 只保留 unused 档
        let rows = build_rows(
            &results,
            Some(Verdict::Unused),
            None,
            SortKey::Verdict,
            SortDir::Asc,
            None,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "beta");
        // None = 不过滤（点击「总计」卡片取消筛选）
        let rows = build_rows(&results, None, None, SortKey::Verdict, SortDir::Asc, None);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn last_run_sort_puts_never_first_then_oldest() {
        let results = vec![
            result("recent", "recent", Verdict::Indirect, Some(2)),
            result("stale", "stale", Verdict::Indirect, Some(90)),
            result("never", "never", Verdict::Unused, None),
        ];
        let rows = build_rows(&results, None, None, SortKey::LastRun, SortDir::Asc, None);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["never", "stale", "recent"]);
    }

    #[test]
    fn warning_cn_translates_known_warnings_keeping_payloads() {
        // 横幅三大常客（实机 2026-11-20 截图形态）：计数保留、措辞自解释
        assert_eq!(
            warning_cn("[npm] 4 orphan shim set(s) — launchers whose package is gone (see clean script)"),
            "[npm] 检测到 4 组卸载残骸——包已卸载，但启动器文件（X / X.cmd / X.ps1）还留在 PATH 目录，敲命令会报错；clean 脚本附带逐文件删除行"
        );
        assert_eq!(
            warning_cn("[etw] 54 one-shot executions look like an automated probe burst; discounted"),
            "[etw] 54 次单次执行疑似自动化批量探测（agent / 安全工具的冒烟测试），已整簇剔除、不计为使用证据"
        );
        assert_eq!(
            warning_cn("[atime] 28 access-time hits look like scanner sweeps; discounted"),
            "[atime] 28 个访问时间命中疑似安全软件扫盘（同一时段秒级连碰一批文件，非真实使用），已剔除"
        );
        // 带载荷（路径 / 天数 / 错误详情）的消息保留载荷
        assert_eq!(
            warning_cn("[imports] log was quarantined by security software (C:\\d\\events.jsonl.IPGSD); import evidence unavailable — it self-recreates on your next import"),
            "[imports] import 日志被安全软件整文件隔离（C:\\d\\events.jsonl.IPGSD）——import 证据暂不可用；你自己的进程下次 import 时会自动重建"
        );
        assert_eq!(
            warning_cn(
                "[etw] newest event is 8d old — capture may be down, recent usage could be missing"
            ),
            "[etw] 最新 ETW 事件已是 8 天前——采集器可能停了，近几天的使用可能没采到"
        );
        assert_eq!(
            warning_cn("[store] inventory save failed: UNIQUE constraint failed: packages.name"),
            "[store] 包清单保存失败：UNIQUE constraint failed: packages.name"
        );
    }

    #[test]
    fn warning_cn_passes_unknown_through() {
        // 未知 tag / 未识别措辞整条透传（core 措辞演进时 GUI 不显示空白）
        assert_eq!(warning_cn("plain text warning"), "plain text warning");
        assert_eq!(
            warning_cn("[future] something new happened"),
            "[future] something new happened"
        );
        assert_eq!(
            warning_cn("[npm] some future npm wording"),
            "[npm] some future npm wording"
        );
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
        let asc = build_rows(&results, None, None, SortKey::Name, SortDir::Asc, None);
        let desc = build_rows(&results, None, None, SortKey::Name, SortDir::Desc, None);
        let asc_names: Vec<&str> = asc.iter().map(|r| r.name.as_str()).collect();
        let desc_names: Vec<&str> = desc.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(asc_names, vec!["alpha", "beta"]);
        assert_eq!(desc_names, vec!["beta", "alpha"]);
    }

    #[test]
    fn typed_sort_uses_numeric_hits_not_display_text() {
        // 展示文案 "10 次" 的字符串序在 "2 次" 之前；排序必须按数值 2 < 10
        let results = vec![result_hits("ten", 10), result_hits("two", 2)];
        let rows = build_rows(&results, None, None, SortKey::Typed, SortDir::Asc, None);
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
        assert_eq!(detail_cn("imported 3x"), "导入 3 次（import 钩子）");
        assert_eq!(
            detail_cn("binary access time touched"),
            "二进制访问时间近期有更新"
        );
        assert_eq!(
            detail_cn("in shell config: .bashrc:12"),
            "shell 配置引用：.bashrc:12"
        );
        assert_eq!(
            detail_cn("no signal from any source"),
            "窗口内无任何使用记录"
        );
        assert_eq!(
            detail_cn("unused in 4 consecutive snapshots"),
            "连续 4 个月快照均判 unused"
        );
        // 未知格式原样返回（core 未来新增格式时的兜底）
        assert_eq!(detail_cn("some future format"), "some future format");
    }

    #[test]
    fn manager_filter_and_counts() {
        let mut npm_pkg = result("n1", "n1", Verdict::Unused, None);
        npm_pkg.package.manager = Manager::Npm;
        let mut pip_pkg = result("p1", "p1", Verdict::Active, None);
        pip_pkg.package.manager = Manager::Pip;
        let results = vec![result("c1", "c1", Verdict::Rare, None), npm_pkg, pip_pkg];

        // 标签行：只出现过的管理器、按固定顺序（cargo → npm → pip）、带计数
        assert_eq!(
            manager_counts(&results),
            vec![(Manager::Cargo, 1), (Manager::Npm, 1), (Manager::Pip, 1)]
        );
        // 未安装的管理器不出现
        assert!(manager_counts(&results)
            .iter()
            .all(|(m, _)| *m != Manager::Winget));

        // 点击「npm」标签 → 只剩 npm 的包（index 仍是 results 索引）
        let rows = build_rows(
            &results,
            None,
            Some(Manager::Npm),
            SortKey::Name,
            SortDir::Asc,
            None,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "n1");
        assert_eq!(rows[0].manager, "npm");
        assert_eq!(rows[0].index, 1);

        // 管理器筛选与档位筛选叠加
        let rows = build_rows(
            &results,
            Some(Verdict::Active),
            Some(Manager::Pip),
            SortKey::Name,
            SortDir::Asc,
            None,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "p1");
    }

    #[test]
    fn newly_installed_rows_carry_observation_flag() {
        let mut r = result("fresh-tool", "fresh-tool", Verdict::Unused, None);
        r.newly_installed = true;
        let rows = build_rows(&[r], None, None, SortKey::Name, SortDir::Asc, None);
        assert_eq!(rows.len(), 1);
        // 观察期是独立标注字段（证据面板单独渲染），不混进证据链
        assert!(rows[0].observation);
        // 非新装行不带标注
        let rows = build_rows(
            &[result("old-tool", "old-tool", Verdict::Unused, None)],
            None,
            None,
            SortKey::Name,
            SortDir::Asc,
            None,
        );
        assert!(!rows[0].observation);
    }

    #[test]
    fn env_chips_group_and_shorten_labels() {
        let mk = |kind, path| DetectedEnv {
            kind,
            path: std::path::PathBuf::from(path),
        };
        let envs = vec![
            mk(EnvKind::NpmPrefix, r"C:\u\AppData\Roaming\npm"),
            mk(EnvKind::NpmPrefix, r"C:\u\AppData\Roaming\QClaw\npm-global"),
            mk(EnvKind::Python, r"C:\Program Files\Python313\python.exe"),
            mk(EnvKind::Python, r"C:\t\py311\python.exe"),
            mk(
                EnvKind::Python,
                r"C:\u\AppData\Roaming\uv\python\cpython-3.12.13-windows-x86_64-none\python.exe",
            ),
        ];
        let chips = env_chips(
            &envs,
            Some(std::path::Path::new(r"C:\u\AppData\Roaming\npm")),
        );
        // npm：默认 prefix 标「默认」，其余取末级目录名
        assert_eq!(chips.npm[0].label, "默认");
        assert_eq!(chips.npm[1].label, "npm-global");
        // python：普通安装取目录名，uv 托管截出版本号
        let labels: Vec<&str> = chips.python.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["Python313", "py311", "3.12.13"]);
        // 完整路径保留（悬停提示用）
        // full_path 带 path_display 的零宽断点（悬停提示按目录边界折行）
        assert_eq!(
            chips.npm[0].full_path,
            path_display(r"C:\u\AppData\Roaming\npm")
        );
    }

    #[test]
    fn env_chips_default_prefix_match_ignores_case_and_falls_back() {
        let envs = vec![DetectedEnv {
            kind: EnvKind::NpmPrefix,
            path: std::path::PathBuf::from(r"C:\u\AppData\Roaming\npm"),
        }];
        // Windows 路径大小写不敏感
        let chips = env_chips(
            &envs,
            Some(std::path::Path::new(r"c:\u\appdata\roaming\NPM")),
        );
        assert_eq!(chips.npm[0].label, "默认");
        // 默认 prefix 推导失败（None）时回退目录名，不误标
        let chips = env_chips(&envs, None);
        assert_eq!(chips.npm[0].label, "npm");
    }
    #[test]
    fn package_env_label_attributes_pip_and_npm() {
        use std::path::Path;
        let npm_default = Path::new(r"C:\u\AppData\Roaming\npm");
        // pip：系统布局（Lib\site-packages）→ 环境根目录名
        assert_eq!(
            package_env_label(
                Manager::Pip,
                Some(Path::new(r"C:\Program Files\Python313\Lib\site-packages")),
                None
            ),
            Some("Python313".into())
        );
        // pip：用户级 site-packages（一层父目录即环境根）
        assert_eq!(
            package_env_label(
                Manager::Pip,
                Some(Path::new(
                    r"C:\u\AppData\Roaming\Python\Python313\site-packages"
                )),
                None
            ),
            Some("Python313".into())
        );
        // pip：uv 托管解释器 → 截出版本号（与环境行同一名字）
        assert_eq!(
            package_env_label(
                Manager::Pip,
                Some(Path::new(
                    r"C:\u\AppData\Roaming\uv\python\cpython-3.12.13-windows-x86_64-none\Lib\site-packages"
                )),
                None
            ),
            Some("3.12.13".into())
        );
        // npm：默认 prefix →「默认」；scoped 包多一层 @scope 也能归位
        assert_eq!(
            package_env_label(
                Manager::Npm,
                Some(Path::new(
                    r"C:\u\AppData\Roaming\npm\node_modules\@scope\tool"
                )),
                Some(npm_default)
            ),
            Some("默认".into())
        );
        // npm：QClaw 独立 prefix（同名包的另一实例）→ 目录名区分归属
        assert_eq!(
            package_env_label(
                Manager::Npm,
                Some(Path::new(
                    r"C:\u\AppData\Roaming\QClaw\npm-global\node_modules\opencli"
                )),
                Some(npm_default)
            ),
            Some("npm-global".into())
        );
        // 单环境管理器 / 无安装路径 → 无归属标注
        assert_eq!(
            package_env_label(Manager::Cargo, Some(Path::new(r"C:\u\.cargo\bin")), None),
            None
        );
        assert_eq!(package_env_label(Manager::Pip, None, None), None);
    }

    #[test]
    fn summary_counts_observation_period() {
        let mut r = result("fresh", "fresh", Verdict::Unused, None);
        r.newly_installed = true;
        let s = Summary::from_results(&[r, result("old", "old", Verdict::Unused, None)]);
        assert_eq!(s.total, 2);
        assert_eq!(s.unused, 2);
        // 观察期是独立计数：未用档里被豁免出清理候选的数量
        assert_eq!(s.observing, 1);
    }

    #[test]
    fn build_rows_labels_env_for_every_row_single_env_included() {
        use std::path::Path;
        // 每行都标注归属环境（批㉔：单环境也显示那一个），同名多环境不再特殊
        let mut a = result("opencli", "opencli", Verdict::Unused, None);
        a.package.manager = Manager::Npm;
        a.package.install_path = Some(r"C:\u\AppData\Roaming\npm\node_modules\opencli".into());
        let mut b = result("opencli", "opencli", Verdict::Unused, None);
        b.package.manager = Manager::Npm;
        b.package.install_path =
            Some(r"C:\u\AppData\Roaming\QClaw\npm-global\node_modules\opencli".into());
        // 唯一实例（同在默认前缀）→ 照样标注「默认」
        let mut c = result("agently", "agently", Verdict::Active, None);
        c.package.manager = Manager::Npm;
        c.package.install_path = Some(r"C:\u\AppData\Roaming\npm\node_modules\agently".into());
        // cargo 无环境语义 → None
        let mut d = result("ripgrep", "ripgrep", Verdict::Active, None);
        d.package.manager = Manager::Cargo;
        d.package.install_path = Some(r"C:\u\.cargo\bin".into());
        let rows = build_rows(
            &[a, b, c, d],
            None,
            None,
            SortKey::Name,
            SortDir::Asc,
            Some(Path::new(r"C:\u\AppData\Roaming\npm")),
        );
        // 名称升序：agently/opencli/ripgrep；两个 opencli 同名，稳定排序保持输入顺序
        assert_eq!(rows[0].name, "agently");
        assert_eq!(rows[0].env_label.as_deref(), Some("默认"));
        assert_eq!(rows[1].env_label.as_deref(), Some("默认"));
        assert_eq!(rows[2].env_label.as_deref(), Some("npm-global"));
        assert_eq!(rows[3].name, "ripgrep");
        assert_eq!(rows[3].env_label, None);
    }

    #[test]
    fn path_display_puts_breaks_at_directory_boundaries() {
        // 每个反斜杠后跟一个零宽空格：折行落在目录边界，行尾保留 `\` 表意未完
        assert_eq!(
            path_display(r"C:\Users\qi.shao\AppData\Roaming\npm\node_modules\@scope\pkg"),
            "C:\\\u{200b}Users\\\u{200b}qi.shao\\\u{200b}AppData\\\u{200b}Roaming\\\u{200b}npm\\\u{200b}node_modules\\\u{200b}@scope\\\u{200b}pkg"
        );
        // 无反斜杠的文本原样通过
        assert_eq!(path_display("plain"), "plain");
        assert_eq!(path_display(""), "");
    }
}

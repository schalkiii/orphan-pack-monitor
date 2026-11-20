//! import 钩子事件：python `.pth` / node `NODE_OPTIONS --require` 钩子的**消费端**。
//!
//! 与 etw.rs 同构：钩子（由 `orphan-audit hooks install` 部署，运行在各解释器/
//! node 进程内部）把每次顶层 import / node_modules 解析追加为一行 JSONL；
//! 本模块只读不写采集逻辑。与 ETW 互补的准确率优势：
//! - ETW 只见 `python.exe` 启动，无法归因到包；import 钩子天然按模块/包归因
//! - 覆盖 IDE / agent / 项目脚本的一切 import 使用（shell 历史/配置完全看不到）
//! - 每条事件带精确时间戳（PSReadLine 历史无时间戳）
//!
//! 日志格式（每行一条）：
//!   {"kind":"py","module":"fitz","ts":"2026-11-19T…","interp":"C:\\…\\python.exe"}
//!   {"kind":"js","module":"firecrawl-cli","ts":"…","exe":"node.exe"}
//! node 侧 module 已是 node_modules 首段包名（scoped 保留 @scope/pkg 形态）。

use crate::model::{Package, SignalSource, UsageEvent};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 默认 import 事件日志：数据目录下 imports/events.jsonl（与 etw/ 平级）。
/// OA_IMPORT_LOG 可整体重定向（钩子写侧与 deep 模式同环境变量，测试隔离 /
/// 避开被管控目录）；读写两侧共用本函数，覆盖自然一致。
pub fn default_import_log_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OA_IMPORT_LOG") {
        if !p.trim().is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    dirs::data_dir().map(|d| d.join("orphan-audit").join("imports").join("events.jsonl"))
}

/// 钩子宿主目录：hooks install 把钩子文件部署到这里，.pth / NODE_OPTIONS 引用它。
pub fn default_hook_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("orphan-audit").join("hooks"))
}

/// 钩子是否已安装（hooks install 部署的标记文件在钩子目录里；uninstall 会摘除）。
pub fn hooks_installed() -> bool {
    default_hook_dir().is_some_and(|d| d.join("oa_node_hook.cjs").is_file())
}

/// import 日志缺失时的降级警告；None = 正常态（日志在，或压根没装钩子）。
///
/// 实机取证 2026-11-20：安全代理把整个 events.jsonl 隔离成 `.IPGSD` 副本后，
/// `read_import_events_since` 对缺失文件返回空且零提示——import 证据静默归零，
/// 靠它撑住 indirect 的包会无声滑向 unused（误判可删方向）。ETW 侧的
/// 「降级必须显式」原则在此对称补齐。钩子写侧是 append 模式，用户自己
/// 进程的下次 import 会自动重建日志，无需工具侧干预。
/// 安全代理隔离日志文件时的影子路径（events.jsonl → events.jsonl.IPGSD）。
/// 只拼路径查存在性，**绝不读内容**（TSD 红线）。审计警告与 `hooks status`
/// 诊断（CLI）共用。
pub fn shadow_path(log: &Path) -> PathBuf {
    let mut s = log.as_os_str().to_os_string();
    s.push(".IPGSD");
    PathBuf::from(s)
}

pub fn import_log_warning(log: &Path, hooks_installed: bool) -> Option<String> {
    if log.exists() || !hooks_installed {
        return None;
    }
    let shadow = shadow_path(log);
    if shadow.is_file() {
        Some(format!(
            "[imports] log was quarantined by security software ({}); import evidence \
             unavailable — it self-recreates on your next import",
            shadow.display()
        ))
    } else {
        Some(
            "[imports] log missing but hooks are installed — no import events recorded \
             yet (or the log was moved)"
                .into(),
        )
    }
}

/// 从落盘日志读取 import 事件（带 since 窗口过滤；None = 不过滤）。
/// 缺 module / ts 的行视为损坏数据跳过（钩子写侧保证原子单行追加）。
pub fn read_import_events_since(
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
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let (Some(module), Some(ts_str)) = (
            v.get("module").and_then(|x| x.as_str()),
            v.get("ts").and_then(|x| x.as_str()),
        ) else {
            continue;
        };
        let Ok(ts) = DateTime::parse_from_rfc3339(ts_str) else {
            continue;
        };
        let ts = ts.with_timezone(&Utc);
        if since.is_some_and(|s| ts < s) {
            continue;
        }
        events.push(UsageEvent::new(module, ts, SignalSource::Import));
    }
    Ok(events)
}

/// 测试与手工探针用：追加一条 import 事件。
pub fn append_import_event(path: &Path, module: &str, kind: &str) -> anyhow::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let rec = serde_json::json!({
        "kind": kind,
        "module": module,
        "ts": Utc::now().to_rfc3339(),
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{rec}")?;
    Ok(())
}

/// 构建 顶层模块名 -> 包名 映射（仅 pip 系：npm 的 import 事件本身就是包名）。
/// 数据源是 site-packages 里各 `*.dist-info/top_level.txt`（pip 标准产物，
/// 纯文件读取，不 spawn python——TSD 树内 python stdout 不可靠的教训同 npm）。
/// 同名模块命中多个包时（极少见）保留先写入者，调用方不感知歧义。
pub fn module_to_package_map(packages: &[Package]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for p in packages {
        // pip 的 install_path 就是 site-packages；uv/pipx 是工具 venv 根
        // （dist-info 在其下的 Lib\site-packages）——布局随管理器走，
        // 但 top_level.txt 的扫描逻辑完全一致，映射对三类管理器都成立
        let roots: Vec<Option<PathBuf>> = match p.manager {
            crate::model::Manager::Pip => vec![p.install_path.clone()],
            crate::model::Manager::Uv | crate::model::Manager::Pipx => {
                vec![p
                    .install_path
                    .as_ref()
                    .map(|r| r.join("Lib").join("site-packages"))]
            }
            _ => vec![None],
        };
        for root in roots.into_iter().flatten() {
            for module in top_level_modules(&root) {
                map.entry(module.to_ascii_lowercase())
                    .or_insert_with(|| p.name.clone());
            }
        }
    }
    map
}

/// 枚举一个 site-packages 目录下所有 dist-info 的 top_level.txt 模块名。
fn top_level_modules(site_packages: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(site_packages) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.ends_with(".dist-info") || name.ends_with(".egg-info")) {
            continue;
        }
        let tl = e.path().join("top_level.txt");
        let Ok(raw) = std::fs::read_to_string(&tl) else {
            continue;
        };
        for line in raw.lines() {
            let m = line.trim();
            if !m.is_empty() {
                out.push(m.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 把带命令行的事件按命令行里的包路径/`-m 模块` 归因成包级事件。
/// ETW 的 binary 只是进程镜像名（node.exe/python.exe），对解释器类进程
/// 必须靠 command_line 里的路径指向（npm bin 脚本、site-packages 内脚本）
/// 或 `-m <模块>`（经 top_level 映射）才能落到具体包。
/// 管理动作天然排除：`pip uninstall -y numpy` / `npm uninstall -g pkg`
/// 不含任何包 install_path，不会归因出事件。
pub fn attribute_cmdline(
    events: &[UsageEvent],
    packages: &[Package],
    module_map: &HashMap<String, String>,
) -> Vec<UsageEvent> {
    let mut out = Vec::new();
    for ev in events {
        let Some(cmd) = &ev.command_line else {
            continue;
        };
        let cmd_lower = cmd.to_ascii_lowercase();
        // 1) 命令行包含某包 install_path（大小写不敏感 + **边界**匹配——
        //    命令行引号/转义形态多变，路径子串是最稳健的锚点，但必须落在
        //    目录边界上：`node_modules\lodash` 是 `node_modules\lodash-cli`
        //    的子串，无边界检查会把相邻包的执行误归因过来。
        //    pip 除外：其 install_path 是同解释器全部 pip 包共享的
        //    site-packages 根，命中无法区分是哪个包——路径归因整体跳过，
        //    pip 的命令行归因由下方 `-m 模块` → top_level 映射承担）
        for p in packages {
            if p.manager == crate::model::Manager::Pip {
                continue;
            }
            let Some(path) = &p.install_path else {
                continue;
            };
            let path_str = path.to_string_lossy().to_ascii_lowercase();
            if path_str.len() > 4 && contains_at_boundary(&cmd_lower, &path_str) {
                let mut hit = UsageEvent::new(p.name.clone(), ev.timestamp, SignalSource::Import);
                hit.command_line = Some(cmd.clone());
                out.push(hit);
            }
        }
        // 2) `python -m <模块>`：模块名经 top_level 映射归到包
        if let Some(module) = dash_m_module(cmd) {
            if let Some(pkg) = module_map.get(&module.to_ascii_lowercase()) {
                let mut hit = UsageEvent::new(pkg.clone(), ev.timestamp, SignalSource::Import);
                hit.command_line = Some(cmd.clone());
                out.push(hit);
            }
        }
    }
    out
}

/// 大小写不敏感的**边界**子串匹配：needle 在 haystack 中出现，且出现段的
/// 末尾要么到串尾、要么紧跟目录分隔符 `\`（正斜杠形态的命令行本就不含
/// 反斜杠 install_path，不会产生假边界）。
fn contains_at_boundary(haystack: &str, needle: &str) -> bool {
    let mut from = 0usize;
    while let Some(at) = haystack[from..].find(needle) {
        let end = from + at + needle.len();
        if end == haystack.len() || haystack.as_bytes()[end] == b'\\' {
            return true;
        }
        from += at + 1;
    }
    false
}

/// 提取 `-m <模块>` 的模块名（只在 python 风格命令行里出现；取 -m 后首个 token）。
fn dash_m_module(cmd: &str) -> Option<String> {
    let mut words = cmd.split_whitespace();
    while let Some(w) = words.next() {
        if w == "-m" {
            let m = words.next()?;
            let top = m.split('.').next().unwrap_or(m);
            if top.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !top.is_empty() {
                return Some(top.to_string());
            }
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Manager;
    use chrono::TimeZone;

    #[test]
    fn shadow_path_appends_ipgsd_suffix() {
        // 影子 = 原路径 + ".IPGSD"（hooks status 与审计警告共用同一拼法）
        assert_eq!(
            shadow_path(Path::new(r"C:\d\imports\events.jsonl")),
            PathBuf::from(r"C:\d\imports\events.jsonl.IPGSD")
        );
    }

    #[test]
    fn import_log_warning_distinguishes_quarantine_from_fresh() {
        let dir = std::env::temp_dir().join(format!("oa_impw_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("events.jsonl");
        // 日志在 → 无警告（正常采集态）
        std::fs::write(&log, "{}\n").unwrap();
        assert!(import_log_warning(&log, true).is_none());
        // 日志缺失 + 未装钩子 → 无警告（没装钩子就没有 import 信号）
        assert!(import_log_warning(&log, false).is_none());
        // 日志缺失 + 钩子在装 + .IPGSD 影子 → 点名隔离成因（实机 2026-11-20 形态）
        std::fs::remove_file(&log).unwrap();
        std::fs::write(dir.join("events.jsonl.IPGSD"), "x").unwrap();
        let w = import_log_warning(&log, true).unwrap();
        assert!(w.contains("quarantined by security software"), "{w}");
        assert!(w.contains("self-recreates"), "{w}");
        // 日志缺失 + 钩子在装 + 无影子 → 尚无事件（新装钩子的正常早期态）
        std::fs::remove_file(dir.join("events.jsonl.IPGSD")).unwrap();
        let w = import_log_warning(&log, true).unwrap();
        assert!(w.contains("no import events recorded yet"), "{w}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_jsonl_roundtrip_and_window() {
        let dir = std::env::temp_dir().join(format!("oa_imp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("events.jsonl");
        std::fs::create_dir_all(&dir).unwrap();
        // 损坏行与缺字段行先落盘，再追加有效事件（fs::write 会截断，顺序不能反）
        std::fs::write(
            &p,
            "not json\n{\"kind\":\"py\"}\n{\"kind\":\"py\",\"module\":\"x\",\"ts\":\"bad\"}\n",
        )
        .unwrap();
        append_import_event(&p, "fitz", "py").unwrap();
        append_import_event(&p, "firecrawl-cli", "js").unwrap();
        append_import_event(&p, "mcp", "py").unwrap();
        let evs = read_import_events_since(&p, None).unwrap();
        assert_eq!(evs.len(), 3, "3 valid events survive corrupt lines");
        assert!(evs.iter().all(|e| e.source == SignalSource::Import));
        assert!(evs.iter().any(|e| e.binary == "fitz"));
        // 窗口过滤：早于窗口的行被剔除
        let since = Utc::now() - chrono::Duration::hours(1);
        assert_eq!(read_import_events_since(&p, Some(since)).unwrap().len(), 3);
        let old = Utc::now() + chrono::Duration::days(4000); // 未来窗口 → 全剔除
        assert!(read_import_events_since(&p, Some(old)).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_log_is_empty_not_error() {
        let p = std::env::temp_dir().join(format!("oa_imp_missing_{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        assert!(read_import_events_since(&p, None).unwrap().is_empty());
    }

    #[test]
    fn import_log_path_honors_env_override() {
        // 环境变量优先（与钩子写侧同一变量）；空白值视为未设置
        std::env::set_var("OA_IMPORT_LOG", r"C:\oa-test\events.jsonl");
        assert_eq!(
            default_import_log_path(),
            Some(PathBuf::from(r"C:\oa-test\events.jsonl"))
        );
        std::env::set_var("OA_IMPORT_LOG", "   ");
        assert_ne!(
            default_import_log_path(),
            Some(PathBuf::from("   ")),
            "空白值回退默认路径"
        );
        std::env::remove_var("OA_IMPORT_LOG");
    }

    #[test]
    fn module_map_reads_top_level_txt() {
        let dir = std::env::temp_dir().join(format!("oa_imp_map_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let site = dir.join("site-packages");
        let di = site.join("pymupdf-1.24.0.dist-info");
        std::fs::create_dir_all(&di).unwrap();
        std::fs::write(di.join("top_level.txt"), "fitz\npymupdf\n").unwrap();
        let mut p = Package::new("PyMuPDF", Manager::Pip);
        p.install_path = Some(site.clone());
        let map = module_to_package_map(&[p]);
        // 契约：键为模块名小写（查询方 attribute_cmdline 负责 lowercase）
        assert_eq!(map.get("fitz").map(String::as_str), Some("PyMuPDF"));
        assert_eq!(map.get("pymupdf").map(String::as_str), Some("PyMuPDF"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cmdline_attribution_by_path_and_dash_m() {
        let now = Utc.with_ymd_and_hms(2026, 11, 19, 0, 0, 0).unwrap();
        let mut tracker = Package::new("tokentracker-cli", Manager::Npm);
        tracker.install_path = Some(PathBuf::from(r"C:\npm\node_modules\tokentracker-cli"));
        let mut pymupdf = Package::new("PyMuPDF", Manager::Pip);
        pymupdf.install_path = Some(PathBuf::from(r"C:\py\Lib\site-packages"));
        let pkgs = vec![tracker, pymupdf];
        let mut map = HashMap::new();
        map.insert("fitz".to_string(), "PyMuPDF".to_string());

        let mk = |cmd: &str| {
            let mut ev = UsageEvent::new("node.exe", now, SignalSource::Etw);
            ev.command_line = Some(cmd.to_string());
            ev
        };
        let events = vec![
            mk(r#"node C:\npm\node_modules\tokentracker-cli\bin\tracker.js"#),
            {
                let mut e = UsageEvent::new("python.exe", now, SignalSource::Etw);
                e.command_line = Some("python -m fitz tools".into());
                e
            },
            mk(r#"pip uninstall -y numpy"#), // 管理动作：无路径 → 不归因
            mk(r#"rg pattern"#),             // 无命令行的事件不参与
            UsageEvent::new("rg", now, SignalSource::Etw),
        ];
        let hits = attribute_cmdline(&events, &pkgs, &map);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|e| e.binary == "tokentracker-cli"));
        assert!(hits.iter().any(|e| e.binary == "PyMuPDF"));
    }

    #[test]
    fn cmdline_attribution_requires_path_boundary() {
        // 前缀相邻包：`node_modules\lodash` 不得吃掉 `node_modules\lodash-cli` 的执行
        let now = Utc.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap();
        let mut lo = Package::new("lodash", Manager::Npm);
        lo.install_path = Some(PathBuf::from(r"C:\npm\node_modules\lodash"));
        let mut lo_cli = Package::new("lodash-cli", Manager::Npm);
        lo_cli.install_path = Some(PathBuf::from(r"C:\npm\node_modules\lodash-cli"));
        let mut ev = UsageEvent::new("node.exe", now, SignalSource::Etw);
        ev.command_line = Some(r"node C:\npm\node_modules\lodash-cli\bin\cli.js".into());
        let hits = attribute_cmdline(&[ev], &[lo.clone(), lo_cli.clone()], &HashMap::new());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].binary, "lodash-cli");
        // 精确路径（边界在串尾）仍命中
        let mut ev = UsageEvent::new("node.exe", now, SignalSource::Etw);
        ev.command_line = Some(r"node C:\npm\node_modules\lodash".into());
        let hits = attribute_cmdline(&[ev], &[lo, lo_cli], &HashMap::new());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].binary, "lodash");
    }

    #[test]
    fn pip_shared_site_packages_root_is_not_attribution_path() {
        // pip 的 install_path 是共享 site-packages 根：含该路径的命令行
        // 不得归因到任何 pip 包（否则一条命令行打中同解释器的全部 pip 包）
        let now = Utc.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap();
        let site = PathBuf::from(r"C:\py\Lib\site-packages");
        let mut a = Package::new("pkg-a", Manager::Pip);
        a.install_path = Some(site.clone());
        let mut b = Package::new("pkg-b", Manager::Pip);
        b.install_path = Some(site);
        // npm 包不受影响：其 install_path 包级唯一，边界命中照常归因
        let mut n = Package::new("tool", Manager::Npm);
        n.install_path = Some(PathBuf::from(r"C:\npm\node_modules\tool"));
        let mut ev = UsageEvent::new("python.exe", now, SignalSource::Etw);
        ev.command_line = Some(
            r"python C:\py\Lib\site-packages\pkg-a\__main__.py C:\npm\node_modules\tool\x.js"
                .into(),
        );
        let hits = attribute_cmdline(&[ev], &[a, b, n], &HashMap::new());
        assert_eq!(hits.len(), 1, "只归因 npm 包");
        assert_eq!(hits[0].binary, "tool");
    }

    #[test]
    fn dash_m_extracts_top_level_module() {
        assert_eq!(
            dash_m_module("python -m http.server 8000"),
            Some("http".into())
        );
        assert_eq!(dash_m_module("py -3.13 -m fitz"), Some("fitz".into()));
        assert_eq!(dash_m_module("node index.js"), None);
        assert_eq!(dash_m_module("python -m"), None);
    }
}

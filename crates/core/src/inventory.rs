//! 清单采集层：枚举各包管理器安装的全局包。
//!
//! 每个采集器遵循同一原则：
//! 1. 缺失的包管理器不报错，只是贡献空列表（与 shelfware 一致）。
//! 2. 一律走各管理器的公开接口（命令行 JSON / 元数据文件），不猜测、不 scraping。
//! 3. 包->二进制映射尽量精确：cargo 读 .crates2.json，npm 读 package.json 的 bin 字段。

use crate::model::{basename, Confidence, Manager, Package};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
/// Windows 子进程隐藏控制台窗口标志；run_command 与 etw/signals 的探测命令共用。
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 统一执行外部命令并返回 stdout。Windows 下隐藏控制台窗口。
pub fn run_command(mut cmd: Command) -> Result<String> {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd
        .output()
        .with_context(|| format!("failed to spawn {:?}", cmd.get_program()))?;
    if !out.status.success() {
        anyhow::bail!(
            "command {:?} exited with {}:\n{}",
            cmd.get_program(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 构造外部工具命令。Windows 上 npm 等是 .cmd 脚本，而 CreateProcess 只能直接
/// 启动 .exe——PATH 上找不到 `<name>.exe` 时回退 `cmd /d /c`（由 cmd 按
/// PATHEXT 解析 npm.cmd；实测 2026-11-19：nodejs 官方安装只有 npm / npm.cmd，
/// 无 npm.exe，此前 collect_npm 因探测失败而整体静默为空）。
/// 含引号/空格参数的调用（python -c）只会出现在 .exe 直启路径上
/// （无 .exe 时 tool_available 已先返回 false），不受 cmd 转义影响。
pub fn command_for(program: &str) -> Command {
    #[cfg(windows)]
    if !exe_on_path(program) {
        let mut c = Command::new("cmd");
        c.arg("/d").arg("/c").arg(program);
        return c;
    }
    Command::new(program)
}

/// `<program>.exe` 是否在 PATH 上（Windows）。.cmd/.bat 脚本无法被
/// CreateProcess 直接启动，不算在内。与 first_exe_on_path 同一探测
/// （后者返回完整路径，此处只需存在性）。
#[cfg(windows)]
fn exe_on_path(program: &str) -> bool {
    first_exe_on_path(program).is_some()
}

/// 探测命令是否可用。
pub fn tool_available(name: &str) -> bool {
    let mut cmd = command_for(name);
    cmd.arg("--version");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Cargo
// ---------------------------------------------------------------------------

/// cargo 的实际 HOME。优先 CARGO_HOME 环境变量，否则 ~/.cargo。
/// 注意：Windows 上 scoop/rustup 安装的 CARGO_HOME 可能不在用户目录，必须读环境变量。
pub fn cargo_home() -> PathBuf {
    if let Ok(h) = std::env::var("CARGO_HOME") {
        if !h.trim().is_empty() {
            return PathBuf::from(h);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cargo")
}

/// 解析 .crates2.json 文本（纯函数，便于单测）。
/// 结构：{ "installs": { "crate-name version (source)": { "version_req", "bins": [...], ... } } }
pub fn parse_crates2(raw: &str, cargo_bin_dir: PathBuf) -> Result<Vec<Package>> {
    let json: serde_json::Value =
        serde_json::from_str(raw).with_context(|| "parsing .crates2.json")?;
    let mut packages = Vec::new();
    if let Some(installs) = json.get("installs").and_then(|v| v.as_object()) {
        for (key, val) in installs {
            // key 形如 "dusty 0.1.0 (registry+...)"，包名是第一个空格前的部分
            let name = key.split_whitespace().next().unwrap_or(key).to_string();
            // 实际字段：bins（不是 binaries）；版本在 version_req，形如 "=1.27.0" 或 null。
            // 兼容旧版 cargo 的 binaries/version 字段。
            let version = val
                .get("version_req")
                .and_then(|v| v.as_str())
                .or_else(|| val.get("version").and_then(|v| v.as_str()))
                .map(|s| s.trim_start_matches('=').to_string());
            let binaries: Vec<String> = val
                .get("bins")
                .or_else(|| val.get("binaries"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str())
                        .map(|s| strip_exe(s).to_string())
                        .collect()
                })
                .unwrap_or_default();
            let mut p = Package::new(name, Manager::Cargo);
            p.version = version;
            p.binaries = binaries;
            p.install_path = Some(cargo_bin_dir.clone());
            p.confidence = Confidence::Exact;
            packages.push(p);
        }
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packages)
}

/// 采集 cargo 全局安装（读 CARGO_HOME/.crates2.json）。
pub fn collect_cargo() -> Result<Vec<Package>> {
    let home = cargo_home();
    let meta = home.join(".crates2.json");
    if !meta.exists() {
        return Ok(Vec::new());
    }
    let raw =
        std::fs::read_to_string(&meta).with_context(|| format!("reading {}", meta.display()))?;
    parse_crates2(&raw, home.join("bin"))
}

// ---------------------------------------------------------------------------
// npm (global)
// ---------------------------------------------------------------------------

/// npm 默认全局 prefix 解析（不运行 npm）。优先级与 npm 一致：
/// `npm_config_prefix` 环境变量 > 用户 .npmrc 的 prefix > Windows 默认 %APPDATA%\npm。
///
/// 不运行 `npm root -g` 的原因（2026-11-19 实测）：nodejs 官方安装的 npm 是
/// .cmd 脚本，CreateProcess 无法直接启动；即便经 `cmd /d /c` 转发成功，本机
/// 安全代理（TSD）也会吞掉非信任进程树下 node 的全部 stdout——连
/// `node -e "console.log(1)"` 都返回 exit 0 + 空输出（python/cmd 不受影响）。
/// 纯读文件免疫该问题，也省去子进程开销。
pub fn npm_default_prefix() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("npm_config_prefix") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    if let Some(home) = dirs::home_dir() {
        if let Ok(rc) = std::fs::read_to_string(home.join(".npmrc")) {
            if let Some(p) = npmrc_prefix(&rc) {
                return Some(p);
            }
        }
    }
    // npmconf 的 Windows 内置默认：prefix = %APPDATA%\npm
    dirs::data_dir().map(|d| d.join("npm"))
}

/// 枚举本机**全部** npm 全局 prefix（多 node 环境检测）：
/// 1. 默认 prefix（npm_default_prefix）
/// 2. 版本管理器：nvm-windows（%APPDATA%\nvm\v*，各版本自带 node_modules）、
///    fnm（%LOCALAPPDATA%\fnm\node-versions\*\installation）
/// 3. 标记扫描：%APPDATA% / %LOCALAPPDATA% 下两层目录内符合 npm prefix 指纹的
///    目录（判定见 is_npm_global_prefix）—— 2026-11-19 实测捕获 QClaw 的独立
///    prefix `%APPDATA%\QClaw\npm-global`（无锁文件，靠 .cmd/.ps1 shim 对识别）
pub fn npm_global_prefixes() -> Vec<PathBuf> {
    let mut prefixes: Vec<PathBuf> = Vec::new();
    let push = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if is_npm_global_prefix(&p) && !out.contains(&p) {
            out.push(p);
        }
    };
    if let Some(p) = npm_default_prefix() {
        push(p, &mut prefixes);
    }
    // nvm-windows：版本目录本身即 prefix
    if let Some(nvm) = dirs::data_dir().map(|d| d.join("nvm")) {
        if let Ok(entries) = std::fs::read_dir(&nvm) {
            for e in entries.flatten() {
                push(e.path(), &mut prefixes);
            }
        }
    }
    // fnm：node-versions\<ver>\installation 为 prefix（比标记扫描深一层，需显式枚举）
    if let Some(fnm) = dirs::cache_dir().map(|d| d.join("fnm").join("node-versions")) {
        if let Ok(versions) = std::fs::read_dir(&fnm) {
            for v in versions.flatten() {
                push(v.path().join("installation"), &mut prefixes);
            }
        }
    }
    // 标记扫描（默认 prefix / nvm 已在上面，重复会被去重）
    for root in [dirs::data_dir(), dirs::cache_dir()].into_iter().flatten() {
        scan_npm_prefix_mark(&root, 0, &mut prefixes);
    }
    prefixes
}

/// 在 `dir` 下找 npm 全局 prefix 标记目录（`<dir>\node_modules\.package-lock.json`），
/// 最多下探 `depth` 层。首层跳过已知大而无关的系统目录（Packages/Temp 等数百子目录）。
/// npm 全局 prefix 判定（多 prefix 检测的复合标记，任一命中即认定）：
/// - `node_modules\.package-lock.json` 存在（npm v7+ 全局安装写锁文件）；或
/// - node_modules 旁有**同名成对的 .cmd/.ps1 命令 shim**——npm 在 Windows
///   prefix 根为每个全局命令同时生成两者（2026-11-19 实测 QClaw 的独立
///   prefix `%APPDATA%\QClaw\npm-global` 无锁文件，但有 agently-cli.cmd/.ps1）。
///
/// 前置排除：根上有 package.json 的是 node **项目**目录（prefix 根没有），
/// 其 node_modules 是项目依赖，不能当全局包审计。
fn is_npm_global_prefix(p: &Path) -> bool {
    if !p.join("node_modules").is_dir() || p.join("package.json").is_file() {
        return false;
    }
    if p.join("node_modules").join(".package-lock.json").is_file() {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(p) else {
        return false;
    };
    entries.flatten().any(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        name.strip_suffix(".cmd")
            .is_some_and(|stem| p.join(format!("{stem}.ps1")).is_file())
    })
}

fn scan_npm_prefix_mark(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    const SKIP: [&str; 8] = [
        "temp",
        "packages",
        "microsoft",
        "d3dscache",
        "comms",
        "squirreltemp",
        "crashdumps",
        "notification",
    ];
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if is_npm_global_prefix(&p) && !out.contains(&p) {
            out.push(p.clone());
        }
        let Some(name) = p.file_name().map(|n| n.to_string_lossy().to_lowercase()) else {
            continue;
        };
        if depth == 0 && !SKIP.contains(&name.as_str()) && !name.starts_with('.') {
            scan_npm_prefix_mark(&p, 1, out);
        }
    }
}

/// 从 .npmrc 文本取 prefix（ini 格式：`prefix = 路径`，可带引号；# / ; 为注释）。
fn npmrc_prefix(rc: &str) -> Option<PathBuf> {
    for line in rc.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == "prefix" {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                if !v.is_empty() {
                    return Some(PathBuf::from(v));
                }
            }
        }
    }
    None
}

/// 解析 npm package.json 的 bin 字段（string 或 object 两种形态）。
/// 字符串形态 `{"bin": "bin/x.js"}` 时 npm 创建的可执行名是**包名**
/// （scoped 包取末段），不是脚本文件名——与 npm 实际行为一致。
fn npm_bin_names(pkg_json: &Path, package_name: &str) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(pkg_json) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(bin) = v.get("bin") else {
        return Vec::new();
    };
    match bin {
        serde_json::Value::String(_) => vec![strip_exe(basename(package_name)).to_string()],
        serde_json::Value::Object(map) => map
            .keys()
            .map(|k| strip_exe(basename(k)).to_string())
            .collect(),
        _ => Vec::new(),
    }
}

/// 采集 npm 全局包：枚举**每个**全局 prefix 的 `node_modules`（含 @scope 子目录），
/// 读各包自己的 package.json——与 `npm ls -g --json` 的数据源相同，但不依赖
/// node 子进程（见 npm_default_prefix）。多 prefix 见 npm_global_prefixes。
pub fn collect_npm() -> Result<Vec<Package>> {
    let mut packages = Vec::new();
    for prefix in npm_global_prefixes() {
        // npm_global_prefixes 只返回带 node_modules 的目录，无需再判存在
        packages.extend(npm_packages_under(&prefix.join("node_modules")));
    }
    Ok(packages)
}

/// 枚举 node_modules 目录（普通包 + @scope/<pkg>；跳过 .bin 等点开头条目——
/// 顶层与 scope 子层都跳：npm 从不把点前缀目录当包装，这类目录只可能是
/// 基础设施（.pnpm）或安全代理的隔离暂存（实测 TSD 把已卸载的包改名成
/// `.opencli-XXXX` 留在 scope 目录下，不跳过会让已删除的包「死而复生」）。
/// package.json 缺失/损坏的目录跳过（npm ls 会标记 invalid，我们直接不列）。
fn npm_packages_under(modules: &Path) -> Vec<Package> {
    let mut packages = Vec::new();
    let Ok(entries) = std::fs::read_dir(modules) else {
        return packages;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(file_name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if file_name.starts_with('.') {
            continue; // .bin / .cache 等基础设施目录
        }
        if file_name.starts_with('@') {
            // scope 目录本身不是包，其下每个子目录才是
            if let Ok(subs) = std::fs::read_dir(&path) {
                for sub in subs.flatten() {
                    let sub_path = sub.path();
                    let Some(sub_name) = sub_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                    else {
                        continue;
                    };
                    if sub_name.starts_with('.') {
                        continue; // 同顶层：隔离暂存 / 基础设施目录不是包
                    }
                    let pkg_name = format!("{file_name}/{sub_name}");
                    if let Some(p) = npm_pkg_from_dir(&sub_path)
                        .or_else(|| npm_pkg_fallback(&sub_path, &pkg_name))
                    {
                        packages.push(p);
                    }
                }
            }
        } else if let Some(p) =
            npm_pkg_from_dir(&path).or_else(|| npm_pkg_fallback(&path, &file_name))
        {
            packages.push(p);
        }
    }
    packages
}

/// 从 node_modules 下的包目录组装 Package（package.json 缺 name / 解析失败返回 None）。
fn npm_pkg_from_dir(dir: &Path) -> Option<Package> {
    let raw = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let name = v.get("name")?.as_str()?.to_string();
    let mut p = Package::new(name.clone(), Manager::Npm);
    p.version = v
        .get("version")
        .and_then(|x| x.as_str())
        .map(str::to_string);
    // 精确映射：bin 字段（string 形态的可执行名是包名，见 npm_bin_names）
    p.binaries = npm_bin_names(&dir.join("package.json"), &name);
    p.install_path = Some(dir.to_path_buf());
    p.confidence = if p.binaries.is_empty() {
        // 没有 bin 字段的全局包通常是纯库，不提供 CLI -> 不纳入路线 B 的匹配
        Confidence::None
    } else {
        Confidence::Exact
    };
    Some(p)
}

/// package.json 不可读时的兜底：包目录名作包名，bin/ 子目录的脚本文件名作
/// 命令名。npm 包 bin 字段的 key 通常与脚本同名但不保证——置信度降为
/// Inferred（GUI / 报告标注「推断」提示复核）。
/// 典型场景：终端安全软件（TSD）把 agent 安装的包文件加密重命名为 *.IPGSD
/// （目录名与文件名保留、内容不可读），package.json 因此读不到（2026-11-19 实测）。
fn npm_pkg_fallback(dir: &Path, name: &str) -> Option<Package> {
    let mut p = Package::new(name.to_string(), Manager::Npm);
    p.binaries = npm_bins_from_dir(&dir.join("bin"));
    p.install_path = Some(dir.to_path_buf());
    p.confidence = if p.binaries.is_empty() {
        Confidence::None
    } else {
        Confidence::Inferred
    };
    Some(p)
}

/// bin/ 子目录的脚本文件名 → 命令名候选。只收 .js/.mjs/.cjs/.ts 并去后缀；
/// 其他扩展名（*.IPGSD 加密残留、README 等）一律不收。
fn npm_bins_from_dir(bin_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(bin_dir) else {
        return Vec::new();
    };
    let mut bins: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name
                .strip_suffix(".js")
                .or_else(|| name.strip_suffix(".mjs"))
                .or_else(|| name.strip_suffix(".cjs"))
                .or_else(|| name.strip_suffix(".ts"))?;
            if stem.ends_with(".d") {
                return None; // .d.ts 类型声明不是可执行入口
            }
            Some(stem.to_string())
        })
        .collect();
    bins.sort();
    bins.dedup();
    bins
}

// ---------------------------------------------------------------------------
// npm 卸载残骸检测
// ---------------------------------------------------------------------------

/// 检测全部 npm prefix 根上的**孤儿启动器**（npm uninstall 半途失败留下的
/// 死文件，实机取证见 R10 报告：包目录已删、启动器三件套仍在，敲命令撞死
/// 启动器报错）。检测语义见 `npm_orphan_shims_in`。
pub fn npm_orphan_shims() -> Vec<crate::model::OrphanShim> {
    let mut out = Vec::new();
    for prefix in npm_global_prefixes() {
        out.extend(npm_orphan_shims_in(&prefix));
    }
    out
}

/// 单个 prefix 的孤儿启动器检测。判定规则（保守，宁可漏报不可误报）：
/// - stem 候选 = `X.cmd` 存在，或 `X.ps1` 与无扩展名伴生 `X` 同时存在
///   （npm 三件套必含 sh 形态；孤立 .ps1 可能是用户自己的脚本，不列）；
/// - node 自管核心命令 npm/npx/corepack 排除（nvm/fnm 版本 prefix 根有之，
///   其包目录形态随版本管理器而变，不值得逐布局适配）；
/// - `.bat` 一律不碰——npm 从不生成 .bat（QClaw 的 pip.bat/python.bat 是
///   用户自建转发脚本）；
/// - 孤儿 = 本 prefix 包集合无人 `owns_binary(stem)` 且 `node_modules/<stem>`
///   目录不存在（后者兜底 TSD 加密 fallback 包的归属：fallback 包名取目录名）。
///
/// 残骸文件清单含 `.IPGSD` 隔离副本，但**只拼路径查存在性，绝不读内容**
/// （TSD 加密红线）。
pub fn npm_orphan_shims_in(prefix: &Path) -> Vec<crate::model::OrphanShim> {
    // stem 收集（大小写不敏感去重；目录即使叫 *.cmd 也不是启动器）
    let mut stems: Vec<String> = Vec::new();
    let push_stem = |stem: &str, stems: &mut Vec<String>| {
        if !stems.iter().any(|s| s.eq_ignore_ascii_case(stem)) {
            stems.push(stem.to_string());
        }
    };
    let Ok(entries) = std::fs::read_dir(prefix) else {
        return Vec::new();
    };
    for e in entries.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".cmd") {
            push_stem(stem, &mut stems);
        } else if let Some(stem) = name.strip_suffix(".ps1") {
            // npm 三件套必含无扩展名 sh 脚本；无伴生的孤立 .ps1 不算
            if prefix.join(stem).is_file() {
                push_stem(stem, &mut stems);
            }
        }
    }
    stems.sort();

    let packages = npm_packages_under(&prefix.join("node_modules"));
    let mut out = Vec::new();
    for stem in stems {
        let lower = stem.to_ascii_lowercase();
        if matches!(lower.as_str(), "npm" | "npx" | "corepack") {
            continue; // node 自管核心命令
        }
        let owned = packages.iter().any(|p| p.owns_binary(&stem))
            || prefix.join("node_modules").join(&stem).is_dir();
        if owned {
            continue;
        }
        let mut files = Vec::new();
        for suffix in ["", ".cmd", ".ps1", ".IPGSD", ".cmd.IPGSD", ".ps1.IPGSD"] {
            let p = prefix.join(format!("{stem}{suffix}"));
            if p.is_file() {
                files.push(p);
            }
        }
        if !files.is_empty() {
            out.push(crate::model::OrphanShim {
                prefix: prefix.to_path_buf(),
                bin: stem,
                files,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// pip (CLI tools only) / uv tool / pipx
// ---------------------------------------------------------------------------

/// 采集 pip 全局包：对**每个**本机 python 环境（见 python_interpreters）运行
/// importlib.metadata 探测脚本，仅纳入提供命令行入口的包（纯库不属于路线 B 范畴），
/// 并剔除每个环境自带的引导设施（pip/setuptools/wheel——它们不是用户装的工具，
/// 多环境下逐个列出只会产生 ×N 的噪音行）。
pub fn collect_pip() -> Result<Vec<Package>> {
    let mut packages = Vec::new();
    for py in python_interpreters() {
        if let Ok(mut ps) = pip_packages_for(&py) {
            packages.append(&mut ps);
        }
    }
    Ok(packages)
}

/// python 引导设施（环境自带的包，不参与审计）。
fn is_pip_infra(name: &str) -> bool {
    matches!(
        name.to_lowercase().replace('_', "-").as_str(),
        "pip" | "setuptools" | "wheel" | "pkg-resources" | "distribute"
    )
}

/// 用指定解释器枚举其 site-packages 里带 console_scripts 入口的包。
/// 失败（解释器损坏/输出不可解析）静默跳过该环境——多环境下单个坏环境
/// 不应让整个采集失败。
fn pip_packages_for(interpreter: &Path) -> Result<Vec<Package>> {
    const SCRIPT: &str = r#"
import json, sys
try:
    from importlib.metadata import distributions
except Exception as e:
    print(json.dumps([])); sys.exit(0)
out=[]
for d in distributions():
    name = d.metadata.get("Name")
    eps=[]
    try:
        eps=[f"{ep.name}" for ep in d.entry_points if ep.group=="console_scripts"]
    except Exception:
        pass
    if eps and name:
        out.append({"name":name,"version":d.version,"binaries":eps,
                    "location":str(d.locate_file(""))})
print(json.dumps(out))
"#;
    let mut cmd = Command::new(interpreter);
    cmd.args(["-c", SCRIPT]);
    let out = run_command(cmd)?;
    Ok(parse_pip_entrypoints(&out)?
        .into_iter()
        .filter(|p| !is_pip_infra(&p.name))
        .collect())
}

/// 枚举本机**全部** python 环境（多环境检测，返回解释器 exe 路径，去重）：
/// 1. `py -0p`——py 启动器注册表里的一切（系统安装 / 自定义注册 / uv / conda），
///    2026-11-19 实测能发现 CodeBuddy 注册的 venv 与 uv 托管解释器；
/// 2. 常见安装根目录（文件系统兜底，py 缺失时仍能发现）；
/// 3. PATH 上首个 python.exe（保持旧采集行为：主解释器必在列）。
///
/// 项目内 .venv 不扫：那些包是项目依赖而非全局 CLI 工具（经 py 注册的除外）。
pub fn python_interpreters() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let add = |p: PathBuf, out: &mut Vec<PathBuf>| {
        // 小写比较去重（py -0p 与 fs 枚举可能给出大小写不同的同一路径）
        if p.is_file() && !out.iter().any(|x| same_path(x, &p)) {
            out.push(p);
        }
    };
    let mut py_cmd = Command::new("py");
    py_cmd.arg("-0p");
    if let Ok(o) = run_command(py_cmd) {
        for p in parse_py_list(&o) {
            add(p, &mut out);
        }
    }
    for root in python_install_roots() {
        add(root.join("python.exe"), &mut out);
    }
    if let Some(p) = first_exe_on_path("python") {
        add(p, &mut out);
    }
    out
}

/// 解析 `py -0p` 输出（纯函数，便于单测）。每行形如：
/// ` -V:3.13 *        C:\Program Files\Python313\python.exe`
/// （tag 后可能有 `*` 默认标记；**路径可含空格**，须取行尾整段）。
pub fn parse_py_list(out: &str) -> Vec<PathBuf> {
    out.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let rest = line.strip_prefix("-V:")?;
            // 跳过 tag 与可选的 "*" 标记，剩余整段（trim 后）是解释器路径
            let after_tag = rest.split_once(char::is_whitespace).map(|(_, s)| s)?;
            let after_tag = after_tag
                .trim_start()
                .strip_prefix("* ")
                .unwrap_or(after_tag);
            let path = after_tag.trim_start();
            (!path.is_empty()).then(|| PathBuf::from(path))
        })
        .collect()
}

/// 常见 python 安装根目录（每根下应有 python.exe 与 Lib\site-packages）：
/// 系统安装、用户安装、conda 基环境与命名环境、uv 托管解释器
/// （uv 目录实测可能位于 %LOCALAPPDATA% 或 %APPDATA%）。
fn python_install_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let glob = |dir: &Path, prefix: &str, out: &mut Vec<PathBuf>| {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if p.is_dir() && name.starts_with(prefix) {
                    out.push(p);
                }
            }
        }
    };
    // 系统安装：C:\Program Files\Python3*（含 x86）
    glob(Path::new(r"C:\Program Files"), "Python3", &mut roots);
    glob(Path::new(r"C:\Program Files (x86)"), "Python3", &mut roots);
    // 用户安装：%LOCALAPPDATA%\Programs\Python\Python3*
    if let Some(user_py) = dirs::cache_dir().map(|d| d.join("Programs").join("Python")) {
        glob(&user_py, "Python3", &mut roots);
    }
    // conda：~/anaconda3|miniconda3 基环境 + envs\* 命名环境
    if let Some(home) = dirs::home_dir() {
        for base in ["anaconda3", "miniconda3"] {
            let root = home.join(base);
            if root.join("python.exe").is_file() {
                roots.push(root.clone());
            }
            let envs = root.join("envs");
            if let Ok(entries) = std::fs::read_dir(&envs) {
                for e in entries.flatten() {
                    if e.path().join("python.exe").is_file() {
                        roots.push(e.path());
                    }
                }
            }
        }
    }
    // uv 托管：%LOCALAPPDATA%\uv\python\* 与 %APPDATA%\uv\python\*
    for uv_root in [dirs::cache_dir(), dirs::data_dir()].into_iter().flatten() {
        let py_dir = uv_root.join("uv").join("python");
        if let Ok(entries) = std::fs::read_dir(&py_dir) {
            for e in entries.flatten() {
                if e.path().join("python.exe").is_file() {
                    roots.push(e.path());
                }
            }
        }
    }
    roots
}

/// PATH 上第一个 `<name>.exe` 的完整路径（python 环境枚举用；.bat 形态跳过——
/// 如 QClaw 的 python.bat 是转发 shim，不是独立环境）。
fn first_exe_on_path(name: &str) -> Option<PathBuf> {
    let exe = format!("{name}.exe");
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(&exe))
            .find(|p| p.is_file())
    })
}

/// Windows 路径大小写不敏感的相等比较（py -0p 与 fs 枚举可能给出大小写不同的写法）。
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// 解析 python entry_points 探测脚本的 JSON 输出（纯函数，便于单测）。
pub fn parse_pip_entrypoints(out: &str) -> Result<Vec<Package>> {
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(out.trim()).with_context(|| "parsing python entry points json")?;
    let mut packages = Vec::new();
    for r in rows {
        let Some(name) = r.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let mut p = Package::new(name, Manager::Pip);
        p.version = r
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        p.binaries = r
            .get("binaries")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        p.install_path = r
            .get("location")
            .and_then(|v| v.as_str())
            .map(PathBuf::from);
        p.confidence = Confidence::Inferred;
        packages.push(p);
    }
    Ok(packages)
}

/// `uv tool list` 输出解析（纯函数，便于单测）：
/// 头行 "name vX.Y.Z"（或 "name vX.Y.Z (path)"），子行 "- bin"。
pub fn parse_uv_list(out: &str) -> Vec<Package> {
    let mut packages = Vec::new();
    let mut current: Option<Package> = None;
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            if let Some(p) = current.as_mut() {
                p.binaries.push(strip_exe(rest.trim()).to_string());
            }
        } else {
            if let Some(p) = current.take() {
                packages.push(p);
            }
            // 格式: "name vX.Y.Z" 或 "name vX.Y.Z (path)"，path 段是 venv 目录
            // （atime 探测与新装观察期都依赖 install_path，不能丢）
            let mut parts = trimmed.split_whitespace();
            let name = parts.next().unwrap_or("").to_string();
            let version = parts
                .find(|s| s.starts_with('v'))
                .map(|s| s[1..].to_string());
            let install_path = trimmed
                .find('(')
                .and_then(|i| trimmed[i + 1..].strip_suffix(')'))
                .map(PathBuf::from);
            let mut p = Package::new(name, Manager::Uv);
            p.version = version;
            p.install_path = install_path;
            p.confidence = Confidence::Exact;
            current = Some(p);
        }
    }
    if let Some(p) = current {
        packages.push(p);
    }
    packages
}

pub fn collect_uv() -> Result<Vec<Package>> {
    if !tool_available("uv") {
        return Ok(Vec::new());
    }
    let mut cmd = command_for("uv");
    cmd.args(["tool", "list"]);
    let out = run_command(cmd)?;
    Ok(parse_uv_list(&out))
}

/// `pipx list --json` 输出解析（纯函数，便于单测）。
pub fn parse_pipx_json(out: &str) -> Result<Vec<Package>> {
    let json: serde_json::Value =
        serde_json::from_str(out).with_context(|| "parsing pipx list --json")?;
    let mut packages = Vec::new();
    if let Some(venvs) = json.get("venvs").and_then(|v| v.as_object()) {
        // pipx venvs 固定布局：<PIPX_HOME>/venvs/<name>（默认 %LOCALAPPDATA%\pipx）。
        // install_path 供 atime 探测与新装观察期使用。
        let pipx_home = std::env::var_os("PIPX_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::data_local_dir().map(|d| d.join("pipx")));
        for (name, val) in venvs {
            let mut p = Package::new(name, Manager::Pipx);
            p.version = val
                .get("metadata")
                .and_then(|m| m.get("main_package"))
                .and_then(|m| m.get("version"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            p.binaries = val
                .get("apps")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            p.install_path = pipx_home.as_ref().map(|h| h.join("venvs").join(name));
            p.confidence = Confidence::Exact;
            packages.push(p);
        }
    }
    Ok(packages)
}

pub fn collect_pipx() -> Result<Vec<Package>> {
    if !tool_available("pipx") {
        return Ok(Vec::new());
    }
    let mut cmd = command_for("pipx");
    cmd.args(["list", "--json"]);
    let out = run_command(cmd)?;
    parse_pipx_json(&out)
}

// ---------------------------------------------------------------------------
// winget (portable)
// ---------------------------------------------------------------------------

/// winget 便携包采集：`WinGet\Links\<命令>.exe` 是指向
/// `WinGet\Packages\<包ID>\<命令>.exe` 的符号链接（Links 目录在 PATH 上）。
/// 解析链接目标、按包 ID 归组——bin 名就是链接名（用户实际输入的命令），
/// 映射精确（Confidence::Exact），install_path 供 atime 探测与观察期使用。
///
/// 只覆盖便携包：`winget list` 的 source=winget 无法区分「winget 安装」与
/// 「安装器自装、恰好匹配目录」（Git / GitHub CLI 等 MSI 应用即如此），也没有
/// bin 清单可用——纳入会产生无证据的「未用」误报，违背可反驳证据原则。
pub fn collect_winget() -> Result<Vec<Package>> {
    let Some(local) = dirs::data_local_dir() else {
        return Ok(Vec::new());
    };
    let links_dir = local.join(r"Microsoft\WinGet\Links");
    if !links_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut links: Vec<(String, Option<PathBuf>)> = Vec::new();
    for entry in
        std::fs::read_dir(&links_dir).with_context(|| format!("reading {}", links_dir.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // 读符号链接目标；读不到（断链/非链接）的条目跳过
        let target = std::fs::read_link(entry.path()).ok();
        links.push((name, target));
    }
    Ok(group_winget_portable(
        &links,
        &local.join(r"Microsoft\WinGet\Packages"),
    ))
}

/// 纯函数：把 Links 的（链接名 → 目标路径）按 `Packages\<包ID>\` 前缀归组为包。
/// 目标不在 Packages 根下的链接跳过（Links 可能含非便携包条目）。
fn group_winget_portable(
    links: &[(String, Option<PathBuf>)],
    packages_root: &Path,
) -> Vec<Package> {
    // BTreeMap 让输出按包 ID 有序、bin 排序——与 collect_all 的排序语义一致
    let mut bins_by_pkg: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (link, target) in links {
        let Some(target) = target else { continue };
        let Ok(rel) = target.strip_prefix(packages_root) else {
            continue;
        };
        let Some(pkg_id) = rel.components().next() else {
            continue;
        };
        bins_by_pkg
            .entry(pkg_id.as_os_str().to_string_lossy().into_owned())
            .or_default()
            .push(strip_exe(link).to_string());
    }
    bins_by_pkg
        .into_iter()
        .map(|(pkg_id, mut binaries)| {
            binaries.sort();
            let mut p = Package::new(pkg_id.clone(), Manager::Winget);
            p.binaries = binaries;
            p.install_path = Some(packages_root.join(pkg_id));
            p.confidence = Confidence::Exact;
            p
        })
        .collect()
}

/// 汇总全部可用的采集器。单个采集器失败不影响其余（降级为警告）。
pub fn collect_all() -> (Vec<Package>, Vec<String>) {
    let mut packages = Vec::new();
    let mut warnings = Vec::new();
    type Collector = fn() -> Result<Vec<Package>>;
    let collectors: Vec<(&str, Collector)> = vec![
        ("cargo", collect_cargo),
        ("npm", collect_npm),
        ("pip", collect_pip),
        ("uv", collect_uv),
        ("pipx", collect_pipx),
        ("winget", collect_winget),
    ];
    for (name, f) in collectors {
        match f() {
            Ok(mut ps) => packages.append(&mut ps),
            Err(e) => warnings.push(format!("[{name}] collector failed: {e:#}")),
        }
    }
    packages.sort_by(|a, b| {
        a.manager
            .as_str()
            .cmp(b.manager.as_str())
            .then_with(|| a.name.cmp(&b.name))
    });
    (packages, warnings)
}

/// 去掉 Windows 可执行后缀，统一比较 basename。
pub fn strip_exe(s: &str) -> &str {
    let base = basename(s);
    base.strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".cmd"))
        .or_else(|| base.strip_suffix(".bat"))
        .unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_py_list_extracts_paths_with_spaces() {
        // `py -0p` 实测格式：tag + 可选 * 默认标记 + 路径（可含空格，取行尾整段）
        let out = concat!(
            " -V:3.13 *        C:\\Program Files\\Python313\\python.exe\n",
            " -V:3.11          C:\\Users\\qi.shao\\CodeBuddy\\py311\\python.exe\n",
            " -V:Astral/CPython3.12.13 C:\\Users\\qi.shao\\AppData\\Roaming\\uv\\python\\cpython-3.12\\python.exe\n",
        );
        let paths = parse_py_list(out);
        assert_eq!(paths.len(), 3);
        assert_eq!(
            paths[0],
            PathBuf::from(r"C:\Program Files\Python313\python.exe")
        );
        assert!(paths[2].ends_with(r"cpython-3.12\python.exe"));
        // 非 -V 行（表头/错误输出）与无路径的残行都不产生条目
        assert!(parse_py_list("Installed Pythons found\n -V:broken\n").is_empty());
    }

    #[test]
    fn npm_prefix_mark_scan_finds_prefixes_and_skips_noise() {
        let dir = std::env::temp_dir().join(format!("oa_npm_mark_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // 两层命中（锁文件标记）：vendor\npm-global\node_modules\.package-lock.json
        let locked = dir.join("QClaw").join("npm-global").join("node_modules");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join(".package-lock.json"), "{}").unwrap();
        // 两层命中（shim 对标记，QClaw 实测布局）：无锁文件但有同名 .cmd/.ps1
        let shim_based = dir.join("Vendor2").join("npm-global");
        std::fs::create_dir_all(shim_based.join("node_modules")).unwrap();
        std::fs::write(shim_based.join("tool.cmd"), "@echo off").unwrap();
        std::fs::write(shim_based.join("tool.ps1"), "").unwrap();
        // 首层命中：<root>\npm\node_modules\.package-lock.json（默认 prefix 布局）
        let npm = dir.join("npm").join("node_modules");
        std::fs::create_dir_all(&npm).unwrap();
        std::fs::write(npm.join(".package-lock.json"), "{}").unwrap();
        // node 项目目录：node_modules + 成对 shim 但根上有 package.json → 排除
        let project = dir.join("myproject");
        std::fs::create_dir_all(project.join("node_modules")).unwrap();
        std::fs::write(project.join("package.json"), "{}").unwrap();
        std::fs::write(project.join("build.cmd"), "").unwrap();
        std::fs::write(project.join("build.ps1"), "").unwrap();
        // 无任何标记的应用 node_modules：不算 npm 全局 prefix
        std::fs::create_dir_all(dir.join("someapp").join("node_modules")).unwrap();
        // 噪音目录（Packages）内部即使有标记也不下探
        let noise = dir.join("Packages").join("deep").join("node_modules");
        std::fs::create_dir_all(&noise).unwrap();
        std::fs::write(noise.join(".package-lock.json"), "{}").unwrap();

        let mut out = Vec::new();
        scan_npm_prefix_mark(&dir, 0, &mut out);
        assert_eq!(out.len(), 3, "{out:?}");
        assert!(out.contains(&dir.join("QClaw").join("npm-global")));
        assert!(out.contains(&shim_based));
        assert!(out.contains(&dir.join("npm")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pip_infra_packages_are_excluded() {
        assert!(is_pip_infra("pip"));
        assert!(is_pip_infra("setuptools"));
        assert!(is_pip_infra("Wheel"));
        assert!(is_pip_infra("pkg_resources"));
        assert!(!is_pip_infra("httpx"));
        assert!(!is_pip_infra("pipx")); // 前缀相同但不是引导设施
    }

    #[test]
    fn strip_exe_handles_windows_suffixes() {
        assert_eq!(strip_exe(r"C:\x\bin\rg.exe"), "rg");
        assert_eq!(strip_exe("npm.cmd"), "npm");
        assert_eq!(strip_exe("plain.bat"), "plain");
        assert_eq!(strip_exe("/usr/bin/rg"), "rg");
    }

    #[test]
    fn npmrc_prefix_parses_ini_and_ignores_comments() {
        assert_eq!(
            npmrc_prefix("prefix = C:\\npm-global\n"),
            Some(PathBuf::from("C:\\npm-global"))
        );
        assert_eq!(
            npmrc_prefix("prefix = \"C:/quoted path\"\n"),
            Some(PathBuf::from("C:/quoted path"))
        );
        assert_eq!(
            npmrc_prefix("# c\n; c2\nregistry=https://x\nother=1\n"),
            None
        );
        assert_eq!(npmrc_prefix(""), None);
    }

    #[test]
    fn npm_scan_enumerates_plain_scoped_and_libonly_packages() {
        let root = std::env::temp_dir().join(format!("oa_npm_scan_{}", std::process::id()));
        std::fs::create_dir_all(root.join(".bin")).unwrap(); // 基础设施目录须跳过
        let mk = |rel: &str, name: &str, bin: Option<&str>| {
            let dir = root.join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            let bin_json = match bin {
                Some(b) => format!(r#","bin":{{"{b}":"bin/x.js"}}"#),
                None => String::new(),
            };
            std::fs::write(
                dir.join("package.json"),
                format!(r#"{{"name":"{name}","version":"1.0.0"{bin_json}}}"#),
            )
            .unwrap();
        };
        mk("plain-tool", "plain-tool", Some("plain"));
        mk(r"@scope\scoped", "@scope/scoped", Some("scoped"));
        mk("lib-only", "lib-only", None);
        // package.json 不可读（如被安全软件加密重命名）：兜底 = 目录名 + bin/ 脚本名
        let broken = root.join("broken-tool");
        std::fs::create_dir_all(broken.join("bin")).unwrap();
        std::fs::write(broken.join("bin").join("mystery.js"), "x").unwrap();
        std::fs::write(broken.join("bin").join("mystery.js.IPGSD"), "x").unwrap();

        let ps = npm_packages_under(&root);
        let mut names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["@scope/scoped", "broken-tool", "lib-only", "plain-tool"]
        );
        let broken = ps.iter().find(|p| p.name == "broken-tool").unwrap();
        assert_eq!(broken.binaries, vec!["mystery"]); // .IPGSD 残留不收
        assert_eq!(broken.confidence, Confidence::Inferred);
        assert!(broken.version.is_none());
        let scoped = ps.iter().find(|p| p.name == "@scope/scoped").unwrap();
        assert_eq!(scoped.binaries, vec!["scoped"]);
        assert_eq!(scoped.confidence, Confidence::Exact);
        assert!(scoped
            .install_path
            .as_deref()
            .is_some_and(|p| p.ends_with(r"@scope\scoped")));
        let lib = ps.iter().find(|p| p.name == "lib-only").unwrap();
        assert!(lib.binaries.is_empty());
        assert_eq!(lib.confidence, Confidence::None);
        assert_eq!(lib.version.as_deref(), Some("1.0.0"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn npm_bin_string_and_object_shapes() {
        let dir = std::env::temp_dir().join(format!("oa_npm_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pj = dir.join("package.json");
        std::fs::write(&pj, r#"{"bin":{"tool":"bin/tool.js","other":"bin/o.js"}}"#).unwrap();
        let mut bins = npm_bin_names(&pj, "pkg");
        bins.sort();
        assert_eq!(bins, vec!["other", "tool"]);
        // 字符串形态的可执行名是包名（scoped 包取末段），不是脚本文件名
        std::fs::write(&pj, r#"{"bin":"bin/single.js"}"#).unwrap();
        assert_eq!(npm_bin_names(&pj, "single"), vec!["single"]);
        assert_eq!(npm_bin_names(&pj, "@scope/tool"), vec!["tool"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn npm_packages_under_skips_dot_dirs_incl_scope_children() {
        // 已卸载的包被安全代理改名成 `.opencli-XXXX` 留在 scope 目录下：
        // 顶层与 @scope 子层的点前缀目录都不是包，绝不能让它「死而复生」
        let root = std::env::temp_dir().join(format!("oa_npm_dots_{}", std::process::id()));
        let modules = root.join("node_modules");
        std::fs::create_dir_all(modules.join("@scope").join("real-pkg")).unwrap();
        std::fs::create_dir_all(modules.join("@scope").join(".quarantined-Xy12")).unwrap();
        std::fs::create_dir_all(modules.join(".pnpm-store")).unwrap();
        std::fs::create_dir_all(modules.join("plain")).unwrap();
        for (pj, name) in [
            (
                modules.join("@scope").join("real-pkg").join("package.json"),
                "real",
            ),
            (modules.join("plain").join("package.json"), "plain"),
        ] {
            std::fs::write(pj, format!(r#"{{"name":"{name}","version":"1.0.0"}}"#)).unwrap();
        }
        let mut names: Vec<String> = npm_packages_under(&modules)
            .into_iter()
            .map(|p| p.name)
            .collect();
        names.sort();
        // 名字取自 package.json 的 name 字段（与真实采集一致）；点前缀目录不出现
        assert_eq!(names, vec!["plain", "real"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn orphan_shims_flag_leftovers_and_keep_everything_owned() {
        // 伪造一个 npm prefix：活包（bin 字段精确 + 加密 fallback + 目录名兜底）
        // 与孤儿三件套并存，验证六类反例
        let prefix = std::env::temp_dir().join(format!("oa_shims_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&prefix);
        let modules = prefix.join("node_modules");
        // 活包：bin 字段精确归属
        std::fs::create_dir_all(modules.join("live-pkg")).unwrap();
        std::fs::write(
            modules.join("live-pkg").join("package.json"),
            r#"{"name":"live-pkg","bin":{"live":"bin/l.js"}}"#,
        )
        .unwrap();
        // 加密 fallback 包：package.json 不可读，bin/ 脚本名归属（confidence 推断）
        std::fs::create_dir_all(modules.join("broken-pkg").join("bin")).unwrap();
        std::fs::write(
            modules.join("broken-pkg").join("bin").join("mystery.js"),
            "x",
        )
        .unwrap();
        // 归属反例的启动器：bin 字段归属（live）与 fallback bins 归属（mystery）
        for f in ["live", "live.cmd", "live.ps1", "mystery.cmd"] {
            std::fs::write(prefix.join(f), "x").unwrap();
        }
        // 孤儿三件套 + IPGSD 伴生（包目录已删）
        for f in ["orphan", "orphan.cmd", "orphan.ps1", "orphan.cmd.IPGSD"] {
            std::fs::write(prefix.join(f), "x").unwrap();
        }
        // 半套残缺：.ps1 + 无扩展名伴生（cmd 被 TSD 吃掉）仍是孤儿
        for f in ["half", "half.ps1"] {
            std::fs::write(prefix.join(f), "x").unwrap();
        }
        // 非候选：孤立 .ps1（无 sh 伴生，可能是用户自己的脚本）
        std::fs::write(prefix.join("lone.ps1"), "x").unwrap();
        // 非候选：.bat 不是 npm 产物（QClaw 的 pip.bat/python.bat 实测布局）
        std::fs::write(prefix.join("pip.bat"), "@echo off").unwrap();
        // 非候选：node 自管核心命令
        for f in ["npm", "npm.cmd", "npm.ps1", "npx.cmd", "corepack.ps1"] {
            std::fs::write(prefix.join(f), "x").unwrap();
        }
        // 非候选：目录即使叫 *.cmd 也不是启动器
        std::fs::create_dir_all(prefix.join("weird.cmd")).unwrap();

        let shims = npm_orphan_shims_in(&prefix);
        let bins: Vec<&str> = shims.iter().map(|s| s.bin.as_str()).collect();
        assert_eq!(bins, vec!["half", "orphan"], "{shims:?}");
        let orphan = shims.iter().find(|s| s.bin == "orphan").unwrap();
        assert_eq!(orphan.files.len(), 4, "sh + cmd + ps1 + IPGSD: {orphan:?}");
        assert!(orphan.files.contains(&prefix.join("orphan.cmd.IPGSD")));
        let half = shims.iter().find(|s| s.bin == "half").unwrap();
        assert_eq!(half.files.len(), 2, "{half:?}");
        assert_eq!(half.prefix, prefix);
        let _ = std::fs::remove_dir_all(&prefix);
    }

    #[test]
    fn orphan_shim_ownership_covers_fallback_and_dir_name() {
        // 归属双保险的第二条：包目录存在但 package.json 不可读时，
        // node_modules/<stem> 目录本身即是归属证据（fallback 包名取目录名）
        let prefix = std::env::temp_dir().join(format!("oa_shims_own_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&prefix);
        let modules = prefix.join("node_modules");
        std::fs::create_dir_all(modules.join("enc-pkg").join("bin")).unwrap();
        std::fs::write(modules.join("enc-pkg").join("bin").join("enc-pkg.js"), "x").unwrap();
        for f in ["enc-pkg", "enc-pkg.cmd", "enc-pkg.ps1"] {
            std::fs::write(prefix.join(f), "x").unwrap();
        }
        // bin 名与目录名不同（bin key ≠ 脚本名）：目录名兜底使其不算孤儿
        assert!(npm_orphan_shims_in(&prefix).is_empty());
        // 包目录移除后同一组启动器立即成为孤儿
        std::fs::remove_dir_all(modules.join("enc-pkg")).unwrap();
        let shims = npm_orphan_shims_in(&prefix);
        assert_eq!(shims.len(), 1);
        assert_eq!(shims[0].bin, "enc-pkg");
        let _ = std::fs::remove_dir_all(&prefix);
    }

    #[test]
    fn crates2_parsing_shape() {
        // 内置 fixture 替代真机 .crates2.json，避免文件缺失时测试空转
        let raw = r#"{"installs":{"ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)":{"version_req":"=14.1.0","bins":["rg"]},"oldtool 0.1.0 (registry+…)":{"binaries":["oldtool"]}}}"#;
        let ps = parse_crates2(raw, PathBuf::from(r"C:\u\.cargo\bin")).unwrap();
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].name, "oldtool"); // 按名排序
        assert_eq!(ps[0].binaries, vec!["oldtool"]); // 旧版字段兼容
        assert_eq!(ps[1].name, "ripgrep");
        assert_eq!(ps[1].version.as_deref(), Some("14.1.0"));
        assert_eq!(ps[1].binaries, vec!["rg"]);
        assert_eq!(
            ps[1].install_path.as_deref(),
            Some(std::path::Path::new(r"C:\u\.cargo\bin"))
        );
    }

    #[test]
    fn uv_list_parses_headers_bins_and_path_suffix() {
        let out = "dusty v0.1.0\n- dusty\n- dustbin\n\nother v2.0.1 (C:\\tools\\other)\n- other\n";
        let ps = parse_uv_list(out);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].name, "dusty");
        assert_eq!(ps[0].version.as_deref(), Some("0.1.0"));
        assert_eq!(ps[0].binaries, vec!["dusty", "dustbin"]);
        assert_eq!(ps[1].name, "other");
        assert_eq!(ps[1].version.as_deref(), Some("2.0.1")); // "(path)" 后缀不污染版本
        assert_eq!(ps[1].binaries, vec!["other"]);
        // "(path)" 段是 venv 目录，必须保留（atime 与新装观察期依赖）
        assert_eq!(
            ps[1].install_path.as_deref(),
            Some(std::path::Path::new(r"C:\tools\other"))
        );
        assert!(ps[0].install_path.is_none()); // 无 path 段的行保持 None
        assert!(parse_uv_list("").is_empty());
    }

    #[test]
    fn pipx_json_parses_venvs_apps_and_version() {
        let out = r#"{"venvs":{"ruff":{"metadata":{"main_package":{"version":"0.8.0"}},"apps":["ruff","ruff-lsp"]}}}"#;
        let ps = parse_pipx_json(out).unwrap();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].name, "ruff");
        assert_eq!(ps[0].version.as_deref(), Some("0.8.0"));
        assert_eq!(ps[0].binaries, vec!["ruff", "ruff-lsp"]);
        // install_path 按 <PIPX_HOME>/venvs/<name> 推断（venvs 布局固定）
        let ip = ps[0].install_path.as_ref().expect("pipx install_path");
        assert!(ip.ends_with("ruff"), "got {}", ip.display());
        assert_eq!(parse_pipx_json("{}").unwrap(), Vec::<Package>::new());
    }

    #[test]
    fn winget_portable_groups_links_by_package_dir() {
        let root = Path::new(r"C:\W\Packages");
        let links = vec![
            (
                "rg.exe".to_string(),
                Some(root.join(r"BurntSushi.ripgrep.MSVC\rg.exe")),
            ),
            (
                "rga.exe".to_string(),
                Some(root.join(r"BurntSushi.ripgrep.MSVC\nested\rga.exe")),
            ),
            ("jq.exe".to_string(), Some(root.join(r"jqlang.jq\jq.exe"))),
            ("broken.exe".to_string(), None),
            (
                "outside.exe".to_string(),
                Some(PathBuf::from(r"C:\elsewhere\x.exe")),
            ),
        ];
        let ps = group_winget_portable(&links, root);
        assert_eq!(ps.len(), 2); // 断链与 Packages 根外目标跳过
        assert_eq!(ps[0].name, "BurntSushi.ripgrep.MSVC");
        assert_eq!(ps[0].binaries, vec!["rg", "rga"]); // bin = 链接名；排序稳定
        assert_eq!(
            ps[0].install_path.as_deref(),
            Some(Path::new(r"C:\W\Packages\BurntSushi.ripgrep.MSVC"))
        );
        assert_eq!(ps[0].confidence, Confidence::Exact);
        assert_eq!(ps[1].name, "jqlang.jq");
        assert!(ps[1].version.is_none()); // 目录布局不含版本信息
    }

    #[test]
    fn pip_entrypoints_parses_name_binaries_location() {
        let out = r#"[{"name":"httpie","version":"3.2.0","binaries":["http","https"],"location":"C:\\py\\Lib\\site-packages"},{"version":"1.0"}]"#;
        let ps = parse_pip_entrypoints(out).unwrap();
        assert_eq!(ps.len(), 1); // 缺 name 的行跳过
        assert_eq!(ps[0].name, "httpie");
        assert_eq!(ps[0].binaries, vec!["http", "https"]);
        assert_eq!(
            ps[0].install_path,
            Some(PathBuf::from("C:\\py\\Lib\\site-packages"))
        );
    }
}

//! 清单采集层：枚举各包管理器安装的全局包。
//!
//! 每个采集器遵循同一原则：
//! 1. 缺失的包管理器不报错，只是贡献空列表（与 shelfware 一致）。
//! 2. 一律走各管理器的公开接口（命令行 JSON / 元数据文件），不猜测、不 scraping。
//! 3. 包->二进制映射尽量精确：cargo 读 .crates2.json，npm 读 package.json 的 bin 字段。

use crate::model::{basename, Confidence, Manager, Package};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

/// 探测命令是否可用。
pub fn tool_available(name: &str) -> bool {
    let mut cmd = Command::new(name);
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

/// 解析 cargo 的 .crates2.json，得到 包名 -> [二进制名] 的精确映射。
/// 结构：{ "installs": { "crate-name version (source)": { "version_req", "bins": [...], ... } } }
pub fn collect_cargo() -> Result<Vec<Package>> {
    let home = cargo_home();
    let meta = home.join(".crates2.json");
    if !meta.exists() {
        return Ok(Vec::new());
    }
    let raw =
        std::fs::read_to_string(&meta).with_context(|| format!("reading {}", meta.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| "parsing .crates2.json")?;
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
            p.install_path = Some(home.join("bin"));
            p.confidence = Confidence::Exact;
            packages.push(p);
        }
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packages)
}

// ---------------------------------------------------------------------------
// npm (global)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct NpmList {
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, serde_json::Value>,
}

/// 获取 npm 全局 prefix（bin 目录的上级）。
fn npm_prefix() -> Result<PathBuf> {
    let mut cmd = Command::new("npm");
    cmd.args(["root", "-g"]);
    let out = run_command(cmd)?;
    let root = PathBuf::from(out.trim());
    // npm root -g 返回 <prefix>/node_modules
    Ok(root.parent().map(Path::to_path_buf).unwrap_or(root))
}

/// 解析 npm package.json 的 bin 字段（string 或 object 两种形态）。
fn npm_bin_names(pkg_json: &Path) -> Vec<String> {
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
        serde_json::Value::String(s) => vec![strip_exe(basename(s)).to_string()],
        serde_json::Value::Object(map) => map
            .keys()
            .map(|k| strip_exe(basename(k)).to_string())
            .collect(),
        _ => Vec::new(),
    }
}

pub fn collect_npm() -> Result<Vec<Package>> {
    if !tool_available("npm") {
        return Ok(Vec::new());
    }
    let mut cmd = Command::new("npm");
    cmd.args(["ls", "-g", "--json", "--depth=0"]);
    let out = run_command(cmd)?;
    let list: NpmList = serde_json::from_str(&out).with_context(|| "parsing npm ls -g --json")?;
    let prefix = npm_prefix().unwrap_or_default();
    let modules_root = prefix.join("node_modules");

    let mut packages = Vec::new();
    for (name, meta) in list.dependencies {
        let version = meta
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let install_path = modules_root.join(&name);
        // 精确映射：读包自己的 package.json
        let binaries = npm_bin_names(&install_path.join("package.json"));
        let confidence = if binaries.is_empty() {
            // 没有 bin 字段的全局包通常是纯库，不提供 CLI -> 不纳入路线 B 的匹配
            Confidence::None
        } else {
            Confidence::Exact
        };
        let mut p = Package::new(name, Manager::Npm);
        p.version = version;
        p.binaries = binaries;
        p.install_path = Some(install_path);
        p.confidence = confidence;
        packages.push(p);
    }
    Ok(packages)
}

// ---------------------------------------------------------------------------
// pip (CLI tools only) / uv tool / pipx
// ---------------------------------------------------------------------------

/// pip 安装的 CLI 入口：扫描 Scripts 目录下的 .exe / .cmd，并用
/// importlib.metadata 的 entry_points 建立「包 -> 命令」映射。
/// 仅纳入提供命令行入口的包（纯库不属于路线 B 范畴）。
pub fn collect_pip() -> Result<Vec<Package>> {
    if !tool_available("python") && !tool_available("python3") {
        return Ok(Vec::new());
    }
    let py = if tool_available("python") {
        "python"
    } else {
        "python3"
    };
    let script = r#"
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
    let mut cmd = Command::new(py);
    cmd.args(["-c", script]);
    let out = run_command(cmd)?;
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
                    .map(str::to_string)
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

/// `uv tool list` 输出解析：形如 "dusty v0.1.0\n- dusty\n- dustbin"。
pub fn collect_uv() -> Result<Vec<Package>> {
    if !tool_available("uv") {
        return Ok(Vec::new());
    }
    let mut cmd = Command::new("uv");
    cmd.args(["tool", "list"]);
    let out = run_command(cmd)?;
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
            // 格式: "name vX.Y.Z" 或 "name vX.Y.Z (path)"
            let mut parts = trimmed.split_whitespace();
            let name = parts.next().unwrap_or("").to_string();
            let version = parts
                .find(|s| s.starts_with('v'))
                .map(|s| s[1..].to_string());
            let mut p = Package::new(name, Manager::Uv);
            p.version = version;
            p.confidence = Confidence::Exact;
            current = Some(p);
        }
    }
    if let Some(p) = current {
        packages.push(p);
    }
    Ok(packages)
}

/// `pipx list --json` 输出解析。
pub fn collect_pipx() -> Result<Vec<Package>> {
    if !tool_available("pipx") {
        return Ok(Vec::new());
    }
    let mut cmd = Command::new("pipx");
    cmd.args(["list", "--json"]);
    let out = run_command(cmd)?;
    let json: serde_json::Value =
        serde_json::from_str(&out).with_context(|| "parsing pipx list --json")?;
    let mut packages = Vec::new();
    if let Some(venvs) = json.get("venvs").and_then(|v| v.as_object()) {
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
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            p.confidence = Confidence::Exact;
            packages.push(p);
        }
    }
    Ok(packages)
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
    fn strip_exe_handles_windows_suffixes() {
        assert_eq!(strip_exe(r"C:\x\bin\rg.exe"), "rg");
        assert_eq!(strip_exe("npm.cmd"), "npm");
        assert_eq!(strip_exe("plain.bat"), "plain");
        assert_eq!(strip_exe("/usr/bin/rg"), "rg");
    }

    #[test]
    fn npm_bin_string_and_object_shapes() {
        let dir = std::env::temp_dir().join(format!("oa_npm_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pj = dir.join("package.json");
        std::fs::write(&pj, r#"{"bin":{"tool":"bin/tool.js","other":"bin/o.js"}}"#).unwrap();
        let mut bins = npm_bin_names(&pj);
        bins.sort();
        assert_eq!(bins, vec!["other", "tool"]);
        std::fs::write(&pj, r#"{"bin":"bin/single.js"}"#).unwrap();
        assert_eq!(npm_bin_names(&pj), vec!["single.js"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cargo_meta_parsing_shape() {
        // 验证 .crates2.json 的字段假设与本机实际结构一致
        let home = cargo_home();
        let meta = home.join(".crates2.json");
        if !meta.exists() {
            return;
        }
        let raw = std::fs::read_to_string(&meta).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let installs = json.get("installs").and_then(|v| v.as_object());
        assert!(installs.is_some(), ".crates2.json should have 'installs'");
        let first = installs.unwrap().values().next().unwrap();
        assert!(
            first.get("bins").is_some() || first.get("binaries").is_some(),
            "entry should have 'bins' or 'binaries'"
        );
    }
}

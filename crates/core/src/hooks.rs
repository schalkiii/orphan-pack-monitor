//! import 钩子管理（原 CLI `mod hooks`，迁入 core 供 GUI 复用——同一实现保证
//! CLI 与 GUI 行为一字不差）：python `.pth`（各解释器 site-packages）+ node
//! 用户级 `NODE_OPTIONS --require`。钩子运行在解释器/node 进程内部，把顶层
//! import / node_modules 解析追加到 imports/events.jsonl，由审计消费为 Import
//! 证据。**仅 Windows**（reg 注册表 + Windows 路径布局），lib.rs 按 cfg 挂载。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::imports;
use crate::inventory::{python_interpreters, run_command, CREATE_NO_WINDOW};

/// .pth 引用的钩子模块名与日志布局的单一事实源在 core（default_hook_dir）。
const PY_HOOK: &str = include_str!("../assets/oa_import_hook.py");
const JS_HOOK: &str = include_str!("../assets/oa_node_hook.cjs");
const PTH_NAME: &str = "oa_import_hook.pth";
const HOOK_MARKER: &str = "oa_node_hook.cjs";

/// import 日志的落盘状态（status 呈现用；R14 起「缺失」与「被隔离」要分开说）。
#[derive(Debug, Clone, PartialEq)]
pub enum ImportLogState {
    /// 日志在（字节数）。
    Present(u64),
    /// 被安全代理整文件隔离（影子路径；R12 实测 churn 形态）。
    Quarantined(PathBuf),
    /// 尚未创建（正常早期态）。
    Missing,
}

/// 钩子安装状态快照（`status_info` 返回；CLI 渲染文本、GUI 渲染面板）。
#[derive(Debug, Clone)]
pub struct HooksStatus {
    /// 每解释器：(解释器路径, 是否已挂 .pth)。
    pub interpreters: Vec<(PathBuf, bool)>,
    /// 工具 venv：(已挂, 总数)（uv / pipx）。
    pub tool_venvs: (usize, usize),
    /// 用户级 NODE_OPTIONS 是否已含我们的 --require。
    pub node_hooked: bool,
    /// import 日志路径 + 落盘状态。
    pub log: Option<(PathBuf, ImportLogState)>,
}

/// 部署钩子文件到数据目录（单份，.pth / NODE_OPTIONS 都引用这里）。
fn write_hook_files(hook_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(hook_dir)?;
    let py = hook_dir.join("oa_import_hook.py");
    let js = hook_dir.join(HOOK_MARKER);
    std::fs::write(&py, PY_HOOK)?;
    std::fs::write(&js, JS_HOOK)?;
    Ok((py, js))
}

/// 查询解释器的 purelib 路径（-c 脚本；失败 None——坏环境静默跳过）。
fn py_purelib(interpreter: &Path) -> Option<PathBuf> {
    let mut cmd = Command::new(interpreter);
    cmd.args([
        "-c",
        "import sysconfig;print(sysconfig.get_paths()['purelib'])",
    ]);
    run_command(cmd).ok().map(|s| PathBuf::from(s.trim()))
}

/// 解释器的版本标签（用户级 site 回退目录用）：Python313 形态。
fn py_version_tag(interpreter: &Path) -> Option<String> {
    let mut cmd = Command::new(interpreter);
    cmd.args([
        "-c",
        "import sys;print('Python%d%d' % sys.version_info[:2])",
    ]);
    run_command(cmd).ok().map(|s| s.trim().to_string())
}

/// .pth 内容：把钩子目录挂上 sys.path 后 import 钩子（单行，pth 规范）。
fn pth_line(hook_dir: &Path) -> String {
    format!(
        "import sys;sys.path.append(r\"{}\");import oa_import_hook",
        hook_dir.display()
    )
}

/// 跑 reg 命令并收集完整 Output（成败看退出码；CREATE_NO_WINDOW 由
/// run_command 的约定保持一致，这里直接内联 flags）。
fn reg(args: &[&str]) -> Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("reg");
    cmd.args(args).creation_flags(CREATE_NO_WINDOW);
    cmd.output().map_err(Into::into)
}

/// 读用户级 NODE_OPTIONS（HKCU\Environment，进程环境变量不算——旧值判断以注册表为准）。
fn node_options_current() -> Option<String> {
    let out = reg(&["query", r"HKCU\Environment", "/v", "NODE_OPTIONS"]).ok()?;
    if !out.status.success() {
        return None; // 值不存在
    }
    parse_reg_value(&String::from_utf8_lossy(&out.stdout))
}

/// 解析 `reg query` 输出里的值（取 REG_SZ 之后的整段，纯函数便于测试）。
pub fn parse_reg_value(out: &str) -> Option<String> {
    out.lines()
        .filter(|l| l.contains("REG_SZ"))
        .map(|l| l.split("REG_SZ").nth(1).unwrap_or("").trim().to_string())
        .find(|s| !s.is_empty())
}

/// 把我们的 --require 段并入现有 NODE_OPTIONS。已含钩子时按路径核对：
/// 数据目录迁移后旧路径会被刷新为当前路径（否则 install 永远报
/// already configured，node 侧 import 证据静默丢失）；
/// 用户自己的第三方 --require 原样保留。
///
/// 路径一律写**正斜杠**形态：node 解析 NODE_OPTIONS 用类 POSIX 引号规则，
/// 反斜杠被当转义符吃掉（`C:\Users\...` → `C:Users...`），--require 找不到
/// 文件 → **机器上一切新 node 进程启动即崩**（2026-11-20 实测：反斜杠 rc=1、
/// 正斜杠 rc=0）。旧安装写下的反斜杠形态在下次 install 时被识别为待刷新值。
pub fn merge_node_options(existing: Option<&str>, js_hook: &Path) -> String {
    let hook_fs = js_hook.to_string_lossy().replace('\\', "/");
    let seg = format!("--require \"{hook_fs}\"");
    let Some(v) = existing else {
        return seg;
    };
    if v.contains(HOOK_MARKER) && v.contains(&hook_fs) {
        return v.to_string(); // 已配置且路径未变（正斜杠形态）
    }
    // 首次追加、路径变更或旧反斜杠形态：先摘掉我们的旧段
    // （strip 按文件名标记匹配，两种斜杠形态都能摘），再并上新段
    match strip_node_options(v) {
        Some(rest) => format!("{rest} {seg}"),
        None => seg,
    }
}

/// 从 NODE_OPTIONS 里摘掉我们的 --require 段：--require 与紧随其后的
/// 含钩子文件名的路径**成对**摘除——无差别删裸 --require 会把用户自己
/// 的第三方钩子留下悬空路径。摘完为空返回 None（可删整个值）。
pub fn strip_node_options(existing: &str) -> Option<String> {
    let toks: Vec<&str> = existing.split_whitespace().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if toks[i] == "--require" && toks.get(i + 1).is_some_and(|n| n.contains(HOOK_MARKER)) {
            i += 2; // 成对摘除我们的钩子段
        } else {
            kept.push(toks[i]);
            i += 1;
        }
    }
    (!kept.is_empty()).then(|| kept.join(" "))
}

/// 部署全部钩子。返回给人看的摘要文本（CLI 直接打印，GUI 放状态行）；
/// 逐解释器的失败细节以 `[hooks]` 行追加在摘要尾部（不影响其他解释器继续
/// 部署）——不走进程 stderr：GUI 子系统（window）没有 stderr 句柄，写失败
/// 会 panic（R16-H1），库代码也不该直接写进程流。
pub fn install() -> Result<String> {
    let hook_dir = imports::default_hook_dir().context("cannot determine data directory")?;
    let (_, js_hook) = write_hook_files(&hook_dir)?;
    let mut diagnostics = Vec::new();

    // python：每个解释器写一份 .pth；直接写 purelib 失败（如 Program Files
    // 需要管理员）回退到用户级 site（%APPDATA%\Python\PythonXXX\site-packages，
    // 对同版本解释器自动生效，除非显式 -s/-I 关闭 user site）
    let mut py_ok = 0usize;
    for interp in python_interpreters() {
        let Some(site) = py_purelib(&interp) else {
            continue;
        };
        let target = site.join(PTH_NAME);
        match std::fs::write(&target, pth_line(&hook_dir)) {
            Ok(()) => py_ok += 1,
            Err(_) => {
                // purelib 写入失败（如 Program Files 需管理员）→ 用户级回退：
                // %APPDATA%\Python\PythonXXX\site-packages 对同版本解释器
                // 自动生效（除非显式 -s/-I 关闭 user site）
                let Some(tag) = py_version_tag(&interp) else {
                    diagnostics.push(format!(
                        "[hooks] {interp:?}: cannot write .pth and version unknown"
                    ));
                    continue;
                };
                let user_site = roaming_root()
                    .join("Python")
                    .join(&tag)
                    .join("site-packages");
                std::fs::create_dir_all(&user_site).ok();
                match std::fs::write(user_site.join(PTH_NAME), pth_line(&hook_dir)) {
                    Ok(()) => py_ok += 1,
                    Err(e) => diagnostics.push(format!(
                        "[hooks] {interp:?}: cannot write .pth (direct and user-site failed): {e}"
                    )),
                }
            }
        }
    }

    // uv / pipx 工具 venv：独立 site-packages，.pth 不随基础解释器生效——
    // 工具内业务模块的 import 事件只有直接部署才看得到
    let mut venv_ok = 0usize;
    for site in tool_venv_sites() {
        if std::fs::write(site.join(PTH_NAME), pth_line(&hook_dir)).is_ok() {
            venv_ok += 1;
        }
    }

    // node：用户级 NODE_OPTIONS 追加 --require（保留既有值；重复 install 幂等）
    let existing = node_options_current();
    let merged = merge_node_options(existing.as_deref(), &js_hook);
    let changed = existing.as_deref() != Some(merged.as_str());
    if changed {
        let val = if merged.is_empty() {
            None
        } else {
            Some(merged.clone())
        };
        match val {
            Some(v) => {
                reg(&[
                    "add",
                    r"HKCU\Environment",
                    "/v",
                    "NODE_OPTIONS",
                    "/t",
                    "REG_SZ",
                    "/d",
                    &v,
                    "/f",
                ])?;
            }
            None => {
                let _ = reg(&["delete", r"HKCU\Environment", "/v", "NODE_OPTIONS", "/f"]);
            }
        }
    }

    let mut out = String::new();
    out.push_str("Hooks installed.\n");
    out.push_str(&format!("  python .pth : {py_ok} interpreter(s)\n"));
    out.push_str(&format!("  tool venvs  : {venv_ok} (uv/pipx)\n"));
    out.push_str(&format!(
        "  node        : {}",
        if changed {
            "NODE_OPTIONS updated (reopen terminals to take effect)"
        } else {
            "NODE_OPTIONS already configured"
        }
    ));
    if let Some(log) = imports::default_import_log_path() {
        out.push_str(&format!("\n  log         : {}", log.display()));
    }
    for d in &diagnostics {
        out.push('\n');
        out.push_str(d);
    }
    Ok(out)
}

/// 摘除全部钩子。返回摘要文本（同 install：失败细节以 `[hooks]` 行附在尾部）。
pub fn uninstall() -> Result<String> {
    let mut removed = 0usize;
    let mut diagnostics = Vec::new();
    for interp in python_interpreters() {
        for site in candidate_sites(&interp) {
            let pth = site.join(PTH_NAME);
            if pth.exists() {
                match std::fs::remove_file(&pth) {
                    Ok(()) => removed += 1,
                    Err(e) => {
                        diagnostics.push(format!("[hooks] cannot remove {}: {e}", pth.display()))
                    }
                }
            }
        }
    }
    for site in tool_venv_sites() {
        let pth = site.join(PTH_NAME);
        if pth.exists() {
            match std::fs::remove_file(&pth) {
                Ok(()) => removed += 1,
                Err(e) => diagnostics.push(format!("[hooks] cannot remove {}: {e}", pth.display())),
            }
        }
    }
    let mut node_cleaned = false;
    if let Some(v) = node_options_current() {
        if let Some(new_val) = strip_node_options(&v) {
            reg(&[
                "add",
                r"HKCU\Environment",
                "/v",
                "NODE_OPTIONS",
                "/t",
                "REG_SZ",
                "/d",
                &new_val,
                "/f",
            ])?;
            node_cleaned = true;
        } else {
            let _ = reg(&["delete", r"HKCU\Environment", "/v", "NODE_OPTIONS", "/f"]);
            node_cleaned = true;
        }
    }
    let mut out = String::new();
    out.push_str(&format!("Hooks uninstalled ({removed} .pth removed).\n"));
    out.push_str(&format!(
        "NODE_OPTIONS: {}",
        if node_cleaned {
            "our --require removed (third-party entries kept)"
        } else {
            "nothing to clean"
        }
    ));
    out.push_str("\nAlready-running terminals/interpreters keep the old environment.");
    Ok(out)
}

/// 钩子安装状态（GUI 面板与 CLI status 共用；不打印，结构化返回）。
pub fn status_info() -> HooksStatus {
    let interps = python_interpreters();
    let interpreters = interps
        .iter()
        .map(|i| {
            let has = candidate_sites(i).iter().any(|s| s.join(PTH_NAME).exists());
            (i.clone(), has)
        })
        .collect();
    let venvs = tool_venv_sites();
    let tool_venvs = (
        venvs.iter().filter(|s| s.join(PTH_NAME).exists()).count(),
        venvs.len(),
    );
    let node_hooked = node_options_current()
        .map(|v| v.contains(HOOK_MARKER))
        .unwrap_or(false);
    let log = imports::default_import_log_path().map(|p| {
        let state = match std::fs::metadata(&p) {
            Ok(md) => ImportLogState::Present(md.len()),
            Err(_) => {
                let shadow = imports::shadow_path(&p);
                if shadow.is_file() {
                    ImportLogState::Quarantined(shadow)
                } else {
                    ImportLogState::Missing
                }
            }
        };
        (p, state)
    });
    HooksStatus {
        interpreters,
        tool_venvs,
        node_hooked,
        log,
    }
}

/// 某解释器可能存在 .pth 的目录（purelib + 用户级 site 回退）。
fn candidate_sites(interpreter: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(pl) = py_purelib(interpreter) {
        out.push(pl);
    }
    if let Some(tag) = py_version_tag(interpreter) {
        out.push(
            roaming_root()
                .join("Python")
                .join(tag)
                .join("site-packages"),
        );
    }
    out
}

/// %APPDATA%（Roaming）根。dirs::data_dir() 即它本身。
fn roaming_root() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// uv / pipx 工具 venv 的 site-packages 列表：工具用独立解释器环境，
/// 基础解释器的 .pth 对它们不生效——import 钩子必须直接部署进去。
/// 环境变量优先（UV_TOOL_DIR / PIPX_HOME），目录不存在则静默跳过。
fn tool_venv_sites() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let uv_tools = std::env::var_os("UV_TOOL_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join("uv").join("tools")));
    if let Some(root) = uv_tools {
        push_child_site_packages(&mut out, &root);
    }
    let pipx_home = std::env::var_os("PIPX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|d| d.join("pipx")));
    if let Some(home) = pipx_home {
        push_child_site_packages(&mut out, &home.join("venvs"));
    }
    out
}

/// root 下每个工具目录的 Lib\site-packages（存在才收录；纯函数便于测试）。
pub fn push_child_site_packages(out: &mut Vec<PathBuf>, root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for e in entries.flatten() {
        let site = e.path().join("Lib").join("site-packages");
        if site.is_dir() {
            out.push(site);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reg_value_parsing_tolerates_locales() {
        let out = "\r\nHKEY_CURRENT_USER\\Environment\r\n    NODE_OPTIONS    REG_SZ    --require C:\\hook.cjs\r\n\r\n";
        assert_eq!(
            parse_reg_value(out).as_deref(),
            Some("--require C:\\hook.cjs")
        );
        assert_eq!(parse_reg_value("value not found"), None);
    }

    #[test]
    fn node_options_merge_is_idempotent_and_strip_restores() {
        let hook = std::path::Path::new(r"C:\data\orphan-audit\hooks\oa_node_hook.cjs");
        // 写入一律正斜杠：node 的 NODE_OPTIONS 解析器把 \ 当转义符吃掉，
        // 反斜杠形态会让一切新 node 进程启动即崩（2026-11-20 实测 rc=1）
        let seg = "--require \"C:/data/orphan-audit/hooks/oa_node_hook.cjs\"";
        // 无旧值 → 只有我们的段
        assert_eq!(merge_node_options(None, hook), seg);
        // 已配置（正斜杠形态）→ 幂等
        assert_eq!(merge_node_options(Some(seg), hook), seg);
        // 既有值保留
        let merged = merge_node_options(Some("--max-old-space-size=4096"), hook);
        assert!(merged.starts_with("--max-old-space-size=4096 --require"));
        // 摘除：只摘我们的段；摘空返回 None（注册表值可删）
        assert_eq!(
            strip_node_options(&merged).as_deref(),
            Some("--max-old-space-size=4096")
        );
        assert_eq!(strip_node_options(seg), None);
    }

    #[test]
    fn node_options_merge_rewrites_backslash_form() {
        // 旧版 install 写下的反斜杠形态（node 解析即崩）：即使路径本身没错，
        // 也不是「已配置」——下次 install 必须刷新成正斜杠形态（自愈）
        let hook = std::path::Path::new(r"C:\data\orphan-audit\hooks\oa_node_hook.cjs");
        let broken = "--require \"C:\\data\\orphan-audit\\hooks\\oa_node_hook.cjs\"";
        let merged = merge_node_options(Some(broken), hook);
        assert_eq!(
            merged,
            "--require \"C:/data/orphan-audit/hooks/oa_node_hook.cjs\""
        );
        // 反斜杠形态也能被 strip 摘除（按文件名标记匹配，斜杠形态无关）
        assert_eq!(strip_node_options(broken), None);
    }

    #[test]
    fn node_options_strip_keeps_third_party_require() {
        // 用户自己的第三方 --require 不受卸载影响（成对摘除，不删裸 --require）
        let hook = std::path::Path::new(r"C:\data\orphan-audit\hooks\oa_node_hook.cjs");
        let mixed = format!(
            "--require C:\\my\\hook.js --max-old-space-size=4096 --require \"{}\"",
            hook.display()
        );
        assert_eq!(
            strip_node_options(&mixed).as_deref(),
            Some("--require C:\\my\\hook.js --max-old-space-size=4096")
        );
        // 悬空保护：裸 --require 后面不是我们的钩子 → 原样保留
        assert_eq!(
            strip_node_options("--require").as_deref(),
            Some("--require")
        );
    }

    #[test]
    fn node_options_merge_refreshes_stale_path() {
        // 数据目录迁移后注册表里的旧路径要被刷新（否则 import 证据静默丢失）
        let new_hook = std::path::Path::new(r"C:\newdata\hooks\oa_node_hook.cjs");
        let stale = r#"--require "C:/olddata/hooks/oa_node_hook.cjs""#;
        let merged = merge_node_options(Some(stale), new_hook);
        assert_eq!(merged, "--require \"C:/newdata/hooks/oa_node_hook.cjs\"");
        assert!(!merged.contains("olddata"));
        // 第三方段保留
        let stale3 = r#"--max-old-space-size=4096 --require "C:/olddata/hooks/oa_node_hook.cjs""#;
        let merged3 = merge_node_options(Some(stale3), new_hook);
        assert!(merged3.starts_with("--max-old-space-size=4096 --require"));
    }

    #[test]
    fn tool_venv_sites_scans_uv_layout() {
        // 伪造 uv tools 布局：只有含 Lib\site-packages 的目录才收录
        let dir = std::env::temp_dir().join(format!("oa_venvs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let site = dir.join("toolA").join("Lib").join("site-packages");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(dir.join("empty-tool")).unwrap();
        let mut out = Vec::new();
        push_child_site_packages(&mut out, &dir);
        assert_eq!(out, vec![site]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

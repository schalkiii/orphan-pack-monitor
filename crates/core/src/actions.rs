//! 处置层：把判定结果变成可审阅的卸载脚本。
//! 安全铁律：只生成脚本，绝不自行删除。脚本里保留每行证据注释，
//! 让用户在真正执行前仍能反驳判定。
//! 平台：Windows 生成 PowerShell 脚本（本工具的目标平台），Unix 生成 bash。

use crate::model::{Manager, OrphanShim, Verdict, VerdictResult};
use std::fmt::Write as _;
use std::path::PathBuf;

/// 为单个包生成卸载命令。返回 None 表示该管理器不支持自动卸载。
pub fn uninstall_command(r: &VerdictResult) -> Option<String> {
    let name = &r.package.name;
    match r.package.manager {
        Manager::Cargo => Some(format!("cargo uninstall {name}")),
        Manager::Npm => {
            // 多 prefix 环境：非默认 prefix 里的包必须显式 --prefix，否则卸的是
            // 当前活动 prefix 的同名包（或报 not installed）
            let prefix = r
                .package
                .install_path
                .as_ref()
                .and_then(|p| npm_prefix_of(p));
            let is_default = prefix.as_ref().is_some_and(|pre| {
                crate::inventory::npm_default_prefix().is_some_and(|d| {
                    pre.to_string_lossy()
                        .eq_ignore_ascii_case(&d.to_string_lossy())
                })
            });
            match prefix {
                Some(pre) if !is_default => Some(format!(
                    "npm uninstall -g --prefix \"{}\" {name}",
                    pre.display()
                )),
                _ => Some(format!("npm uninstall -g {name}")),
            }
        }
        Manager::Uv => Some(format!("uv tool uninstall {name}")),
        Manager::Pipx => Some(format!("pipx uninstall {name}")),
        Manager::Pip => Some(pip_uninstall_cmd(r)),
        Manager::Go => r.package.install_path.as_ref().map(|p| {
            if cfg!(windows) {
                format!(
                    "Remove-Item -LiteralPath \"{}\"  # go install 无清单，请确认路径",
                    p.display()
                )
            } else {
                format!("rm -f \"{}\"  # go install 无清单，请确认路径", p.display())
            }
        }),
        Manager::Brew => Some(format!("brew uninstall {name}")),
        Manager::Scoop => Some(format!("scoop uninstall {name}")),
        Manager::Winget => Some(format!("winget uninstall --id {name}")),
    }
}

/// pip 卸载命令（多 python 环境下必须定位到包所在的环境，否则
/// `python -m pip` 只会作用于 PATH 上第一个解释器——卸错环境或报 not installed）：
/// 1. venv/系统布局：site-packages 伴随解释器，直接调用；
/// 2. 用户级 site-packages（`%APPDATA%\Python\Python313\site-packages`）：无伴随
///    解释器，但父目录名带版本标签，经 py 启动器按版本定位（`py -3.13`）；
/// 3. 推导不出：回退 PATH 上的 python。
fn pip_uninstall_cmd(r: &VerdictResult) -> String {
    let name = &r.package.name;
    if let Some(site) = r.package.install_path.as_deref() {
        if let Some(py) = python_for_site_packages(site) {
            return if cfg!(windows) {
                // PowerShell 的调用运算符（路径带空格时必须）
                format!("& \"{}\" -m pip uninstall -y {name}", py.display())
            } else {
                format!("\"{}\" -m pip uninstall -y {name}", py.display())
            };
        }
        if cfg!(windows) {
            if let Some(tag) = user_site_version_tag(site) {
                return format!("py -{tag} -m pip uninstall -y {name}");
            }
        }
    }
    format!("python -m pip uninstall -y {name}")
}

/// 用户级 site-packages 的版本标签：路径形如
/// `%APPDATA%\Python\Python313\site-packages`，取 site-packages 的父目录名
/// （Python313 → "3.13"）。非该布局返回 None。
fn user_site_version_tag(site: &std::path::Path) -> Option<String> {
    let parent = site.parent()?.file_name()?.to_string_lossy().into_owned();
    let rest = parent.strip_prefix("Python3")?;
    (rest.len() == 2 && rest.chars().all(|c| c.is_ascii_digit())).then(|| format!("3.{rest}"))
}

/// 从 npm 包的 install_path 反推其全局 prefix：沿目录向上找到名为
/// `node_modules` 的目录，其父即 prefix。scoped 包（`@scope\pkg`）比 plain
/// 包多一层目录，不能用固定的 parent 次数（曾因此把 node_modules 当成了 prefix）。
fn npm_prefix_of(install_path: &std::path::Path) -> Option<PathBuf> {
    let mut cur = install_path.parent()?;
    loop {
        if cur
            .file_name()?
            .to_string_lossy()
            .eq_ignore_ascii_case("node_modules")
        {
            return cur.parent().map(|p| p.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

/// 从 pip 包的 install_path（site-packages 目录）反推该环境的 python.exe：
/// 系统布局 `<root>\Lib\site-packages` → `<root>\python.exe`；
/// venv 布局 `<venv>\Lib\site-packages` → `<venv>\Scripts\python.exe`。
fn python_for_site_packages(site: &std::path::Path) -> Option<PathBuf> {
    let env_root = site.parent()?.parent()?;
    let system = env_root.join("python.exe");
    if system.is_file() {
        return Some(system);
    }
    let venv = env_root.join("Scripts").join("python.exe");
    venv.is_file().then_some(venv)
}

/// 残骸清理段：npm uninstall 半途失败留下的孤儿启动器逐文件删除行。
/// 段头说明来源与审查义务——与包卸载同一纪律：脚本只生成，删不删由用户审后定。
fn write_shim_section(out: &mut String, orphan_shims: &[OrphanShim]) {
    if orphan_shims.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "# {} orphan npm shim set(s) — launchers whose package is gone",
        orphan_shims.len()
    );
    let _ = writeln!(
        out,
        "# (half-failed uninstall leftovers; review each path before running)"
    );
    for s in orphan_shims {
        let _ = writeln!(out, "# {} — {}", s.bin, s.prefix.display());
        for f in &s.files {
            if cfg!(windows) {
                // 单引号 = PowerShell 字面量字符串（路径含 $ 也不展开）
                let _ = writeln!(out, "Remove-Item -LiteralPath '{}'", f.display());
            } else {
                let _ = writeln!(out, "rm -f \"{}\"", f.display());
            }
        }
        let _ = writeln!(out);
    }
}

/// 残骸逐文件删除命令（GUI「清理残骸」直接执行用）：与 clean 脚本同款形态
/// （Windows = PowerShell 单引号字面量，路径含 $ 也不展开），每文件一条按组
/// 序展平，执行方逐条跑。
pub fn shim_removal_commands(orphan_shims: &[OrphanShim]) -> Vec<String> {
    orphan_shims
        .iter()
        .flat_map(|s| {
            s.files.iter().map(|f| {
                if cfg!(windows) {
                    format!("Remove-Item -LiteralPath '{}'", f.display())
                } else {
                    format!("rm -f \"{}\"", f.display())
                }
            })
        })
        .collect()
}

/// 清理脚本默认导出路径（GUI「导出清理脚本」用）：桌面最顺手；取不到时
/// 回退数据目录、再回退当前目录。
pub fn default_script_export_path() -> std::path::PathBuf {
    dirs::desktop_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("orphan-audit-cleanup.ps1")
}

/// 把清理脚本写盘并带 UTF-8 BOM：PowerShell 5.1 对无 BOM 文件按 ANSI 解析，
/// 脚本里的中文路径（解释器 / 包目录）会乱码（CLI clean 同款处理）。
pub fn write_script_file(path: &std::path::Path, script: &str) -> std::io::Result<()> {
    let mut bom = "\u{feff}".to_string();
    bom.push_str(script);
    std::fs::write(path, bom)
}

/// 生成完整的清理脚本。Windows 生成 PowerShell（.ps1），Unix 生成 bash。
pub fn generate_script(
    results: &[VerdictResult],
    orphan_shims: &[OrphanShim],
    since_days: u32,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Generated by orphan-audit. Review, delete what you disagree with, then run."
    );
    let _ = writeln!(out, "# Nothing executes on its own. since={since_days}d");
    if cfg!(windows) {
        let _ = writeln!(
            out,
            "# PowerShell 脚本：审阅后以 powershell -File <name>.ps1 执行"
        );
        let _ = writeln!(out, "$ErrorActionPreference = 'Stop'");
    } else {
        let _ = writeln!(out, "#!/usr/bin/env bash");
        let _ = writeln!(out, "set -euo pipefail");
    }
    let _ = writeln!(out);

    // 清理候选 = unused 且**不在新装观察期**：装了还没来得及用的包，
    // 静默不足一个月不能当作弃用证据；被跳过的观察期包也要留痕
    let candidates: Vec<&VerdictResult> = results
        .iter()
        .filter(|r| r.verdict == Verdict::Unused && !r.newly_installed)
        .collect();
    let skipped: Vec<&VerdictResult> = results
        .iter()
        .filter(|r| r.verdict == Verdict::Unused && r.newly_installed)
        .collect();
    let skipped_note = |out: &mut String| {
        if !skipped.is_empty() {
            let _ = writeln!(
                out,
                "# {} newly installed package(s) skipped (observation window): {}",
                skipped.len(),
                skipped
                    .iter()
                    .map(|r| r.package.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    };

    if candidates.is_empty() {
        let _ = writeln!(out, "# No unused packages found. Nothing to remove.");
        skipped_note(&mut out);
        // 无包候选也输出残骸段：残骸与包卸载是两条独立的清理通道
        write_shim_section(&mut out, orphan_shims);
        return out;
    }

    let _ = writeln!(out, "# {} removal candidate(s)", candidates.len());
    skipped_note(&mut out);
    let _ = writeln!(out);
    for r in candidates {
        let evidence = if r.evidence.is_empty() {
            "no signal from any source".to_string()
        } else {
            r.evidence
                .iter()
                .map(|e| e.detail.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let _ = writeln!(
            out,
            "# {} ({}) — {evidence}",
            r.package.name, r.package.manager
        );
        if r.package.confidence == crate::model::Confidence::None {
            let _ = writeln!(
                out,
                "#   NOTE: binary name mapping is uncertain; verify before running."
            );
        }
        match uninstall_command(r) {
            Some(cmd) => {
                let _ = writeln!(out, "{cmd}");
            }
            None => {
                let _ = writeln!(
                    out,
                    "#   AUTO-UNINSTALL UNAVAILABLE — remove manually: {}",
                    r.package.name
                );
            }
        }
        let _ = writeln!(out);
    }
    write_shim_section(&mut out, orphan_shims);
    out
}

/// 生成人类可读的报告（终端友好的紧凑格式）。
pub fn generate_report(
    results: &[VerdictResult],
    orphan_shims: &[OrphanShim],
    since_days: u32,
) -> String {
    use std::collections::BTreeMap;
    let mut out = String::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for r in results {
        *counts.entry(r.verdict.as_str()).or_default() += 1;
    }
    let _ = writeln!(
        out,
        "orphan-audit — {} packages inspected (since={since_days}d)",
        results.len()
    );
    let _ = writeln!(out);
    for v in [
        Verdict::Active,
        Verdict::Indirect,
        Verdict::Rare,
        Verdict::Integration,
        Verdict::Unused,
    ] {
        let n = counts.get(v.as_str()).copied().unwrap_or(0);
        let _ = writeln!(out, "  {:<12} {:<4} {}", v.as_str(), n, v.description());
    }
    let _ = writeln!(out);

    for v in [Verdict::Unused, Verdict::Rare, Verdict::Integration] {
        let group: Vec<&VerdictResult> = results.iter().filter(|r| r.verdict == v).collect();
        if group.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{} — {}", v.as_str().to_uppercase(), v.description());
        for r in group {
            let last = r
                .last_exec_days_ago
                .map(|d| format!("last run {d}d ago"))
                .unwrap_or_else(|| "never run".to_string());
            let typed = if r.interactive_hits > 0 {
                format!("typed {}x", r.interactive_hits)
            } else {
                "never typed".to_string()
            };
            // 新装观察期标注：提示「没在用」可能只是「刚装还没用」
            let obs = if r.newly_installed {
                " · newly installed (observation)"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "  {:<24} {:<6} {typed} · {last}{obs}",
                r.package.name, r.package.manager
            );
            for e in &r.evidence {
                let _ = writeln!(out, "      [{}] {}", e.source.as_str(), e.detail);
            }
        }
        let _ = writeln!(out);
    }

    // 残骸不进五档（不是包），单列一节；完整文件路径在 clean 脚本里
    if !orphan_shims.is_empty() {
        let _ = writeln!(out, "ORPHAN SHIMS — npm launchers whose package is gone");
        let _ = writeln!(
            out,
            "  (half-failed uninstall leftovers; clean script has the file list)"
        );
        for s in orphan_shims {
            let _ = writeln!(
                out,
                "  {:<24} {} file(s) in {}",
                s.bin,
                s.files.len(),
                s.prefix.display()
            );
        }
        let _ = writeln!(out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_removal_commands_match_clean_script_form() {
        let shims = vec![OrphanShim {
            prefix: std::path::PathBuf::from(r"C:\u\AppData\Roaming\npm"),
            bin: "ocusage".into(),
            files: vec![
                std::path::PathBuf::from(r"C:\u\AppData\Roaming\npm\ocusage"),
                std::path::PathBuf::from(r"C:\u\AppData\Roaming\npm\ocusage.cmd"),
            ],
        }];
        assert_eq!(
            shim_removal_commands(&shims),
            vec![
                r"Remove-Item -LiteralPath 'C:\u\AppData\Roaming\npm\ocusage'",
                r"Remove-Item -LiteralPath 'C:\u\AppData\Roaming\npm\ocusage.cmd'",
            ]
        );
        assert!(shim_removal_commands(&[]).is_empty());
    }

    #[test]
    fn write_script_file_adds_utf8_bom() {
        let dir = std::env::temp_dir().join(format!("oa_bom_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cleanup.ps1");
        write_script_file(&p, "# 清理脚本\nnpm uninstall -g x").unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "PS 5.1 需要 BOM");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn result(name: &str, manager: Manager, verdict: Verdict) -> VerdictResult {
        let mut p = crate::model::Package::new(name, manager);
        p.binaries = vec![name.to_string()];
        VerdictResult {
            package: p,
            verdict,
            interactive_hits: 0,
            last_exec_days_ago: None,
            in_shell_config: false,
            newly_installed: false,
            evidence: vec![],
        }
    }

    #[test]
    fn script_contains_only_unused_with_evidence() {
        let results = vec![
            result("dusty", Manager::Cargo, Verdict::Unused),
            result("ripgrep", Manager::Cargo, Verdict::Active),
        ];
        let s = generate_script(&results, &[], 30);
        assert!(s.contains("cargo uninstall dusty"));
        assert!(!s.contains("cargo uninstall ripgrep"));
        assert!(s.contains("# 1 removal candidate(s)"));
        assert!(s.contains("no signal from any source"));
    }

    #[test]
    fn newly_installed_packages_are_skipped_in_script() {
        let mut fresh = result("fresh-tool", Manager::Cargo, Verdict::Unused);
        fresh.newly_installed = true;
        let results = vec![fresh, result("dusty", Manager::Cargo, Verdict::Unused)];
        let s = generate_script(&results, &[], 30);
        // 观察期包不进清理命令，但要在说明里留痕
        assert!(!s.contains("cargo uninstall fresh-tool"));
        assert!(s.contains("cargo uninstall dusty"));
        assert!(s.contains("newly installed package(s) skipped"));
        assert!(s.contains("fresh-tool"));

        // 全部是观察期：无候选，但说明仍列出
        let only_fresh = generate_script(
            &[VerdictResult {
                newly_installed: true,
                ..result("fresh-tool", Manager::Cargo, Verdict::Unused)
            }],
            &[],
            30,
        );
        assert!(!only_fresh.contains("cargo uninstall"));
        assert!(only_fresh.contains("observation window"));
    }

    #[test]
    fn uninstall_commands_per_manager() {
        assert_eq!(
            uninstall_command(&result("x", Manager::Npm, Verdict::Unused)).as_deref(),
            Some("npm uninstall -g x")
        );
        assert_eq!(
            uninstall_command(&result("x", Manager::Uv, Verdict::Unused)).as_deref(),
            Some("uv tool uninstall x")
        );
        assert_eq!(
            uninstall_command(&result("x", Manager::Pipx, Verdict::Unused)).as_deref(),
            Some("pipx uninstall x")
        );
    }

    #[test]
    fn script_matches_platform_shell() {
        let s = generate_script(&[result("dusty", Manager::Cargo, Verdict::Unused)], &[], 30);
        if cfg!(windows) {
            // Windows 生成 PowerShell：不能是 bash 头（曾长期生成 bash 脚本，
            // 与目标平台不符）
            assert!(s.contains("$ErrorActionPreference = 'Stop'"));
            assert!(!s.contains("#!/usr/bin/env bash"));
            assert!(!s.contains("set -euo pipefail"));
        } else {
            assert!(s.contains("#!/usr/bin/env bash"));
            assert!(s.contains("set -euo pipefail"));
        }
    }

    #[test]
    fn pip_uninstall_targets_the_owning_interpreter() {
        // temp 目录构造真实布局，验证 site-packages → 解释器推导
        let dir = std::env::temp_dir().join(format!("oa_pip_uninst_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // venv 布局：<root>\Scripts\python.exe
        std::fs::create_dir_all(dir.join("py311").join("Scripts")).unwrap();
        std::fs::write(dir.join("py311").join("Scripts").join("python.exe"), "").unwrap();
        let mut r = result("httpx", Manager::Pip, Verdict::Unused);
        r.package.install_path = Some(dir.join("py311").join("Lib").join("site-packages"));
        let cmd = uninstall_command(&r).unwrap();
        if cfg!(windows) {
            assert!(
                cmd.contains("& \"")
                    && cmd.contains("Scripts\\python.exe\" -m pip uninstall -y httpx"),
                "{cmd}"
            );
        } else {
            assert!(cmd.contains("Scripts/python.exe\" -m pip"), "{cmd}");
        }
        // 系统布局优先：<root>\python.exe 直接在根上
        std::fs::write(dir.join("py311").join("python.exe"), "").unwrap();
        let cmd = uninstall_command(&r).unwrap();
        assert!(
            cmd.contains("py311\\python.exe") && !cmd.contains("Scripts"),
            "system layout wins: {cmd}"
        );
        // 推导不出（install_path 缺失）→ 回退 PATH python
        assert_eq!(
            uninstall_command(&result("x", Manager::Pip, Verdict::Unused)).as_deref(),
            Some("python -m pip uninstall -y x")
        );
        // 用户级 site-packages：无伴随解释器，按父目录版本标签经 py 启动器定位
        // （2026-11-19 实测 PyMuPDF 装在 %APPDATA%\Python\Python313\site-packages）
        let mut user_site = result("PyMuPDF", Manager::Pip, Verdict::Unused);
        user_site.package.install_path = Some(PathBuf::from(
            r"C:\Users\u\AppData\Roaming\Python\Python313\site-packages",
        ));
        if cfg!(windows) {
            assert_eq!(
                uninstall_command(&user_site).as_deref(),
                Some("py -3.13 -m pip uninstall -y PyMuPDF")
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn script_and_report_cover_orphan_shims() {
        // 残骸三件套 + IPGSD 伴生：脚本逐文件 Remove-Item（PowerShell 单引号
        // 字面量形态），报告单列一节
        let shims = vec![OrphanShim {
            prefix: PathBuf::from(r"C:\Users\u\AppData\Roaming\npm"),
            bin: "opencli".into(),
            files: vec![
                PathBuf::from(r"C:\Users\u\AppData\Roaming\npm\opencli"),
                PathBuf::from(r"C:\Users\u\AppData\Roaming\npm\opencli.cmd"),
                PathBuf::from(r"C:\Users\u\AppData\Roaming\npm\opencli.cmd.IPGSD"),
            ],
        }];
        let with_candidates = generate_script(
            &[result("dusty", Manager::Cargo, Verdict::Unused)],
            &shims,
            30,
        );
        assert!(with_candidates.contains("orphan npm shim set(s)"));
        assert!(with_candidates.contains(
            "Remove-Item -LiteralPath 'C:\\Users\\u\\AppData\\Roaming\\npm\\opencli.cmd'"
        ));
        assert!(with_candidates.contains("opencli.cmd.IPGSD"));
        assert!(with_candidates.contains("cargo uninstall dusty"));
        // 无包候选（全部观察期/无 unused）时残骸段仍要输出：
        // 残骸与包卸载是两条独立的清理通道
        let shim_only = generate_script(&[], &shims, 30);
        assert!(shim_only.contains("Remove-Item"));
        assert!(!shim_only.contains("cargo uninstall"));
        // 报告节：计数 + prefix，完整文件路径留给脚本
        let report = generate_report(&[], &shims, 30);
        assert!(report.contains("ORPHAN SHIMS"));
        assert!(report.contains("opencli"));
        assert!(report.contains("3 file(s)"));
        // 无残骸：两处都不出现该节
        assert!(!generate_report(&[], &[], 30).contains("ORPHAN SHIMS"));
        assert!(!generate_script(&[], &[], 30).contains("Remove-Item"));
    }

    #[test]
    fn npm_uninstall_uses_prefix_for_non_default_prefix() {
        // install_path 指向非默认 prefix（如 QClaw 的独立 npm-global）时必须
        // 显式 --prefix，否则卸的是当前活动 prefix 里的同名包。
        // scoped 包多一层 @scope 目录——prefix 推导曾在此把 node_modules
        // 本身当成 prefix（沿目录向上找 node_modules 的父目录才是正解）
        let mut r = result("@qclaw/foo", Manager::Npm, Verdict::Unused);
        r.package.install_path = Some(PathBuf::from(
            r"C:\Users\u\AppData\Roaming\QClaw\npm-global\node_modules\@qclaw\foo",
        ));
        let default_is_qclaw = crate::inventory::npm_default_prefix()
            .is_some_and(|d| d.to_string_lossy().contains("QClaw"));
        if !default_is_qclaw {
            let cmd = uninstall_command(&r).unwrap();
            assert_eq!(
                cmd,
                "npm uninstall -g --prefix \"C:\\Users\\u\\AppData\\Roaming\\QClaw\\npm-global\" @qclaw/foo"
            );
        }
        // plain 包（无 @scope 层）同样推到 prefix 而非 node_modules
        let mut plain = result("foo", Manager::Npm, Verdict::Unused);
        plain.package.install_path = Some(PathBuf::from(
            r"C:\Users\u\AppData\Roaming\QClaw\npm-global\node_modules\foo",
        ));
        if !default_is_qclaw {
            let cmd = uninstall_command(&plain).unwrap();
            assert!(
                cmd.contains("--prefix \"C:\\Users\\u\\AppData\\Roaming\\QClaw\\npm-global\""),
                "{cmd}"
            );
            assert!(!cmd.contains("node_modules"), "{cmd}");
        }
    }
}

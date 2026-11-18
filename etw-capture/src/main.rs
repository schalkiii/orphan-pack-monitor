//! orphan-audit ETW 采集端（S17）：订阅 Microsoft-Windows-Kernel-Process 的进程创建事件，
//! 追加到 `core::etw::default_log_path()` 指向的 events.jsonl（与消费端共用单一契约）。
//! 设计说明见同目录 DESIGN.md。
//!
//! 运行形态：
//! - `etw-capture.exe`            前台采集（Ctrl+C 停止）
//! - `etw-capture.exe install`    部署到数据目录并注册登录自启计划任务（需管理员）
//! - `etw-capture.exe uninstall`  删除计划任务并停掉采集会话（需管理员）
//! - `etw-capture.exe status`     查看任务/进程/日志状态

use anyhow::{anyhow, bail, Context, Result};
use ferrisetw::parser::Parser;
use ferrisetw::provider::Provider;
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::{stop_trace_by_name, TraceTrait, UserTrace};
use ferrisetw::EventRecord;
use orphan_audit_core::etw;
use orphan_audit_core::inventory::strip_exe;
use orphan_audit_core::model::basename;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// ETW 会话名固定；异常退出残留的会话由下次启动先停后建（幂等）。
const TRACE_NAME: &str = "orphan-audit-etw";
/// 登录自启计划任务名（install/uninstall/status 共用）。
const TASK_NAME: &str = "orphan-audit-etw-capture";
/// Kernel-Process 的进程创建事件 ID（2=退出、15/16=rundown，均只用于维护父名缓存）。
const EVENT_PROCESS_START: u16 = 1;
/// Microsoft-Windows-Kernel-Process（`logman query providers` 实测 GUID）。
const KERNEL_PROCESS_GUID: &str = "22fb2cd6-0e7b-422b-a0c7-2fad1fd0e716";
/// 启用关键词：进程事件(0x10) + rundown(0x8000)——后者让会话建立时内核先下发
/// 现存进程快照，父名缓存立即预热（用户早已开着的 shell 也能归因）。
const KEYWORD_PROCESS_OR_RUNDOWN: u64 = 0x8010;

fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("install") => return install(),
        Some("uninstall") => return uninstall(),
        Some("status") => return status(),
        Some(other) => {
            bail!("unknown subcommand: {other}\nusage: etw-capture.exe [install|uninstall|status]")
        }
        None => {}
    }
    run_capture()
}

/// 部署位置：数据目录（与 events.jsonl 同根，由 core 的日志路径反推，避免重复约定）。
fn deployed_exe() -> Option<PathBuf> {
    etw::default_log_path()?
        .parent()?
        .parent()?
        .join("etw-capture.exe")
        .into()
}

/// 采集进程（部署名 etw-capture.exe）是否在跑。status 展示与 install 升级停机复用。
fn capture_running() -> bool {
    run_hidden("tasklist", &["/FI", "IMAGENAME eq etw-capture.exe", "/NH"])
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout)
                    .to_ascii_lowercase()
                    .contains("etw-capture.exe")
        })
        .unwrap_or(false)
}

/// 注册计划任务：复制自身到数据目录 + schtasks ONLOGON/HIGHEST + 立即启动一次。
/// 已知取舍：控制台程序随任务启动会显示一个控制台窗口（见 DESIGN.md）。
fn install() -> Result<()> {
    if !etw::is_elevated() {
        bail!("install requires administrator privileges; run from an elevated terminal");
    }
    let dest = deployed_exe().context("cannot determine data directory")?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let src = std::env::current_exe().context("locating current executable")?;
    // 同路径重装（已部署的 exe 上再跑 install）：跳过自我复制（Windows 下会报错）
    if std::fs::canonicalize(&src).ok() != std::fs::canonicalize(&dest).ok() {
        // 升级场景：部署目标正在运行——Windows 锁定运行中的 exe，必须先停再拷。
        // 先优雅停会话（进程随会话退出），超时再强制结束任务
        if capture_running() {
            println!("Stopping running capture before update...");
            let _ = stop_trace_by_name(TRACE_NAME);
            for _ in 0..15 {
                if !capture_running() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if capture_running() {
                let _ = run_hidden("schtasks", &["/End", "/TN", TASK_NAME]);
                for _ in 0..15 {
                    if !capture_running() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
            if capture_running() {
                bail!(
                    "deployed capture is still running; end it manually \
                     (taskkill /IM etw-capture.exe) and retry"
                );
            }
        }
        std::fs::copy(&src, &dest).with_context(|| format!("copying to {}", dest.display()))?;
    }
    let tr = format!("\"{}\"", dest.display());
    let out = run_hidden(
        "schtasks",
        &[
            "/Create", "/F", "/TN", TASK_NAME, "/TR", &tr, "/SC", "ONLOGON", "/RL", "HIGHEST",
        ],
    )
    .context("running schtasks /Create")?;
    if !out.status.success() {
        bail!(
            "schtasks /Create failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // schtasks 创建的任务默认「电池模式停止 + 72 小时执行时限」——两者都会杀死
    // 常驻采集（实测 2026-11-18：笔记本切电池即被调度器终止）。schtasks 没有
    // 对应开关，用 PowerShell 把设置改为「电池可启停容忍 + 无时限」
    let patch = format!(
        "Set-ScheduledTask -TaskName '{TASK_NAME}' -Settings \
         (New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries \
         -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero))"
    );
    // 补丁失败不阻断 install（任务仍可用默认设置运行），但必须把退化暴露出来
    match run_hidden("powershell", &["-NoProfile", "-Command", &patch]) {
        Ok(o) if o.status.success() => {}
        other => eprintln!(
            "[etw-capture] task settings patch failed (battery/72h defaults remain): {other:?}"
        ),
    }
    // 注册后立即启动一次，不用等下次登录才生效
    let _ = run_hidden("schtasks", &["/Run", "/TN", TASK_NAME]);
    println!("Scheduled task '{TASK_NAME}' installed (runs at logon, HIGHEST privileges).");
    println!("Executable: {}", dest.display());
    if let Some(log) = etw::default_log_path() {
        println!("Log: {}", log.display());
    }
    println!("NOTE: a console window appears at each logon (documented tradeoff; see DESIGN.md).");
    Ok(())
}

/// 删除计划任务并停掉采集会话；运行中的采集进程会随会话停止自行退出。
fn uninstall() -> Result<()> {
    if !etw::is_elevated() {
        bail!("uninstall requires administrator privileges; run from an elevated terminal");
    }
    // 先停会话：采集端的事件线程返回时会置停机标志，进程随之退出
    let _ = stop_trace_by_name(TRACE_NAME);
    let out = run_hidden("schtasks", &["/Delete", "/F", "/TN", TASK_NAME])
        .context("running schtasks /Delete")?;
    if out.status.success() {
        println!("Scheduled task '{TASK_NAME}' removed.");
    } else {
        println!("Scheduled task '{TASK_NAME}' not found (nothing to remove).");
    }
    if let Some(log) = etw::default_log_path() {
        println!("Log kept at {}", log.display());
    }
    Ok(())
}

/// 查看常驻化状态：计划任务 / 采集进程 / 事件日志。
fn status() -> Result<()> {
    let installed = run_hidden("schtasks", &["/Query", "/TN", TASK_NAME])
        .map(|o| o.status.success())
        .unwrap_or(false);
    println!(
        "Scheduled task : {}",
        if installed {
            "installed (logon autostart)"
        } else {
            "not installed"
        }
    );
    let running = capture_running();
    println!(
        "Capture process: {}",
        if running { "running" } else { "not running" }
    );
    match etw::default_log_path() {
        Some(log) => match std::fs::metadata(&log) {
            Ok(md) => {
                let modified = md
                    .modified()
                    .map(|t| format!("{t:?}"))
                    .unwrap_or_else(|_| "unknown".into());
                println!(
                    "Event log      : {} ({} bytes, modified {modified})",
                    log.display(),
                    md.len()
                );
            }
            Err(_) => println!("Event log      : {} (not created yet)", log.display()),
        },
        None => println!("Event log      : cannot determine path"),
    }
    Ok(())
}

/// 无窗口运行系统命令并收集输出（CREATE_NO_WINDOW；被 GUI/计划任务拉起时不弹窗）。
fn run_hidden(program: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .creation_flags(orphan_audit_core::inventory::CREATE_NO_WINDOW);
    cmd.output()
}

/// 单文件日志上限：超过则在启动时轮转。64MB 足够覆盖审计窗口回看
/// （消费端 core::etw::read_etw_log_since 只取 since 窗口内事件）。
const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;

/// 日志超过 max 时轮转：`<log>` 改名为 `<log>.1`（旧 .1 覆盖），随后正常新建追加。
/// 轮转失败（如句柄被占）只警告不中断——继续追加原文件。
fn rotate_log_if_huge(log: &std::path::Path, max: u64) {
    let Ok(md) = std::fs::metadata(log) else {
        return;
    };
    if md.len() < max {
        return;
    }
    let rotated = log.with_extension("jsonl.1");
    let _ = std::fs::remove_file(&rotated);
    match std::fs::rename(log, &rotated) {
        Ok(()) => println!(
            "Log rotated: {} -> {} ({} bytes)",
            log.display(),
            rotated.display(),
            md.len()
        ),
        Err(e) => eprintln!("[etw-capture] log rotation failed: {e}"),
    }
}

/// 前台采集（默认形态；计划任务启动的也是这个形态）。
fn run_capture() -> Result<()> {
    if !etw::is_elevated() {
        bail!("orphan-audit-etw-capture requires administrator privileges; run from an elevated terminal");
    }
    let Some(log) = etw::default_log_path() else {
        bail!("cannot determine data directory for events.jsonl");
    };
    rotate_log_if_huge(&log, MAX_LOG_BYTES);
    // 停掉上次异常退出可能残留的会话（失败说明本来就没有，忽略）
    let _ = stop_trace_by_name(TRACE_NAME);

    println!("Capturing process starts (Microsoft-Windows-Kernel-Process)");
    println!("Log: {}", log.display());
    println!("Press Ctrl+C to stop.");

    // PID -> ImageName 缓存：事件只带 ParentProcessID（数字），父进程名从本会话
    // 见过的进程解析（start/stop/rundown 事件都会刷新缓存）。
    let parent_names: Arc<Mutex<HashMap<u32, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let log: Arc<PathBuf> = Arc::new(log);

    let cb_log = Arc::clone(&log);
    let cb_parents = Arc::clone(&parent_names);
    let callback = move |record: &EventRecord, locator: &SchemaLocator| {
        let Ok(schema) = locator.event_schema(record) else {
            return;
        };
        let parser = Parser::create(record, &schema);
        // 主体 PID 优先取事件属性，缺失时回退 header（Kernel-Process 两者一致）
        let pid = parser
            .try_parse::<u32>("ProcessID")
            .unwrap_or_else(|_| record.process_id());
        // 任意带 ImageName 的事件都维护缓存；rundown 让缓存覆盖采集前已启动的进程
        if let Ok(image) = parser.try_parse::<String>("ImageName") {
            if let Ok(mut cache) = cb_parents.lock() {
                cache.insert(pid, image);
            }
        }
        if record.event_id() != EVENT_PROCESS_START {
            return;
        }
        let Ok(image_name) = parser.try_parse::<String>("ImageName") else {
            return; // 无镜像名的创建事件（如 System Idle Process）跳过
        };
        // 实测（Win11 build 28000）：Kernel-Process event 1 的 schema 已无 CommandLine
        // 属性（TDH NotFound），cmd 恒为 null；契约允许缺省，归因用 binary+parent（见 DESIGN.md）。
        let command_line = parser.try_parse::<String>("CommandLine").ok();
        let parent = parser
            .try_parse::<u32>("ParentProcessID")
            .ok()
            .and_then(|ppid| {
                let cache = cb_parents.lock().ok()?;
                cache.get(&ppid).map(|img| basename(img).to_string())
            });
        let binary = strip_exe(&image_name).to_string();
        if let Err(e) =
            etw::append_etw_event(&cb_log, &binary, parent.as_deref(), command_line.as_deref())
        {
            eprintln!("[etw-capture] append failed: {e:#}");
        }
    };

    let provider = Provider::by_guid(KERNEL_PROCESS_GUID)
        .any(KEYWORD_PROCESS_OR_RUNDOWN)
        .add_callback(callback)
        .build();
    // start() 返回 (UserTrace, PROCESSTRACE_HANDLE=u64)；TraceHandle 别名在 crate 外不可见，
    // 且 TraceError 未实现 std::error::Error，不能用 anyhow::Context，须 map_err。
    let (trace, handle) = UserTrace::new()
        .named(TRACE_NAME.to_string())
        .enable(provider)
        .start()
        .map_err(|e| {
            anyhow!("failed to start ETW session (another session with the same name?): {e:?}")
        })?;

    // 事件消费是阻塞调用，放后台线程。Ctrl+C 和事件线程本身都能触发停机：
    // 会话被外部停止（如 uninstall 先停会话）时 process_from_handle 返回，
    // 主循环也必须结束，不能只在 Err 分支处理
    let running = Arc::new(AtomicBool::new(true));
    let done = Arc::clone(&running);
    std::thread::spawn(move || {
        if let Err(e) = UserTrace::process_from_handle(handle) {
            eprintln!("[etw-capture] event processing ended: {e:?}");
        }
        done.store(false, Ordering::Release);
    });

    let flag = Arc::clone(&running);
    ctrlc::set_handler(move || flag.store(false, Ordering::Release))
        .context("installing Ctrl+C handler")?;
    while running.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(200));
    }
    if let Err(e) = trace.stop() {
        // 会话可能已被外部停止（uninstall 路径），此时视为正常退出
        eprintln!("[etw-capture] session stop: {e:?} (stopped externally?)");
    }
    println!("Session stopped. Captured events are kept in the log above.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_log_if_huge_rotates_only_over_limit() {
        let dir = std::env::temp_dir().join(format!("oa_rot_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("events.jsonl");
        std::fs::write(&p, "abc").unwrap();
        // 小于阈值：不动
        rotate_log_if_huge(&p, 1024);
        assert!(p.exists());
        assert!(!dir.join("events.jsonl.1").exists());
        // 超过阈值：轮转为 .1，且旧 .1 被覆盖
        std::fs::write(dir.join("events.jsonl.1"), "stale").unwrap();
        rotate_log_if_huge(&p, 1);
        assert!(!p.exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("events.jsonl.1")).unwrap(),
            "abc"
        );
        // 不存在的文件：静默返回
        rotate_log_if_huge(&dir.join("nope.jsonl"), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

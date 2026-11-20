// Release 构建为 GUI 子系统：计划任务/登录拉起不再弹控制台窗口（debug 保持
// 控制台子系统，cargo run 的开发体验不变）。交互输出的衔接见 attach_console_if_available。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! orphan-audit ETW 采集端（S17）：订阅 Microsoft-Windows-Kernel-Process 的进程创建事件，
//! 追加到 `core::etw::default_log_path()` 指向的 events.jsonl（与消费端共用单一契约）。
//! 设计说明见同目录 DESIGN.md。
//!
//! 运行形态：
//! - `etw-capture.exe`            前台采集（Ctrl+C 停止）
//! - `etw-capture.exe install`    部署到数据目录并注册登录自启计划任务（需管理员）；同时启用 4688 命令行审计
//!   （Process Creation + ProcessCreationIncludeCmdLine_Enabled），常驻轮询 Security 日志把 node/python 进程的**命令行**并入日志
//! - `etw-capture.exe uninstall`  删除计划任务并停掉采集会话（需管理员）
//! - `etw-capture.exe status`     查看任务/进程/日志状态
//! - `etw-capture.exe deep`       深度模式（opt-in spike）：Kernel-File 订阅 site-packages / node_modules 下的文件打开事件，写入 import 事件日志（import 使用证据，噪音大）

use anyhow::{anyhow, bail, Context, Result};
use ferrisetw::parser::Parser;
use ferrisetw::provider::{kernel_providers, Provider};
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::{stop_trace_by_name, KernelTrace, TraceTrait, UserTrace};
use ferrisetw::EventRecord;
use orphan_audit_core::etw;
use orphan_audit_core::inventory::strip_exe;
use orphan_audit_core::model::basename;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// ETW 会话名固定；异常退出残留的会话由下次启动先停后建（幂等）。
const TRACE_NAME: &str = "orphan-audit-etw";
/// deep 模式独立的内核会话名（与常驻采集互不影响）。
const DEEP_TRACE_NAME: &str = "orphan-audit-deep";
/// 登录自启计划任务名（install/uninstall/status 共用）。
const TASK_NAME: &str = "orphan-audit-etw-capture";
/// Kernel-Process 的进程创建事件 ID（2=退出、15/16=rundown，均只用于维护父名缓存）。
const EVENT_PROCESS_START: u16 = 1;
/// Microsoft-Windows-Kernel-Process（`logman query providers` 实测 GUID）。
const KERNEL_PROCESS_GUID: &str = "22fb2cd6-0e7b-422b-a0c7-2fad1fd0e716";
/// 启用关键词：进程事件(0x10) + rundown(0x8000)——后者让会话建立时内核先下发
/// 现存进程快照，父名缓存立即预热（用户早已开着的 shell 也能归因）。
const KEYWORD_PROCESS_OR_RUNDOWN: u64 = 0x8010;
/// 4688 轮询间隔（Security 日志查询；wevtutil 每次拉增量，EventRecordID 断点续传）。
const POLL_4688_SECS: u64 = 5;
/// 视为「解释器」的进程镜像名——4688 只对这类进程写事件：它们的 binary 名
/// 对包归因无意义（node/python 不对应任何包），必须靠命令行路径归因；其余 exe
/// 的镜像名本身就是命令（Kernel-Process 已覆盖），写两遍只会双计。
const INTERPRETER_IMAGES: &[&str] = &["node.exe", "python.exe", "pythonw.exe"];

fn main() -> Result<()> {
    attach_console_if_available();
    match std::env::args().nth(1).as_deref() {
        Some("install") => return install(),
        Some("uninstall") => return uninstall(),
        Some("status") => return status(),
        Some("deep") => return run_deep(),
        Some(other) => {
            bail!("unknown subcommand: {other}\nusage: etw-capture.exe [install|uninstall|status|deep]")
        }
        None => {}
    }
    run_capture()
}

/// GUI 子系统（release）下衔接交互输出：AttachConsole 挂到父进程的控制台，
/// **只补空缺的 std 句柄**。各场景归宿——
/// - 终端直接运行子命令（内核不给 GUI 进程配 std 句柄）→ 挂接后补上 CONOUT$，println 可见；
/// - std 句柄已被父进程布置（管道 / 重定向文件：cargo test、bash 捕获、`> out.txt`）
///   → 句柄有效不抢，输出照原路流向管道/文件（无条件重定向会把输出吞进隐藏控制台，
///   本机 git-bash「隐藏控制台 + 管道」场景实测吞掉过 install 的全部输出）；
/// - 无控制台无管道（计划任务拉起守护）→ 挂接失败，诊断走 capture.log
///   （Rust std 对空句柄的写入是静默丢弃，不会 panic）。
#[cfg(all(windows, not(debug_assertions)))]
fn attach_console_if_available() {
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return; // 无父控制台（管道/守护场景），或本就有控制台（debug 构建）
        }
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        let conout = CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_WRITE,
            std::ptr::null::<SECURITY_ATTRIBUTES>(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if conout == INVALID_HANDLE_VALUE {
            return;
        }
        for (slot, current) in [
            (STD_OUTPUT_HANDLE, GetStdHandle(STD_OUTPUT_HANDLE)),
            (STD_ERROR_HANDLE, GetStdHandle(STD_ERROR_HANDLE)),
        ] {
            if current.is_null() || current == INVALID_HANDLE_VALUE {
                SetStdHandle(slot, conout);
            }
        }
    }
}

/// debug 构建是控制台子系统，输出天然可用。
#[cfg(any(not(windows), debug_assertions))]
fn attach_console_if_available() {}

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
/// release 构建为 GUI 子系统，登录拉起不弹控制台窗口（守护诊断落 capture.log）。
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
    // 4688 命令行审计（幂等）：安全日志的进程创建事件已含完整命令行后，
    // 常驻轮询能把 node/python 的脚本路径归因到包（ETW 内核事件已无命令行字段）
    enable_4688_cmdline();
    println!("Scheduled task '{TASK_NAME}' installed (runs at logon, HIGHEST privileges).");
    println!("Executable: {}", dest.display());
    if let Some(log) = etw::default_log_path() {
        println!("Log: {}", log.display());
    }
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

/// 守护侧诊断：GUI 子系统 + 计划任务拉起时无控制台，这是诊断的唯一去处。
/// 一行一条时间戳记录，写 `<数据目录>/etw/capture.log`（与 events.jsonl 同目录）。
/// 交互运行时诊断同时上 stderr（终端立即可见）。
fn log_diag(msg: &str) {
    use std::io::Write;
    let Some(p) = diag_log_path() else {
        return;
    };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    {
        let _ = writeln!(f, "[{}] {msg}", chrono::Utc::now().to_rfc3339());
    }
}

/// 诊断日志路径：默认 `<数据目录>/etw/capture.log`（与 events.jsonl 同目录）；
/// `OA_CAPTURE_LOG` 可重定向（单测隔离生产日志用，先例：import 日志的 OA_IMPORT_LOG）。
fn diag_log_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OA_CAPTURE_LOG") {
        return Some(PathBuf::from(p));
    }
    etw::default_log_path().map(|mut p| {
        p.set_file_name("capture.log");
        p
    })
}

/// 守护侧诊断的标准出口：stderr（有控制台时可见）+ capture.log（永远落盘）。
fn diag(msg: &str) {
    eprintln!("[etw-capture] {msg}");
    log_diag(msg);
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
        Ok(()) => {
            println!(
                "Log rotated: {} -> {} ({} bytes)",
                log.display(),
                rotated.display(),
                md.len()
            );
            log_diag(&format!(
                "log rotated to {} ({} bytes)",
                rotated.display(),
                md.len()
            ));
        }
        Err(e) => diag(&format!("log rotation failed: {e}")),
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
    // 守护形态（计划任务拉起）没有控制台，上面三行没人看见——启动事实落诊断日志
    log_diag(&format!(
        "capture session started (pid {})",
        std::process::id()
    ));

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
            diag(&format!("append failed: {e:#}"));
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
            diag(&format!("event processing ended: {e:?}"));
        }
        done.store(false, Ordering::Release);
    });

    let flag = Arc::clone(&running);
    ctrlc::set_handler(move || flag.store(false, Ordering::Release))
        .context("installing Ctrl+C handler")?;

    // 4688 轮询线程：Security 日志的进程创建事件（含命令行）并入同一份日志。
    // 只写解释器类进程（node/python）——它们的镜像名归因不了包，命令行才能；
    // 其余 exe 由 Kernel-Process 事件按镜像名归因，重复写会双计。
    let poll_log = Arc::clone(&log);
    let poll_running = Arc::clone(&running);
    std::thread::spawn(move || poll_4688_loop(&poll_log, &poll_running));

    while running.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(200));
    }
    if let Err(e) = trace.stop() {
        // 会话可能已被外部停止（uninstall 路径），此时视为正常退出
        diag(&format!("session stop: {e:?} (stopped externally?)"));
    }
    println!("Session stopped. Captured events are kept in the log above.");
    log_diag("capture session stopped");
    Ok(())
}

/// 启用 4688 命令行审计（幂等，需管理员）：
/// 1. auditpol 打开「进程创建」成功审计——**必须用子类 GUID**（
///    {0CCE922B-69AE-11D9-BED3-505054503030}），本地化名字（英/中）随系统语言
///    变化，zh-CN 实测英文名不被解析（2026-11-19）；
/// 2. 注册表 ProcessCreationIncludeCmdLine_Enabled=1——实测本机审计开着但
///    此开关没开，CommandLine 全为空串。
///
/// 失败只警告不中断：策略可能被组策略接管，install 的主流程（ETW 采集）不依赖它。
fn enable_4688_cmdline() {
    let ap = run_hidden(
        "auditpol",
        &["/set", PROCESS_CREATION_SUBCATEGORY, "/success:enable"],
    );
    let rk = write_cmdline_flags();
    match (ap, rk) {
        (Ok(_), Ok(true)) => {
            println!("4688 audit: Process Creation enabled with command lines.");
        }
        _ => eprintln!(
            "[etw-capture] 4688 cmdline audit not fully enabled (group policy may override); \
             command-line attribution stays unavailable"
        ),
    }
}

/// 「进程创建」审计子类别（GUID 引用，中文系统不认英文名）。
const PROCESS_CREATION_SUBCATEGORY: &str = "/subcategory:{0CCE922B-69AE-11D9-BED3-505054503030}";
/// ProcessCreationIncludeCmdLine_Enabled 的两个候选落点：文档标准路径
/// （Policies\System\Audit）与老路径（Lsa）——各版本/组件读的位置不一，
/// 双写最稳。该值在本机实测会被环境回写删除（poll_4688_loop 周期自愈兜底）。
const CMDLINE_FLAG_KEYS: [&str; 2] = [
    r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System\Audit",
    r"HKLM\SYSTEM\CurrentControlSet\Control\Lsa",
];
/// 命令行标志自愈周期（秒）。
const FLAG_REASSERT_SECS: u64 = 600; // 周期核对，见 reassert_cmdline_flags_if_missing

/// 把两个落点的 ProcessCreationIncludeCmdLine_Enabled 全部写成 1。
/// 返回是否全部成功（只用作 install 的成败汇总）。
fn write_cmdline_flags() -> Result<bool> {
    let mut all_ok = true;
    for key in CMDLINE_FLAG_KEYS {
        let out = run_hidden(
            "reg",
            &[
                "add",
                key,
                "/v",
                "ProcessCreationIncludeCmdLine_Enabled",
                "/t",
                "REG_DWORD",
                "/d",
                "1",
                "/f",
            ],
        )?;
        all_ok &= out.status.success();
    }
    Ok(all_ok)
}

/// 标志自愈：任一落点缺失/不为 1 就重写全部。环境回写删除是实测行为
/// （2026-11-19，数分钟内值消失、事件命令行随之变空）——静默失灵比
/// 启用失败更糟，轮询循环周期性兜底。
fn reassert_cmdline_flags_if_missing() {
    let flag_present = |key: &str| {
        run_hidden(
            "reg",
            &["query", key, "/v", "ProcessCreationIncludeCmdLine_Enabled"],
        )
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("0x1"))
        .unwrap_or(false)
    };
    let missing = CMDLINE_FLAG_KEYS.iter().any(|key| !flag_present(key));
    if missing {
        let _ = write_cmdline_flags();
        diag("4688 cmdline flag was missing; re-asserted");
    }
}

/// Security 日志里一条 4688 事件的最小字段集。
#[derive(Debug, Clone, PartialEq)]
struct Sec4688 {
    record_id: u64,
    ts: String,
    exe: String,
    parent: Option<String>,
    cmd: Option<String>,
}

/// 事件 SystemTime（rfc3339，含 7 位小数秒）→ Utc；解析失败回退当前时间
/// （宁可时间略晚也不丢事件）。4688 落盘必须用事件时间而非采集时间：
/// 轮询延迟推后的 ts 会直接影响消费端的判定窗口过滤与 last_seen 展示。
fn parse_4688_ts(ts: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now())
}

/// 4688 轮询主循环：EventRecordID 断点续传（状态存 `<log>.4688.state`），
/// 每 5 秒拉一次增量，只写解释器类进程事件。查询/解析失败静默重试（下一轮）。
/// 命令行注册表标志每 FLAG_REASSERT_SECS 核对一次，被环境回写删除则重写。
fn poll_4688_loop(log: &Path, running: &AtomicBool) {
    let state_path = log.with_extension("jsonl.4688.state");
    let mut last_record: u64 = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    // install 刚写过标志，首个自愈周期从 now 起算
    let mut last_flag_check = std::time::Instant::now();
    while running.load(Ordering::Acquire) {
        if last_flag_check.elapsed().as_secs() >= FLAG_REASSERT_SECS {
            last_flag_check = std::time::Instant::now();
            reassert_cmdline_flags_if_missing();
        }
        if let Ok(xml) = query_4688_xml() {
            let mut max_seen = last_record;
            for ev in parse_4688_xml(&xml) {
                if ev.record_id <= last_record {
                    continue;
                }
                max_seen = max_seen.max(ev.record_id);
                if is_interpreter_image(&ev.exe) {
                    let binary = strip_exe(&ev.exe).to_string();
                    if let Err(e) = etw::append_etw_event_at(
                        log,
                        &binary,
                        ev.parent.as_deref(),
                        ev.cmd.as_deref(),
                        parse_4688_ts(&ev.ts),
                    ) {
                        diag(&format!("4688 append failed: {e:#}"));
                    }
                }
            }
            if max_seen > last_record {
                last_record = max_seen;
                let _ = std::fs::write(&state_path, last_record.to_string());
            }
        }
        // 可中断的 sleep：置停机标志后 ≤200ms 内退出
        for _ in 0..(POLL_4688_SECS * 5) {
            if !running.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// 查询最近 2 分钟内（重叠窗口，靠 record_id 去重）的 4688 事件 XML。
/// 直接 spawn wevtutil，不走 cmd /c 包装：Rust 参数拼接按 MSVCRT 规则把内部
/// 引号写成 `\"`，cmd 不认这个转义、把含空格的 XPath 拆成多个参数（实测
/// rc=87 "Too many arguments"）。/uni:true 强制 Unicode（UTF-16LE）输出，
/// 绕开「重定向下 wevtutil 用控制台代码页（中文系统 GBK）」的乱码问题。
fn query_4688_xml() -> Result<String> {
    use std::os::windows::process::CommandExt;
    let query = "/q:*[System[(EventID=4688) and TimeCreated[timediff(@SystemTime) <= 120000]]]";
    // /c:2000：单轮上限。突发超过上限时，旧于断点的溢出事件永久丢失
    // （/rd:true 最新优先 + record_id 断点取 max——下一轮直接跳到更新的断点）；
    // 常态负载每轮个位数，2000 已是充足余量。
    let mut cmd = std::process::Command::new("wevtutil");
    cmd.args([
        "qe",
        "Security",
        query,
        "/c:2000",
        "/rd:true",
        "/f:xml",
        "/uni:true",
    ])
    .creation_flags(orphan_audit_core::inventory::CREATE_NO_WINDOW);
    let out = cmd.output()?;
    if !out.status.success() {
        bail!(
            "wevtutil failed: {}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
    }
    Ok(decode_utf16_bom(&out.stdout))
}

/// 解码 wevtutil /uni:true 的输出：FF FE BOM 开头的 UTF-16LE；其余按 UTF-8 兜底。
fn decode_utf16_bom(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// 解析 wevtutil 输出（一串 <Event>…</Event>，纯函数便于单测）。
/// 只提取归因所需字段；缺 NewProcessName / EventRecordID 的事件跳过。
fn parse_4688_xml(xml: &str) -> Vec<Sec4688> {
    let mut out = Vec::new();
    for ev in xml.split("<Event ").skip(1) {
        let Some((_, body)) = ev.split_once('>') else {
            continue;
        };
        let Some(record_id) = xml_tag(body, "EventRecordID").and_then(|v| v.parse().ok()) else {
            continue;
        };
        let Some(exe) = xml_data(body, "NewProcessName") else {
            continue;
        };
        let ts = xml_attr(body, "TimeCreated", "SystemTime").unwrap_or_default();
        out.push(Sec4688 {
            record_id,
            ts,
            exe,
            parent: xml_data(body, "ParentProcessName"),
            cmd: xml_data(body, "CommandLine").filter(|s| !s.is_empty()),
        });
    }
    out
}

/// 提取 <Tag>value</Tag>（找首个出现；值内可含转义）。
fn xml_tag(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&format!("</{tag}>"))? + start;
    Some(xml_unescape(&body[start..end]))
}

/// 提取 <Data Name='Name'>value</Data>。
fn xml_data(body: &str, name: &str) -> Option<String> {
    let marker = format!("Name='{name}'>");
    let start = body.find(&marker)? + marker.len();
    let end = body[start..].find("</Data>")? + start;
    Some(xml_unescape(&body[start..end]))
}

/// 提取 <Tag Attr='value' …> 形态的属性值。
fn xml_attr(body: &str, tag: &str, attr: &str) -> Option<String> {
    let open = format!("<{tag} ");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find('>')? + start;
    let marker = format!("{attr}='");
    let at = body[start..end].find(&marker)? + start + marker.len();
    let close = body[at..].find('\'')? + at;
    Some(body[at..close].to_string())
}

/// XML 最小反转义（命令行里常见这四个实体）。
fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// 镜像名是否为解释器（不区分大小写；4688 只写这类进程）。
fn is_interpreter_image(exe: &str) -> bool {
    let name = basename(exe).to_ascii_lowercase();
    INTERPRETER_IMAGES.contains(&name.as_str())
}

/// 深度模式（opt-in spike）：Kernel-File 订阅，把 site-packages / node_modules
/// 下的文件打开事件归因成包写入 import 事件日志。内核无路径过滤，事件洪流
/// 由本进程用户态丢弃——CPU 代价可感知，只在前台手动短时运行。
fn run_deep() -> Result<()> {
    if !etw::is_elevated() {
        bail!("deep mode requires administrator privileges");
    }
    let Some(ilog) = orphan_audit_core::imports::default_import_log_path() else {
        bail!("cannot determine data directory for import events");
    };
    // 只清 deep 自己的残留会话（强杀进程会遗留）；不能动常驻采集的 TRACE_NAME
    let _ = stop_trace_by_name(DEEP_TRACE_NAME);
    println!("Deep capture: file opens under site-packages / node_modules");
    println!("Import log: {}", ilog.display());
    println!("Press Ctrl+C to stop.");

    // 实测（Win11 26100 / ferrisetw 1.2）：FILE_IO_INIT 下路径在 Create
    //（opcode 64）事件里叫 **OpenPath** 而非 FileName；Cleanup/Close/QueryInfo
    // 的属性表没有路径。只处理 Create 即可，其余事件直接丢弃。
    // 每包每进程只记一次（与 import 钩子语义一致）：一次 import 会触发同一包
    // 上百次文件打开，不去重会把日志写爆（实测 11 秒 6500 行）。
    let seen = std::sync::Mutex::new(std::collections::HashSet::new());
    let provider = Provider::kernel(&kernel_providers::FILE_INIT_IO_PROVIDER)
        .add_callback(move |record: &EventRecord, locator: &SchemaLocator| {
            if record.opcode() != 64 {
                return; // 非 Create：不带路径
            }
            let Ok(schema) = locator.event_schema(record) else {
                return;
            };
            let parser = Parser::create(record, &schema);
            let Ok(file) = parser.try_parse::<String>("OpenPath") else {
                return;
            };
            let Some(pkg) = package_from_watched_path(&file) else {
                return;
            };
            let first = seen
                .lock()
                .map(|mut s| s.insert(pkg.clone()))
                .unwrap_or(false);
            if first {
                if let Err(e) = orphan_audit_core::imports::append_import_event(&ilog, &pkg, "file")
                {
                    diag(&format!("import append failed: {e:#}"));
                }
            }
        })
        .build();
    let (trace, handle) = KernelTrace::new()
        .named(DEEP_TRACE_NAME.to_string())
        .enable(provider)
        .start()
        .map_err(|e| anyhow!("failed to start kernel trace: {e:?}"))?;

    let running = Arc::new(AtomicBool::new(true));
    let done = Arc::clone(&running);
    std::thread::spawn(move || {
        if let Err(e) = UserTrace::process_from_handle(handle) {
            diag(&format!("event processing ended: {e:?}"));
        }
        done.store(false, Ordering::Release);
    });
    let flag = Arc::clone(&running);
    ctrlc::set_handler(move || flag.store(false, Ordering::Release))
        .context("installing Ctrl+C handler")?;
    while running.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = trace.stop();
    println!("Deep capture stopped.");
    Ok(())
}

/// 从被监视路径提取包名：site-packages 的下一段（顶层包/模块）或
/// node_modules 的下一段（scoped 取两段）；再过滤掉非包段——
/// `.dist-info` / `.egg-info`（pip 装卸与元数据扫描也会打开，不能算使用）、
/// `__pycache__`、`.pth`（解释器启动即扫描）与 TSD 隔离副本（`.IPGSD` /
/// 点前缀）。不匹配返回 None。
fn package_from_watched_path(path: &str) -> Option<String> {
    let norm = path.replace('\\', "/").to_ascii_lowercase();
    for marker in ["site-packages/", "node_modules/"] {
        if let Some(pos) = norm.find(marker) {
            let rest = &path.replace('\\', "/")[pos + marker.len()..];
            let mut segs = rest.split('/');
            let first = segs.next()?;
            if is_non_package_segment(first) {
                continue;
            }
            if first.starts_with('@') {
                let second = segs.next()?;
                if is_non_package_segment(second) {
                    continue;
                }
                return Some(format!("{first}/{second}"));
            }
            return Some(first.to_string());
        }
    }
    None
}

/// 段是否为非包内容（详见 package_from_watched_path 文档）。
fn is_non_package_segment(seg: &str) -> bool {
    let s = seg.to_ascii_lowercase();
    s.is_empty()
        || s.starts_with('.')
        || s == "__pycache__"
        || s.ends_with(".dist-info")
        || s.ends_with(".egg-info")
        || s.ends_with(".pth")
        || s.ends_with(".ipgsd")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_4688_xml_extracts_fields_and_skips_empty_cmd() {
        let xml = "<Event xmlns='x'><System><EventRecordID>731443</EventRecordID>\
<TimeCreated SystemTime='2026-11-18T01:56:08.6970590Z'/></System><EventData>\
<Data Name='NewProcessName'>C:\\Program Files\\nodejs\\node.exe</Data>\
<Data Name='CommandLine'>node C:\\npm\\node_modules\\firecrawl-cli\\bin\\x.js --</Data>\
<Data Name='ParentProcessName'>C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe</Data>\
</EventData></Event><Event xmlns='x'><System><EventRecordID>731444</EventRecordID>\
<TimeCreated SystemTime='2026-11-18T01:56:09Z'/></System><EventData>\
<Data Name='NewProcessName'>C:\\Windows\\System32\\lsass.exe</Data>\
<Data Name='CommandLine'></Data></EventData></Event>";
        let evs = parse_4688_xml(xml);
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].record_id, 731443);
        assert_eq!(evs[0].exe, r"C:\Program Files\nodejs\node.exe");
        assert_eq!(
            evs[0].cmd.as_deref(),
            Some(r"node C:\npm\node_modules\firecrawl-cli\bin\x.js --")
        );
        assert!(evs[0]
            .parent
            .as_deref()
            .is_some_and(|p| p.ends_with("powershell.exe")));
        // 空命令行归一成 None（真实机器 CommandLine 常为空串）
        assert_eq!(evs[1].cmd, None);
    }

    #[test]
    fn interpreter_filter_and_unescape() {
        assert!(is_interpreter_image(r"C:\Program Files\nodejs\node.exe"));
        assert!(is_interpreter_image(r"C:\py\python.EXE"));
        assert!(!is_interpreter_image(r"C:\Windows\System32\lsass.exe"));
        assert_eq!(
            xml_unescape("a &quot;b&quot; &amp; &lt;x&gt;"),
            "a \"b\" & <x>"
        );
    }

    #[test]
    fn decode_utf16_bom_handles_unicode_and_fallback() {
        // BOM + UTF-16LE（含中文，模拟 /uni:true 输出）
        let mut b = vec![0xFF, 0xFE];
        for u in "<Event>".encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_utf16_bom(&b), "<Event>");
        // 无 BOM → UTF-8 兜底
        assert_eq!(decode_utf16_bom(b"<Event>"), "<Event>");
    }

    #[test]
    fn watched_path_extracts_package_names() {
        assert_eq!(
            package_from_watched_path(r"C:\npm\node_modules\firecrawl-cli\bin\x.js"),
            Some("firecrawl-cli".into())
        );
        assert_eq!(
            package_from_watched_path(r"C:\npm\node_modules\@jackwener\opencli\dist\run.js"),
            Some("@jackwener/opencli".into())
        );
        assert_eq!(
            package_from_watched_path(r"C:\py\Lib\site-packages\pymupdf\__init__.py"),
            Some("pymupdf".into())
        );
        // 用户级 site-packages 一样命中
        assert_eq!(
            package_from_watched_path(
                r"C:\Users\u\AppData\Roaming\Python\Python313\site-packages\mcp\server.py"
            ),
            Some("mcp".into())
        );
        assert_eq!(package_from_watched_path(r"C:\Windows\notepad.exe"), None);
        // 非包段过滤：dist-info / __pycache__ / .pth / TSD 隔离副本不算使用
        assert_eq!(
            package_from_watched_path(r"C:\py\Lib\site-packages\aiohttp-3.14.1.dist-info\METADATA"),
            None
        );
        assert_eq!(
            package_from_watched_path(r"C:\py\Lib\site-packages\__pycache__\mcp.cpython-313.pyc"),
            None
        );
        assert_eq!(
            package_from_watched_path(r"C:\py\Lib\site-packages\djlint.pth"),
            None
        );
        assert_eq!(
            package_from_watched_path(r"C:\py\Lib\site-packages\_cffi_backend.pyd.IPGSD\copy.bin"),
            None
        );
    }

    #[test]
    fn sec4688_ts_parses_with_fallback() {
        // wevtutil 实测形态：7 位小数秒的 rfc3339
        let t = parse_4688_ts("2026-11-18T01:56:08.6970590Z");
        let expect = chrono::NaiveDate::from_ymd_opt(2026, 11, 18)
            .unwrap()
            .and_hms_micro_opt(1, 56, 8, 697059)
            .unwrap()
            .and_utc();
        assert_eq!(t, expect);
        // 非法串回退 now（不 panic，值落在当前时刻附近）
        let now = chrono::Utc::now();
        let fb = parse_4688_ts("not-a-time");
        assert!((fb - now).num_seconds().abs() < 60);
        // 空串（解析期无 SystemTime 属性的事件）同样安全回退
        let fb2 = parse_4688_ts("");
        assert!((fb2 - now).num_seconds().abs() < 60);
    }

    #[test]
    fn rotate_log_if_huge_rotates_only_over_limit() {
        let dir = std::env::temp_dir().join(format!("oa_rot_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 诊断落盘重定向到临时目录：单测不得污染生产 capture.log
        std::env::set_var("OA_CAPTURE_LOG", dir.join("capture.log"));
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
        // 轮转成功的同时诊断已落盘（经 OA_CAPTURE_LOG 重定向到临时目录）
        let diag = std::fs::read_to_string(dir.join("capture.log")).unwrap();
        assert!(diag.contains("log rotated"));
        // 不存在的文件：静默返回
        rotate_log_if_huge(&dir.join("nope.jsonl"), 1);
        std::env::remove_var("OA_CAPTURE_LOG");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

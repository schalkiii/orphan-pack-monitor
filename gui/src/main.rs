//! orphan-audit GPUI 桌面界面。
//!
//! 布局（自上而下）：标题栏（徽标 + 版本标识；加载中/失败时右侧显示状态）、
//! 判定分布条（五档占比 + 计数图例，点击色块按档位筛选）、
//! 管理器标签行（UniGetUI 式分类浏览）、
//! 检测环境行（多环境时列出全部 npm prefix / python，悬停看完整路径）、
//! 工具条（观察窗口档位 / 筛选与忽略标签 / 重新审计）、审计警告、
//! 主体（包表格 + 证据面板）、底部卸载操作条（勾选包后两步确认执行卸载）。
//!
//! 说明不打集中面板：各控件就地携带「？」悬停（各路信号的原理 / 判定 / 局限、
//! 五档判定、观察窗口），证据面板空态展示信号总览，警告横幅内嵌 ETW 开启指引。
//! 文案与 core 的判定实现一一对应（verdict.rs 的规则顺序与自适应阈值、
//! signals.rs 的历史与配置扫描、lib.rs 的漂移富化）——改判定逻辑时同步这里。
//!
//! 卸载执行：勾选的包逐个运行 core `uninstall_command` 给出的命令（与 CLI clean
//! 同一来源，多环境定位一致），经 PowerShell 执行（命令含 `& "python.exe"` 与
//! `Remove-Item` 等 PS 语法）；完成即重跑审计核验包是否真的从清单消失——
//! 安全代理环境下 node 子进程的退出码/输出不可信，以审计结果为准。
//!
//! CLI 能力对齐（批 ㉒）：残骸删除入口（shim_removal_commands 同源命令 + 重审计
//! 核验）、导出 clean 脚本（generate_script + write_script_file 同源）、长期对比
//! （store::drift_diff 同源）、钩子管理（core hooks 同源）——GUI 与 CLI 共用
//! core 实现，不各写一份。
//!
//! 数据来自 core 的 audit()；GUI 只负责展示与交互，不重算判定。

mod view;

use std::collections::HashSet;
use std::os::windows::process::CommandExt;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, relative, rgb, rgba, Animation, AnimationExt, App, Bounds, ClickEvent,
    Context, Div, Entity, FontWeight, IntoElement, SharedString, Styled, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
};
use orphan_audit_core::hooks::{self, HooksStatus, ImportLogState};
use orphan_audit_core::inventory;
use orphan_audit_core::model::{
    AuditConfig, DetectedEnv, Manager, OrphanShim, SignalSource, Verdict, VerdictResult,
};
use orphan_audit_core::store::DriftDiff;
use view::{
    build_rows, env_chips, manager_counts, path_display, row_by_index, source_cn, verdict_cn,
    warning_cn, EnvChip, EvidenceLine, Row, SortDir, SortKey, Summary,
};

/// 应用状态：所有可变数据集中在 root view 中。
struct AppState {
    /// 原始审计结果（筛选/排序重建视图用，避免重复跑外部命令）
    results: Vec<VerdictResult>,
    summary: Summary,
    rows: Vec<Row>,
    selected: Option<usize>, // 对应 results 中的索引
    /// 仪表盘档位筛选：Some(v) 只显示该档（点击卡片切换），None 显示全部
    verdict_filter: Option<Verdict>,
    /// 管理器筛选：Some(m) 只显示该管理器（标签行点击），None 显示全部。
    /// 与档位筛选可叠加
    manager_filter: Option<Manager>,
    sort_key: SortKey,
    sort_dir: SortDir,
    /// 状态行：只在有话要说时出现（审计中 / 失败）；成功后保持安静
    status_line: SharedString,
    /// 审计诊断（atime 跳过 / 采集器失败 / store 写失败等）——降级状态必须对用户可见
    warnings: Vec<String>,
    /// 后台审计进行中：表格区显示占位，完成前无数据
    loading: bool,
    /// ETW 追踪是否可用（警告横幅据此内嵌采集器开启指引）
    etw_on: bool,
    /// 观察窗口（天）：切换后按该窗口重新审计，与 CLI --since 对齐
    since_days: u32,
    /// 会话级忽略列表（对应 CLI --ignore）：证据面板「忽略此包」追加，
    /// 工具条标签可移出；每次变更后重跑审计
    ignored: Vec<String>,
    /// 检测到的运行环境（多环境检测）：「环境」行的数据源
    detected_envs: Vec<DetectedEnv>,
    /// 勾选待卸载的包（results 索引）：表格行首勾选框操作
    removal_selected: HashSet<usize>,
    /// 两步确认：第一次点「移除所选」只进入确认态，再点同一按钮才执行
    removal_confirm: bool,
    /// 卸载执行的进度/结果反馈（卸载操作条展示，下次操作或重审计完成前保留）
    removal_status: Option<SharedString>,
    /// 卸载后待核验的包（名称 + 安装路径）：重审计完成后对照新清单，
    /// 未消失的明确告知。带路径是因为同名包可能只卸了其中一份
    removal_pending: Vec<(String, Option<String>)>,
    /// 卸载执行中（按钮进入不可点状态，防重复触发）
    removing: bool,
    /// 审计请求代次：每次 start_audit 自增，回填时代次不符的旧报告直接丢弃——
    /// 快速连点「重新审计」/切窗口时并发审计乱序完成，旧结果不得覆盖新请求
    audit_generation: u64,
    /// npm 卸载残骸（包已卸载、启动器文件仍在 PATH 目录）：残骸操作条数据源
    orphan_shims: Vec<OrphanShim>,
    /// 残骸删除两步确认（与卸载同款：先武装再确认）
    shim_confirm: bool,
    /// 残骸删除执行中
    shim_removing: bool,
    /// 删除完成待核验：重审计后残骸清零才算成功
    shim_pending: bool,
    /// 残骸删除的进度/结果反馈
    shim_status: Option<SharedString>,
    /// 右栏当前面板（证据链 / 长期对比 / 钩子管理）
    right_panel: RightPanel,
    /// 月度快照列表（首次打开长期对比时后台加载）
    snapshots: Vec<String>,
    drift_old: Option<String>,
    drift_new: Option<String>,
    drift_result: Option<Result<Vec<DriftDiff>, String>>,
    /// 快照加载 / 对比进行中（按钮不可点）
    drift_busy: bool,
    /// 钩子状态（首次打开面板时后台探测——要起 python/node 子进程）
    hooks_status: Option<HooksStatus>,
    hooks_busy: bool,
    /// 卸载钩子两步确认
    hooks_confirm: bool,
    /// 钩子安装 / 卸载的反馈
    hooks_feedback: Option<SharedString>,
}

/// 右栏面板：证据链（默认）/ 长期对比 / 钩子管理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RightPanel {
    Evidence,
    Drift,
    Hooks,
}

impl AppState {
    /// 初始加载态：窗口立即可见，审计在后台线程进行（冷启动不阻塞 UI）。
    fn loading() -> Self {
        Self {
            results: Vec::new(),
            summary: Summary::default(),
            rows: Vec::new(),
            selected: None,
            verdict_filter: None,
            manager_filter: None,
            sort_key: SortKey::Verdict,
            sort_dir: SortDir::Asc,
            status_line: "".into(),
            warnings: Vec::new(),
            loading: true,
            etw_on: false,
            since_days: 30,
            ignored: Vec::new(),
            detected_envs: Vec::new(),
            removal_selected: HashSet::new(),
            removal_confirm: false,
            removal_status: None,
            removal_pending: Vec::new(),
            removing: false,
            audit_generation: 0,
            orphan_shims: Vec::new(),
            shim_confirm: false,
            shim_removing: false,
            shim_pending: false,
            shim_status: None,
            right_panel: RightPanel::Evidence,
            snapshots: Vec::new(),
            drift_old: None,
            drift_new: None,
            drift_result: None,
            drift_busy: false,
            hooks_status: None,
            hooks_busy: false,
            hooks_confirm: false,
            hooks_feedback: None,
        }
    }

    /// 后台审计完成后回填状态（成功与失败路径共用）。
    /// 错误已在 spawn 侧格式化为字符串（gui 不直接依赖 anyhow）。
    /// 档位/管理器筛选与排序跨审计保留：用户盯着「npm + 未用」重跑审计，多半还想看同一视图。
    fn apply_report(&mut self, report: Result<orphan_audit_core::AuditReport, String>) {
        self.loading = false;
        match report {
            Ok(report) => {
                self.warnings = report.warnings;
                self.etw_on = report.tracker.available;
                self.detected_envs = report.detected_envs.clone();
                self.summary = Summary::from_results(&report.results);
                self.rows = build_rows(
                    &report.results,
                    self.verdict_filter,
                    self.manager_filter,
                    self.sort_key,
                    self.sort_dir,
                    inventory::npm_default_prefix().as_deref(),
                );
                // 勾选集存的是 results 索引：清单已更新，旧索引指向别的包，
                // 必须整体失效（确认态同步复位——确认的必须是当前所见）
                self.removal_selected.clear();
                self.removal_confirm = false;
                self.shim_confirm = false;
                // 卸载核验（须在 results 被移走之前对照新清单）：
                // 退出码不可信（安全代理吞输出、npm 卸了包也可能报失败），
                // 结论只按重审计后的清单消失与否给出
                if !self.removal_pending.is_empty() {
                    let pending = std::mem::take(&mut self.removal_pending);
                    let (gone, still) = verify_removal(&pending, &report.results);
                    let msg = if still.is_empty() {
                        format!("已移除 {gone} 个包")
                    } else if gone == 0 {
                        format!("未能移除：{}（请到终端手动处理）", still.join("、"))
                    } else {
                        format!(
                            "已移除 {gone} 个包，仍在清单：{}（请到终端手动处理）",
                            still.join("、")
                        )
                    };
                    self.removal_status = Some(msg.into());
                    self.removing = false;
                }
                // 残骸核验：删除动作完成后重审计，残骸清零才算成功
                //（与卸载同一哲学：删除进程的退出码在安全代理环境下不可信）
                if self.shim_pending {
                    self.shim_pending = false;
                    self.shim_removing = false;
                    self.shim_status = Some(if report.orphan_shims.is_empty() {
                        "残骸文件已清理".into()
                    } else {
                        format!(
                            "仍有 {} 组残骸未能删除（文件可能被占用），请到终端手动处理",
                            report.orphan_shims.len()
                        )
                        .into()
                    });
                }
                // 成功后状态行保持安静：包数看图例「全部 N」，ETW 状态看警告横幅
                self.status_line = "".into();
                self.orphan_shims = report.orphan_shims;
                self.results = report.results;
            }
            Err(e) => {
                self.warnings = Vec::new();
                self.detected_envs = Vec::new();
                self.removal_pending.clear();
                self.removing = false;
                // 残骸核验同样依赖重审计成功；失败时撤下悬挂的过程态
                self.shim_pending = false;
                self.shim_removing = false;
                self.shim_status = None;
                // 核验结论依赖重审计成功；失败时悬挂的「正在核验…」必须撤下，
                // 否则操作条永远卡在过程态
                self.removal_status = None;
                self.status_line = format!("审计失败：{e}").into();
            }
        }
    }

    fn rebuild(&mut self) {
        self.summary = Summary::from_results(&self.results);
        self.rows = build_rows(
            &self.results,
            self.verdict_filter,
            self.manager_filter,
            self.sort_key,
            self.sort_dir,
            inventory::npm_default_prefix().as_deref(),
        );
        // 保留选中：只要该包仍在当前视图内（未被筛选丢弃）就不清空。
        // selected 是 results 索引，与排序后的行位置无关，按 index 查找判断。
        if let Some(sel) = self.selected {
            if row_by_index(&self.rows, sel).is_none() {
                self.selected = None;
            }
        }
    }

    /// 后台执行审计（启动与「重新审计」/窗口切换/忽略变更共用）：期间置加载态，
    /// 报告回填后由 apply_report 清除。窗口立即响应，不被子进程探测阻塞。
    /// 代次守卫：快速连点时并发审计乱序完成，只有最新代次的报告允许回填。
    fn start_audit(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.status_line = "".into();
        self.audit_generation += 1;
        let generation = self.audit_generation;
        let since_days = self.since_days;
        let ignored = self.ignored.clone();
        cx.spawn(async move |this, cx| {
            let report = cx
                .background_executor()
                .spawn(async move {
                    orphan_audit_core::audit(&AuditConfig {
                        since_days,
                        ignore: ignored,
                        ..Default::default()
                    })
                })
                .await;
            let _ = this.update(cx, |state: &mut AppState, cx| {
                if state.audit_generation != generation {
                    return; // 旧请求后完成：丢弃，不得覆盖新报告
                }
                state.apply_report(report.map_err(|e| format!("{e:#}")));
                cx.notify();
            });
        })
        .detach();
    }

    /// 勾选/取消一行。选择一旦变化，确认态立即复位——确认的必须是当前所见；
    /// 上一轮卸载的反馈也一并清掉（对新选择没有意义，勾选与取消都算选择变化）。
    fn toggle_removal(&mut self, idx: usize) {
        if !self.removal_selected.remove(&idx) {
            self.removal_selected.insert(idx);
        }
        self.removal_confirm = false;
        self.removal_status = None;
    }

    /// 执行勾选包的卸载：逐个运行 core `uninstall_command`（与 CLI clean 同源，
    /// 多环境定位一致），经 PowerShell 执行——命令里有 `& "python.exe"` 调用
    /// 运算符与 `Remove-Item`，cmd 不认。完成后重跑审计核验是否真的卸掉。
    /// 不按退出码下结论：安全代理会吞卸载进程的输出、扭曲退出码（实测 npm 卸掉
    /// 了包却报失败），成功与否只看重审计后的清单。
    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let targets: Vec<(String, Option<String>, String)> = self
            .results
            .iter()
            .enumerate()
            .filter(|(i, _)| self.removal_selected.contains(i))
            .filter_map(|(_, r)| {
                orphan_audit_core::actions::uninstall_command(r).map(|cmd| {
                    (
                        r.package.name.clone(),
                        r.package
                            .install_path
                            .as_ref()
                            .map(|p| p.display().to_string()),
                        cmd,
                    )
                })
            })
            .collect();
        let count = targets.len();
        self.removal_confirm = false;
        self.removing = true;
        self.removal_status = Some(format!("正在移除 {count} 个包…").into());
        cx.spawn(async move |this, cx| {
            // 命令串克隆给后台块；targets 本体留给下方核验清单用
            let cmds: Vec<String> = targets.iter().map(|(_, _, c)| c.clone()).collect();
            cx.background_executor()
                .spawn(async move {
                    for cmd in &cmds {
                        let mut command = std::process::Command::new("powershell");
                        command
                            .args(["-NoProfile", "-NonInteractive", "-Command", cmd])
                            .creation_flags(inventory::CREATE_NO_WINDOW);
                        let _ = command.output();
                    }
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                // 全部送去核验：重审计完成后按清单消失与否给结论
                state.removal_pending = targets.into_iter().map(|(n, path, _)| (n, path)).collect();
                state.removal_status = Some("正在重新审计核验…".into());
                // 卸载会改变清单，选中的 results 索引即将失效
                state.removal_selected.clear();
                state.selected = None;
                state.start_audit(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 删除全部卸载残骸：逐个运行 core `shim_removal_commands` 给出的命令（与
    /// clean 脚本的逐文件删除行同源），经 PowerShell 执行；完成后重跑审计核验——
    /// 残骸清零才算成功（同卸载哲学：删除进程的退出码在安全代理环境下不可信）。
    fn remove_shims(&mut self, cx: &mut Context<Self>) {
        let cmds = orphan_audit_core::actions::shim_removal_commands(&self.orphan_shims);
        let count = cmds.len();
        self.shim_confirm = false;
        self.shim_removing = true;
        self.shim_status = Some(format!("正在删除 {count} 个残骸文件…").into());
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    for cmd in &cmds {
                        let mut command = std::process::Command::new("powershell");
                        command
                            .args(["-NoProfile", "-NonInteractive", "-Command", cmd])
                            .creation_flags(inventory::CREATE_NO_WINDOW);
                        let _ = command.output();
                    }
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.shim_pending = true;
                state.shim_status = Some("正在重新审计核验…".into());
                state.start_audit(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 导出 clean 脚本到桌面（与 CLI `clean --output` 同一生成器与写盘函数；
    /// write_script_file 带 UTF-8 BOM，PowerShell 5.1 无 BOM 按 ANSI 读会乱码）。
    fn export_script(&mut self, cx: &mut Context<Self>) {
        self.status_line = "正在导出脚本…".into();
        let results = self.results.clone();
        let shims = self.orphan_shims.clone();
        let since_days = self.since_days;
        cx.spawn(async move |this, cx| {
            let out = cx
                .background_executor()
                .spawn(async move {
                    let script =
                        orphan_audit_core::actions::generate_script(&results, &shims, since_days);
                    let path = orphan_audit_core::actions::default_script_export_path();
                    match orphan_audit_core::actions::write_script_file(&path, &script) {
                        Ok(()) => Ok(path),
                        Err(e) => Err(format!("{e}")),
                    }
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.status_line = match out {
                    Ok(p) => format!("清理脚本已导出到 {}（审阅后运行）", p.display()).into(),
                    Err(e) => format!("脚本导出失败：{e}").into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 右栏面板切换（再点同一按钮回到证据链）；首次进入长期对比时加载快照。
    fn show_panel(&mut self, panel: RightPanel, cx: &mut Context<Self>) {
        self.right_panel = if self.right_panel == panel {
            RightPanel::Evidence
        } else {
            panel
        };
        if panel == RightPanel::Drift
            && self.right_panel == RightPanel::Drift
            && self.snapshots.is_empty()
        {
            self.load_snapshots(cx);
        }
        cx.notify();
    }

    /// 后台加载月度快照列表（开库不应卡 UI；审计同库写入时可能 busy，错误入结果区）。
    fn load_snapshots(&mut self, cx: &mut Context<Self>) {
        if self.drift_busy {
            return;
        }
        self.drift_busy = true;
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    orphan_audit_core::store::Store::open(&orphan_audit_core::default_db_path())
                        .and_then(|st| st.snapshot_ids())
                        .map_err(|e| format!("{e:#}"))
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.drift_busy = false;
                match loaded {
                    Ok(ids) => state.snapshots = ids,
                    Err(e) => state.drift_result = Some(Err(e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 执行快照对比（旧 / 新都已选才可点）：库读取与对比都在后台线程。
    fn run_drift(&mut self, cx: &mut Context<Self>) {
        let (Some(old), Some(new)) = (self.drift_old.clone(), self.drift_new.clone()) else {
            return;
        };
        self.drift_busy = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    orphan_audit_core::store::Store::open(&orphan_audit_core::default_db_path())
                        .and_then(|st| st.drift_diff(&old, &new))
                        .map_err(|e| format!("{e:#}"))
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.drift_busy = false;
                state.drift_result = Some(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// 钩子面板开关（再点回到证据链）；状态未探测过时后台探测。
    fn show_hooks(&mut self, cx: &mut Context<Self>) {
        self.right_panel = if self.right_panel == RightPanel::Hooks {
            RightPanel::Evidence
        } else {
            RightPanel::Hooks
        };
        if self.right_panel == RightPanel::Hooks && self.hooks_status.is_none() && !self.hooks_busy
        {
            self.refresh_hooks(cx);
        }
        cx.notify();
    }

    /// 后台探测钩子状态（python 解释器 / venv / node 逐个查证，要起子进程）。
    fn refresh_hooks(&mut self, cx: &mut Context<Self>) {
        self.hooks_busy = true;
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async move { hooks::status_info() })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.hooks_busy = false;
                state.hooks_status = Some(status);
                cx.notify();
            });
        })
        .detach();
    }

    /// 安装钩子（一次点击直接执行；装完重新探测，以最新状态为准）。
    fn install_hooks(&mut self, cx: &mut Context<Self>) {
        self.hooks_busy = true;
        self.hooks_feedback = Some("正在安装钩子…".into());
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move { hooks::install().map_err(|e| format!("{e:#}")) })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.hooks_busy = false;
                state.hooks_feedback = Some(match res {
                    Ok(text) => hooks_feedback_text("钩子已安装，下方为最新状态", &text).into(),
                    Err(e) => format!("钩子安装失败：{e}").into(),
                });
                state.refresh_hooks(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 卸载钩子（两步确认后执行；卸完重新探测）。
    fn uninstall_hooks(&mut self, cx: &mut Context<Self>) {
        self.hooks_confirm = false;
        self.hooks_busy = true;
        self.hooks_feedback = Some("正在卸载钩子…".into());
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move { hooks::uninstall().map_err(|e| format!("{e:#}")) })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.hooks_busy = false;
                state.hooks_feedback = Some(match res {
                    Ok(text) => hooks_feedback_text("钩子已卸载，下方为最新状态", &text).into(),
                    Err(e) => format!("钩子卸载失败：{e}").into(),
                });
                state.refresh_hooks(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

/// 钩子操作反馈拼装（纯函数，便于测试）：结果行在前；core 随返回值带回的
/// `[hooks]` 诊断行（R16-H1：逐解释器失败明细不再走 stderr）逐行并入。
/// gpui 纯文本不渲染 `\n`，渲染侧按行拆 div。
fn hooks_feedback_text(headline: &str, result: &str) -> String {
    let diag: Vec<&str> = result
        .lines()
        .filter(|l| l.starts_with("[hooks]"))
        .collect();
    if diag.is_empty() {
        return headline.to_string();
    }
    let mut out = format!("{headline}（部分失败）：");
    for d in diag {
        out.push('\n');
        out.push_str(d);
    }
    out
}

/// 卸载核验对账（纯函数，便于测试）：对照重审计后的清单，返回
/// （已消失数, 仍在清单的包名——去重排序供展示）。gone 按 (名称, 安装路径)
/// 对逐份计算——同名多环境包（如双 prefix 的 opencli）各算各的，卸载失败
/// 两份就得报 0；名字去重只用于提示文案。路径匹配按字符串原值（采集侧
/// 生成待核验对与重审计清单走同一来源）。
fn verify_removal(
    pending: &[(String, Option<String>)],
    results: &[orphan_audit_core::model::VerdictResult],
) -> (usize, Vec<String>) {
    let mut still: Vec<String> = pending
        .iter()
        .filter(|(n, path)| {
            results.iter().any(|r| {
                &r.package.name == n
                    && r.package
                        .install_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .as_deref()
                        == path.as_deref()
            })
        })
        .map(|(n, _)| n.clone())
        .collect();
    let gone = pending.len() - still.len();
    still.sort();
    still.dedup();
    (gone, still)
}

/// 钩子面板的状态行（纯函数，便于测试）：中文渲染 core HooksStatus，
/// import 日志三分支与 CLI status / 审计警告同一语义（缺失 ≠ 被隔离）。
fn hooks_lines(st: &HooksStatus) -> Vec<String> {
    let mut out = Vec::new();
    for (interp, has) in &st.interpreters {
        out.push(format!(
            "{} — {}",
            interp.display(),
            if *has { "已挂 .pth" } else { "未挂钩" }
        ));
    }
    let hooked = st.interpreters.iter().filter(|(_, h)| *h).count();
    out.push(format!(
        "python 解释器 {hooked}/{} · 工具 venv {}/{} · node {}",
        st.interpreters.len(),
        st.tool_venvs.0,
        st.tool_venvs.1,
        if st.node_hooked {
            "NODE_OPTIONS 已接"
        } else {
            "未接"
        }
    ));
    if let Some((log, state)) = &st.log {
        out.push(match state {
            ImportLogState::Present(n) => {
                format!("import 日志：{}（{n} 字节）", log.display())
            }
            ImportLogState::Quarantined(shadow) => format!(
                "import 日志被安全软件隔离：影子副本 {}（使用证据以审计侧读取尝试为准）",
                shadow.display()
            ),
            ImportLogState::Missing => {
                "import 日志尚未生成（钩子装好后跑一次 python 即产生）".to_string()
            }
        });
    }
    out
}

/// 长期对比结果行（纯函数）：(档位中文名, 新增名单, 移除名单)；空名单渲染层显示「—」。
fn drift_lines(diffs: &[DriftDiff]) -> Vec<(String, String, String)> {
    diffs
        .iter()
        .map(|d| {
            (
                verdict_cn(d.verdict).to_string(),
                if d.added.is_empty() {
                    "—".to_string()
                } else {
                    d.added.join("、")
                },
                if d.removed.is_empty() {
                    "—".to_string()
                } else {
                    d.removed.join("、")
                },
            )
        })
        .collect()
}

// --- 「？」悬停说明：内容定义 ---

/// 各路信号的一句话说明（每信号恰好一行）：证据链空态的信号总览与
/// 每条证据的「？」共用。与 core 实现对应：History = signals.rs 历史扫描
/// （全量计数）、Etw = etw.rs / etw-capture、Atime = 访问时间兜底（含扫盘
/// 折扣）、ShellConfig/Alias = signals.rs 配置索引、Wrapped = 剥 wrapper、
/// Drift = lib.rs 月度快照富化。
fn signal_help(s: SignalSource) -> (&'static str, Vec<&'static str>) {
    match s {
        SignalSource::History => ("shell 历史", vec![
            "历史文件里的输入计数——交互使用的直接记录，阈值随历史量自适应；看不到 IDE 与脚本里的调用。",
        ]),
        SignalSource::ShellConfig => ("shell 配置", vec![
            "配置文件里登记过的命令——没执行也算「集成」，删除会破坏启动脚本与补全（宁可多保留）。",
        ]),
        SignalSource::Alias => ("别名", vec![
            "解析配置里的 alias / Set-Alias / function——别名指向的原命令同样计入「集成」。",
        ]),
        SignalSource::Wrapped => ("还原命令", vec![
            "sudo / xargs / timeout 等前缀剥掉 wrapper 还原真实命令，使用归属到包本体。",
        ]),
        SignalSource::Etw => ("ETW 执行日志", vec![
            "常驻采集器记录的真实进程执行（含命令行与父进程）——覆盖一切调用方式；未部署时该路缺席。",
        ]),
        SignalSource::Import => ("import 钩子", vec![
            "python/node 钩子记录 import / require——捕捉 IDE、agent、项目脚本「被程序调用」的使用。",
        ]),
        SignalSource::Atime => ("文件访问时间", vec![
            "二进制文件最后访问时间——兜底信号；杀毒 / 备份扫盘的误触已按簇剔除。",
        ]),
        SignalSource::Drift => ("长期未用", vec![
            "月度快照连续 3 个月以上「未用」时追加——区分「本月恰好没跑」与「长期没人用」。",
        ]),
    }
}

/// 五档判定总览（证据面板「？」用；规则顺序见 core verdict.rs，命中即停）。
fn verdict_tiers_help() -> Vec<&'static str> {
    vec![
        "活跃：手动输入次数达到自适应阈值（每 1000 条历史计 1 次、3~50 之间）。",
        "间接：窗口内被执行过（ETW / import 钩子 / 访问时间），即使触发者是脚本或程序。",
        "偶用：历史里输入过，但窗口内没有任何执行迹象。",
        "集成：没执行过，但被 shell 配置 / 别名引用——删除会破坏环境。",
        "未用：窗口内所有信号均无记录——唯一进入清理候选的档位。",
        "规则按「活跃 → 间接 → 偶用 → 集成 → 未用」的顺序判定，命中即停。",
    ]
}

/// 单个档位的悬停说明（分布条图例用；规则顺序见 core verdict.rs，命中即停）。
fn verdict_help(v: Verdict) -> (&'static str, Vec<&'static str>) {
    let line = match v {
        Verdict::Active => "活跃：手动输入次数达到自适应阈值（每 1000 条历史计 1 次、3~50 之间）。",
        Verdict::Indirect => {
            "间接：窗口内被执行过（ETW / import 钩子 / 访问时间），即使触发者是脚本或程序。"
        }
        Verdict::Rare => "偶用：历史里输入过，但窗口内没有任何执行迹象。",
        Verdict::Integration => "集成：没执行过，但被 shell 配置 / 别名引用——删除会破坏环境。",
        Verdict::Unused => "未用：窗口内所有信号均无记录——唯一进入清理候选的档位。",
    };
    (verdict_cn(v), vec![line])
}

/// 观察窗口的悬停说明。
fn window_help() -> Vec<&'static str> {
    vec![
        "「近期」都以观察窗口为界：ETW 执行与文件访问时间只统计最近 N 天，窗口外的旧执行不进证据链。",
        "历史输入是全量计数（不按窗口截断），阈值随历史总量自适应。",
        "切换档位会按新窗口重新审计，与 CLI --since 参数一致。",
    ]
}

// --- 「？」悬停说明：组件 ---

/// 悬停提示的内容视图：标题 + 正文行。由 gpui 框架自动定位在鼠标附近。
struct HelpTooltip {
    title: SharedString,
    lines: Vec<SharedString>,
}

impl Render for HelpTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // 定宽是必须的：gpui 以 AvailableSpace::Min 测量 tooltip（window.rs 的
        // layout_as_root(min_size)），max_w 下文本拿不到确定宽度 → 不换行，
        // 长句挤成超宽一行且各行高度塌陷叠在一起（2026-11-19 实测）；
        // 定宽后子文本以 Definite 宽度测量才会正常折行、逐行堆叠
        let mut col = div()
            .w(px(360.0))
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_md()
            .bg(rgb(0x1e2228))
            .border_1()
            .border_color(rgb(0x3a4049))
            .shadow_lg()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xe3e6ea))
                    .child(self.title.clone()),
            );
        for l in &self.lines {
            col = col.child(
                div()
                    .text_xs()
                    .line_height(relative(1.6))
                    .text_color(rgb(0xcdd3db))
                    .child(l.clone()),
            );
        }
        col
    }
}

/// 「？」图标：悬停约 0.2s 弹出说明。id 需在视图内唯一（复用处传不同 id）。
/// tooltip 闭包是 Fn（可多次触发），标题与行内容每次调用前克隆。
/// 字符用半角 "?"（全角 ？ 的墨迹偏字面左下，圆圈内视觉不居中），
/// 行高压到 1.0 + 1px 下移做光学对齐。
fn help_icon(id: SharedString, title: &str, lines: &[&'static str]) -> impl IntoElement {
    let title: SharedString = title.into();
    let lines: Vec<SharedString> = lines.iter().map(|l| (*l).into()).collect();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(16.0))
        .rounded_full()
        .border_1()
        .border_color(rgb(0x3a4049))
        .text_xs()
        .text_color(rgb(0x757d87))
        .hover(|s| s.border_color(rgb(0x7aa2f7)).text_color(rgb(0x7aa2f7)))
        .child(div().line_height(relative(1.0)).pt(px(1.0)).child("?"))
        .tooltip(move |_window, app| {
            app.new(|_| HelpTooltip {
                title: title.clone(),
                lines: lines.clone(),
            })
            .into()
        })
        .tooltip_show_delay(Duration::from_millis(200))
}

// --- 配色 ---

/// 档位主题色（仪表盘色条与数字 / 判定文字 / 选中行左侧色条共用）。
fn verdict_color(v: Verdict) -> gpui::Rgba {
    match v {
        Verdict::Active => rgb(0x5dcaa5),
        Verdict::Indirect => rgb(0xb48ead),
        Verdict::Rare => rgb(0xef9f27),
        Verdict::Integration => rgb(0xe0af68),
        Verdict::Unused => rgb(0xe24b4a),
    }
}

/// 信号来源配色（badge 前景 / 底色）：主信号各一色，辅助信号用中性色。
/// 底色为同色 ~15% 透明度（rgba 低字节是 alpha）。
fn source_palette(s: SignalSource) -> (gpui::Rgba, gpui::Rgba) {
    let (fg, bg) = match s {
        SignalSource::History => (0x7aa2f7u32, 0x7aa2f726),
        SignalSource::Etw => (0x5dcaa5, 0x5dcaa526),
        SignalSource::Import => (0x89ddff, 0x89ddff26),
        SignalSource::Atime => (0xe0af68, 0xe0af6826),
        SignalSource::ShellConfig => (0xb48ead, 0xb48ead26),
        SignalSource::Alias => (0x7dcfff, 0x7dcfff26),
        SignalSource::Wrapped => (0x9aa2ac, 0x9aa2ac26),
        SignalSource::Drift => (0xef9f27, 0xef9f2726),
    };
    (rgb(fg), rgba(bg))
}

// --- 布局组件 ---

/// 标题栏：徽标（呼应 exe 图标的放大镜）+ 名称 + 版本标识 + 副标题。
/// 右侧只在有话要说时出现状态（审计中 / 失败）；成功后保持安静——
/// 包数看图例「全部 N」，ETW 降级看警告横幅。
fn title_bar(status: SharedString) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_5()
        .py_3()
        .border_b_1()
        .border_color(rgb(0x2a2f37))
        .child(
            div()
                .flex()
                .size(px(26.0))
                .rounded_full()
                .border_2()
                .border_color(rgb(0x9aa2ac))
                .justify_center()
                .items_center()
                .child(div().size(px(9.0)).rounded_full().bg(rgb(0x9aa2ac))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("orphan-audit"),
                        )
                        .child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded_md()
                                .bg(rgb(0x2a3138))
                                .text_xs()
                                .text_color(rgb(0x9aa2ac))
                                .child(concat!("v", env!("CARGO_PKG_VERSION"))),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x757d87))
                        .child("Windows 全局包审计器，找出装了却不用的孤儿包"),
                ),
        )
        .child(div().flex_1())
        .when(!status.is_empty(), |el| {
            el.child(div().text_sm().text_color(rgb(0x9aa2ac)).child(status))
        })
}

/// 仪表盘：五档判定分布条 + 图例。占比是真实数据不是装饰——一段色就是一档的
/// 包数比例；色块与图例都可点击按档位筛选（再点一次恢复），悬停图例看档位含义。
fn dashboard(
    summary: &Summary,
    verdict_filter: Option<Verdict>,
    this: Entity<AppState>,
) -> impl IntoElement {
    let verdicts = [
        Verdict::Active,
        Verdict::Indirect,
        Verdict::Rare,
        Verdict::Integration,
        Verdict::Unused,
    ];
    let counts = [
        summary.active,
        summary.indirect,
        summary.rare,
        summary.integration,
        summary.unused,
    ];
    let total: usize = counts.iter().sum();
    let mut bar = div()
        .flex()
        .h(px(10.0))
        .w_full()
        .rounded_sm()
        .overflow_hidden()
        .bg(rgb(0x22262d));
    let mut legend = div().flex().flex_wrap().items_center().gap_x_3().gap_y_1();
    // 「全部」项：显式的筛选恢复入口 + 总数（旧「总计」卡片的安静版本）
    let all_active = verdict_filter.is_none();
    let this_all = this.clone();
    legend = legend.child(
        div()
            .id("legend-all")
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .py_0p5()
            .rounded_sm()
            .hover(|s| s.bg(rgb(0x22262d)))
            .child(
                div()
                    .text_xs()
                    .font_weight(if all_active {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if all_active {
                        rgb(0xe3e6ea)
                    } else {
                        rgb(0x9aa2ac)
                    })
                    .child("全部"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x757d87))
                    .child(summary.total.to_string()),
            )
            .tooltip(move |_window, app| {
                app.new(|_| HelpTooltip {
                    title: "全部".into(),
                    lines: vec![
                        "全部审计到的全局包，不筛选。".into(),
                        "点击恢复显示全部档位。".into(),
                    ],
                })
                .into()
            })
            .tooltip_show_delay(Duration::from_millis(300))
            .on_click(
                move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                    this_all.update(app, |state: &mut AppState, cx| {
                        state.verdict_filter = None;
                        state.rebuild();
                        cx.notify();
                    });
                },
            ),
    );
    for (v, n) in verdicts.iter().zip(counts) {
        if n > 0 {
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("seg-{}", v.as_str())))
                    .cursor_pointer()
                    .w(relative(n as f32 / total as f32))
                    .h_full()
                    .bg(verdict_color(*v))
                    .hover(|s| s.opacity(0.8))
                    .on_click(segment_click(*v, this.clone())),
            );
        }
        legend = legend.child(legend_item(*v, n, verdict_filter, this.clone()));
    }
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .px_5()
        .py_3()
        .border_b_1()
        .border_color(rgb(0x2a2f37))
        .child(bar)
        .child(legend)
}

/// 分布条色块与图例共用的点击行为：同档再点取消筛选，否则只看该档。
fn segment_click(
    v: Verdict,
    this: Entity<AppState>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
        this.update(app, |state: &mut AppState, cx| {
            state.verdict_filter = if state.verdict_filter == Some(v) {
                None
            } else {
                Some(v)
            };
            state.rebuild();
            cx.notify();
        });
    }
}

/// 图例单项：色点 + 档名 + 计数。当前筛选档位提亮；悬停显示档位含义与点击行为。
fn legend_item(
    v: Verdict,
    count: usize,
    filter: Option<Verdict>,
    this: Entity<AppState>,
) -> impl IntoElement {
    let color = verdict_color(v);
    let active = filter == Some(v);
    let (help_title, mut help_lines) = verdict_help(v);
    help_lines.push("点击色块或图例只看该档；再点一次恢复全部。");
    let help_title: SharedString = help_title.into();
    let help_lines: Vec<SharedString> = help_lines.into_iter().map(Into::into).collect();
    div()
        .id(SharedString::from(format!("legend-{}", v.as_str())))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap_1()
        .px_1()
        .py_0p5()
        .rounded_sm()
        .hover(|s| s.bg(rgb(0x22262d)))
        .child(div().size(px(7.0)).rounded_full().bg(color))
        .child(
            div()
                .text_xs()
                .font_weight(if active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if active { color } else { rgb(0xcdd3db) })
                .child(verdict_cn(v)),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x757d87))
                .child(count.to_string()),
        )
        .tooltip(move |_window, app| {
            app.new(|_| HelpTooltip {
                title: help_title.clone(),
                lines: help_lines.clone(),
            })
            .into()
        })
        .tooltip_show_delay(Duration::from_millis(300))
        .on_click(segment_click(v, this))
}

/// 工具条：观察窗口（分段控件 + ？）→ 档位筛选标签 → 忽略标签 → 操作按钮。
fn toolbar(
    since_days: u32,
    verdict_filter: Option<Verdict>,
    ignored: &[String],
    panel: RightPanel,
    this: Entity<AppState>,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_5()
        .py_2()
        .border_b_1()
        .border_color(rgb(0x2a2f37))
        .child(div().text_xs().text_color(rgb(0x757d87)).child("观察窗口"))
        .child(help_icon("help-window".into(), "观察窗口", &window_help()))
        .child(window_tabs(since_days, this.clone()))
        // 当前档位筛选的指示标签（仪表盘卡片点击后出现）
        .when_some(verdict_filter, |el, v| {
            el.child(filter_chip(v, this.clone()))
        })
        // 已忽略包标签（点击 × 移出忽略列表并重跑审计）；
        // 忽略语义与 CLI --ignore 一致：整包退出审计
        .when(!ignored.is_empty(), |el| {
            el.child(div().text_xs().text_color(rgb(0x757d87)).child("已忽略"))
                .children(
                    ignored
                        .iter()
                        .map(|n| ignored_chip(n, this.clone()))
                        .collect::<Vec<_>>(),
                )
        })
        .child(div().flex_1())
        .child(tool_button(
            "export-script",
            "导出脚本",
            false,
            this.clone(),
            |state, cx| state.export_script(cx),
        ))
        .child(tool_button(
            "panel-drift",
            "长期对比",
            panel == RightPanel::Drift,
            this.clone(),
            |state, cx| state.show_panel(RightPanel::Drift, cx),
        ))
        .child(tool_button(
            "panel-hooks",
            "钩子",
            panel == RightPanel::Hooks,
            this.clone(),
            |state, cx| state.show_hooks(cx),
        ))
        .child(
            div()
                .id("re-audit")
                .cursor_pointer()
                .px_3()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(rgb(0x2a2f37))
                .bg(rgb(0x22262d))
                .text_sm()
                .text_color(rgb(0x9aa2ac))
                .child("重新审计")
                .on_click({
                    let this = this.clone();
                    move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                        this.update(app, |state: &mut AppState, cx| {
                            // shell 历史/包清单会变化，重跑一次完整审计
                            state.start_audit(cx);
                            cx.notify();
                        });
                    }
                }),
        )
}

/// 工具条通用按钮（导出脚本 / 右栏面板开关）：active 时主题色高亮。
fn tool_button(
    id: &'static str,
    label: &'static str,
    active: bool,
    this: Entity<AppState>,
    on_click: impl Fn(&mut AppState, &mut Context<AppState>) + 'static,
) -> impl IntoElement {
    let hover_color = if active { rgb(0x7aa2f7) } else { rgb(0xe3e6ea) };
    div()
        .id(SharedString::from(format!("tb-{id}")))
        .cursor_pointer()
        .px_3()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(if active { rgb(0x33415e) } else { rgb(0x2a2f37) })
        .bg(if active { rgb(0x2a3138) } else { rgb(0x22262d) })
        .text_sm()
        .text_color(if active { rgb(0x7aa2f7) } else { rgb(0x9aa2ac) })
        .hover(move |s| s.text_color(hover_color))
        .child(label)
        .on_click(move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
            this.update(app, |state: &mut AppState, cx| {
                on_click(state, cx);
                cx.notify();
            });
        })
}

/// 观察窗口分段控件：当前档位高亮，点击切换并按新窗口重新审计。
fn window_tabs(current: u32, this: Entity<AppState>) -> impl IntoElement {
    div()
        .flex()
        .gap_0p5()
        .p_0p5()
        .rounded_md()
        .border_1()
        .border_color(rgb(0x2a2f37))
        .bg(rgb(0x191d22))
        .child(window_tab("30 天", 30, current, this.clone()))
        .child(window_tab("90 天", 90, current, this.clone()))
        .child(window_tab("1 年", 365, current, this))
}

/// 分段控件的单个档位标签。
fn window_tab(
    label: &'static str,
    days: u32,
    current: u32,
    this: Entity<AppState>,
) -> impl IntoElement {
    let active = current == days;
    // 悬停色：非激活时提亮，激活时保持主题色（避免高亮被悬停冲掉）
    let hover_color = if active { rgb(0x7aa2f7) } else { rgb(0xe3e6ea) };
    div()
        .id(SharedString::from(format!("since-{days}")))
        .cursor_pointer()
        .px_2p5()
        .py_0p5()
        .rounded_sm()
        .text_xs()
        .text_color(if active { rgb(0x7aa2f7) } else { rgb(0x9aa2ac) })
        .bg(if active {
            rgb(0x2a3138)
        } else {
            rgba(0x00000000)
        })
        .when(active, |el| el.font_weight(FontWeight::MEDIUM))
        .hover(move |s| s.text_color(hover_color))
        .child(label)
        .on_click(
            move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                this.update(app, move |state: &mut AppState, cx| {
                    state.since_days = days;
                    // since 影响判定，必须重跑（不能只重排缓存结果）
                    state.start_audit(cx);
                    cx.notify();
                });
            },
        )
}

/// 管理器标签行的悬停说明（承担原使用说明面板的「运作方式」段）。
fn manager_help() -> Vec<&'static str> {
    vec![
        "审计范围：cargo / npm / pip / uv / pipx 的全局包 + winget 便携包；本机没装的管理器不会出现在标签里。",
        "winget 只收便携包（WinGet\\Links 下注册过命令的）；MSI/EXE 应用不纳入——安装来源与命令都无法可靠对应到包，纳入只会产出无证据的「未用」。",
        "点击标签只看该管理器的包（可与仪表盘档位筛选叠加）；再点一次或点「全部」恢复。",
    ]
}

/// 管理器标签行：UniGetUI 式按包管理器分类浏览——「全部」+ 每个出现过的
/// 管理器一枚（名称 + 包数）。筛选与档位（仪表盘卡片）叠加，双维度交叉查看。
fn manager_bar(
    counts: &[(Manager, usize)],
    current: Option<Manager>,
    this: Entity<AppState>,
) -> impl IntoElement {
    let total: usize = counts.iter().map(|(_, n)| *n).sum();
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_5()
        .py_2()
        .border_b_1()
        .border_color(rgb(0x2a2f37))
        .child(div().text_xs().text_color(rgb(0x757d87)).child("管理器"))
        .child(help_icon("help-managers".into(), "管理器", &manager_help()))
        .child(manager_tab("全部", None, total, current, this.clone()))
        .children(
            counts
                .iter()
                .map(|(m, n)| manager_tab(m.as_str(), Some(*m), *n, current, this.clone()))
                .collect::<Vec<_>>(),
        )
}

/// 单枚管理器标签：选中态蓝底描边，包数以弱色缀后（加载中无计数）。
fn manager_tab(
    label: &str,
    manager: Option<Manager>,
    count: usize,
    current: Option<Manager>,
    this: Entity<AppState>,
) -> impl IntoElement {
    let active = current == manager;
    let id = SharedString::from(format!(
        "mgr-{}",
        manager.map(|m| m.as_str()).unwrap_or("all")
    ));
    let label: SharedString = label.into();
    div()
        .id(id)
        .cursor_pointer()
        .flex()
        .items_center()
        .gap_1()
        .px_2p5()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(if active { rgb(0x7aa2f7) } else { rgb(0x2a2f37) })
        .bg(if active {
            rgb(0x2a3138)
        } else {
            rgba(0x00000000)
        })
        .text_xs()
        .text_color(if active { rgb(0x7aa2f7) } else { rgb(0x9aa2ac) })
        .when(active, |el| el.font_weight(FontWeight::MEDIUM))
        .hover(|s| s.text_color(rgb(0xe3e6ea)))
        .child(label)
        // 审计进行中时 counts 为空（全部标签也不显示 0），完成后自然带上计数
        .when(count > 0, |el| {
            el.child(
                div()
                    .text_xs()
                    .text_color(if active { rgb(0x7aa2f7) } else { rgb(0x757d87) })
                    .child(count.to_string()),
            )
        })
        .on_click(
            move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                this.update(app, move |state: &mut AppState, cx| {
                    // 同一标签再点一次：取消筛选（与仪表盘卡片一致）
                    state.manager_filter = if state.manager_filter == manager {
                        None
                    } else {
                        manager
                    };
                    state.rebuild();
                    cx.notify();
                });
            },
        )
}

/// 检测环境行：多环境检测的可视化——npm 枚举到的全部全局 prefix 与 python
/// 枚举到的全部解释器各一组短名标签（彩点区分种类），悬停查看完整路径。
/// 每类都只有 0/1 处时不显示（默认单环境没有信息量）。
fn env_bar(envs: &[DetectedEnv]) -> Option<impl IntoElement> {
    let chips = env_chips(envs, inventory::npm_default_prefix().as_deref());
    if chips.npm.len() <= 1 && chips.python.len() <= 1 {
        return None;
    }
    let mut row = div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_5()
        .py_2()
        .border_b_1()
        .border_color(rgb(0x2a2f37))
        .child(div().text_xs().text_color(rgb(0x757d87)).child("环境"))
        .child(help_icon("help-envs".into(), "检测环境", &env_help()));
    if !chips.npm.is_empty() {
        row = row
            .child(div().text_xs().text_color(rgb(0x757d87)).child("npm"))
            .children(
                chips
                    .npm
                    .iter()
                    .map(|c| env_chip("npm", rgb(0x7aa2f7), "npm 全局 prefix", c))
                    .collect::<Vec<_>>(),
            );
    }
    if !chips.python.is_empty() {
        row = row
            .child(div().text_xs().text_color(rgb(0x757d87)).child("python"))
            .children(
                chips
                    .python
                    .iter()
                    .map(|c| env_chip("python", rgb(0x5dcaa5), "python 解释器", c))
                    .collect::<Vec<_>>(),
            );
    }
    Some(row)
}

/// 单枚环境标签：彩点标识种类（npm 蓝 / python 绿），悬停显示完整路径。
/// id 用完整路径保证唯一（不同环境的短名可能撞车）。
fn env_chip(
    group: &str,
    color: gpui::Rgba,
    tooltip_title: &str,
    chip: &EnvChip,
) -> impl IntoElement {
    let id = SharedString::from(format!("env-{group}-{}", chip.full_path));
    let title: SharedString = tooltip_title.into();
    let full: SharedString = chip.full_path.clone().into();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(0x2a2f37))
        .bg(rgb(0x22262d))
        .hover(|s| s.border_color(color))
        .child(div().size(px(7.0)).rounded_full().bg(color))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0xcdd3db))
                .child(chip.label.clone()),
        )
        .tooltip(move |_window, app| {
            app.new(|_| HelpTooltip {
                title: title.clone(),
                lines: vec![full.clone()],
            })
            .into()
        })
        .tooltip_show_delay(Duration::from_millis(200))
}

fn env_help() -> [&'static str; 3] {
    [
        "多环境检测：npm 枚举本机全部全局 prefix（默认 + nvm/fnm + 指纹扫描），python 枚举全部解释器（py 启动器 + 各安装根目录 + PATH）——每个环境装的包都纳入审计。",
        "项目内 .venv 不扫（项目依赖不是全局工具）；各环境自带的 pip/setuptools/wheel 引导设施不参与审计。",
        "悬停每枚标签查看该环境的完整路径。",
    ]
}

/// 档位筛选指示标签：点击恢复显示全部档位。
fn filter_chip(v: Verdict, this: Entity<AppState>) -> impl IntoElement {
    let color = verdict_color(v);
    div()
        .id("clear-verdict-filter")
        .cursor_pointer()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(color)
        .bg(rgb(0x22262d))
        .text_xs()
        .text_color(color)
        .child(format!("只看：{} ×", verdict_cn(v)))
        .on_click(
            move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                this.update(app, |state: &mut AppState, cx| {
                    state.verdict_filter = None;
                    state.rebuild();
                    cx.notify();
                });
            },
        )
}

/// 已忽略包标签：点击即移出忽略列表并重跑审计（恢复该包的判定与展示）。
fn ignored_chip(name: &str, this: Entity<AppState>) -> impl IntoElement {
    let n = name.to_string();
    div()
        .id(SharedString::from(format!("ignored-{n}")))
        .cursor_pointer()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(0x5a4a1e))
        .bg(rgb(0x2b2416))
        .text_xs()
        .text_color(rgb(0xd9c58a))
        .child(format!("{n} ×"))
        .on_click(
            move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                // on_click 是 Fn（可多次触发），String 先复制再移交内层闭包
                let n = n.clone();
                this.update(app, move |state: &mut AppState, cx| {
                    state.ignored.retain(|x| x != &n);
                    state.start_audit(cx);
                    cx.notify();
                });
            },
        )
}

/// 警告文本归属的信号（按 core 警告的 `[tag]` 前缀）：etw / signal → Etw，
/// imports → Import，atime → Atime；npm（残骸——操作条已单独处理）与
/// store（库故障）无对应信号 → None，归入「其他」。
fn warning_signal(w: &str) -> Option<SignalSource> {
    let rest = w.strip_prefix('[')?;
    let tag = rest.split_once(']')?.0;
    match tag {
        "etw" | "signal" => Some(SignalSource::Etw),
        "imports" => Some(SignalSource::Import),
        "atime" => Some(SignalSource::Atime),
        _ => None,
    }
}

/// 诊断卡片（琥珀色，与原警告横幅同一配色）：text 已是翻译后的中文。
fn warning_card(text: impl Into<SharedString>) -> Div {
    div()
        .p_2()
        .rounded_md()
        .bg(rgb(0x2b2416))
        .border_1()
        .border_color(rgb(0x4a3d1a))
        .text_xs()
        .line_height(relative(1.5))
        .text_color(rgb(0xd9c58a))
        .child(text.into())
}

/// 证据链信号总览（常驻右栏底部，选中包与否都显示）：按来源分三组——
/// shell 足迹（历史 / 配置 / 别名 / 还原命令）、执行观测（ETW / import /
/// 访问时间）、长期趋势（月度快照）。**组名单独一行、组内芯片自成一行**——
/// 窄栏换行不再把不同组混到一行。审计诊断警告按 `[tag]` 归入对应信号组
/// 下方（琥珀卡片），ETW 未开启的行动指引挂在执行观测组；无对应信号的
/// 归「其他」。辅助信号（不直接定档：别名 / 还原命令 / 长期未用）文字调暗。
/// 每枚 = 色点 + 名称 + ？（一句话说明）；badge 配色与此一致（source_palette）。
fn signal_overview(warnings: &[String], etw_on: bool) -> impl IntoElement {
    const GROUPS: &[(&str, &[SignalSource])] = &[
        (
            "Shell 足迹",
            &[
                SignalSource::History,
                SignalSource::ShellConfig,
                SignalSource::Alias,
                SignalSource::Wrapped,
            ],
        ),
        (
            "执行观测",
            &[SignalSource::Etw, SignalSource::Import, SignalSource::Atime],
        ),
        ("长期趋势", &[SignalSource::Drift]),
    ];
    let mut col = div().flex().flex_col().gap_1().child(
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x757d87))
                    .child("信号与诊断"),
            )
            .child(help_icon(
                "help-signals".into(),
                "信号与诊断",
                &[
                    "各路信号按来源分组，每枚「？」看一句话说明。",
                    "信号组下方的琥珀卡片是审计诊断（对应信号的降级 / 折扣），不影响其余信号的判定。",
                    "警告意味着证据覆盖不完整——清理前请先补齐对应信号源。",
                ],
            )),
    );
    for (label, sources) in GROUPS {
        col = col.child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0x757d87))
                .child(*label),
        );
        let mut row = div().flex().flex_wrap().gap_2();
        for s in *sources {
            // 辅助信号（不直接定档）调暗文字，与主信号拉开层级
            let aux = matches!(
                s,
                SignalSource::Alias | SignalSource::Wrapped | SignalSource::Drift
            );
            let (fg, _) = source_palette(*s);
            let (title, lines) = signal_help(*s);
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x2a2f37))
                    .bg(rgb(0x22262d))
                    .child(div().size(px(7.0)).rounded_full().bg(fg))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if aux { rgb(0x9aa2ac) } else { rgb(0xcdd3db) })
                            .child(source_cn(*s)),
                    )
                    .child(help_icon(
                        SharedString::from(format!("sig-{}", s.as_str())),
                        title,
                        &lines,
                    )),
            );
        }
        col = col.child(row);
        // 该组信号的诊断警告，逐条琥珀卡片挂在组下方
        for w in warnings {
            if warning_signal(w).is_some_and(|sig| sources.contains(&sig)) {
                col = col.child(warning_card(warning_cn(w)));
            }
        }
        // ETW 未开启的中文行动指引挂在执行观测组
        if !etw_on && sources.contains(&SignalSource::Etw) {
            col = col.child(warning_card(
                "ETW 执行日志未开启：以管理员身份运行 etw-capture install（一次部署、登录自启、常驻采集），即可补上「被脚本 / 程序调用」的执行证据；未开启时判定依赖历史 + 访问时间。",
            ));
        }
    }
    // 无对应信号的警告（store 库故障 / npm 残骸——残骸另有操作条）归「其他」
    let other: Vec<&String> = warnings
        .iter()
        .filter(|w| warning_signal(w).is_none())
        .collect();
    if !other.is_empty() {
        col = col.child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0x757d87))
                .child("其他"),
        );
        for w in other {
            col = col.child(warning_card(warning_cn(w)));
        }
    }
    col
}

// --- 表格 ---

/// 表头：行首是全选勾选框（作用于当前筛选可见的可卸载行），其余列点击排序。
/// 勾选框后的 3px 空槽与数据行的选中色条对齐（gpui 无分侧边框色，色条用子元素实现）。
fn table_header(
    rows: &[Row],
    sort_key: SortKey,
    sort_dir: SortDir,
    checked: &HashSet<usize>,
    this: Entity<AppState>,
) -> impl IntoElement {
    let removable: Vec<usize> = rows
        .iter()
        .filter(|r| r.removable)
        .map(|r| r.index)
        .collect();
    let all_checked = !removable.is_empty() && removable.iter().all(|i| checked.contains(i));
    let this_all = this.clone();
    div()
        .flex()
        .items_center()
        .px_4()
        .py_2()
        .bg(rgb(0x22262d))
        .border_b_1()
        .border_color(rgb(0x2a2f37))
        .text_xs()
        .text_color(rgb(0x757d87))
        .child(
            div()
                .id("chk-all")
                .w(px(28.0))
                .flex()
                .justify_center()
                .child(checkbox_box(all_checked))
                .when(!removable.is_empty(), |el| {
                    el.cursor_pointer().on_click(
                        move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                            let idxs = removable.clone();
                            this_all.update(app, move |state: &mut AppState, cx| {
                                // 全选/反选只作用于当前可见行：已是全选则清掉，否则补齐
                                if idxs.iter().all(|i| state.removal_selected.contains(i)) {
                                    for i in &idxs {
                                        state.removal_selected.remove(i);
                                    }
                                } else {
                                    for i in idxs {
                                        state.removal_selected.insert(i);
                                    }
                                }
                                state.removal_confirm = false;
                                cx.notify();
                            });
                        },
                    )
                }),
        )
        .child(div().w(px(3.0)))
        .child(sortable_header(
            "包名",
            SortKey::Name,
            sort_key,
            sort_dir,
            230.0,
            this.clone(),
        ))
        .child(sortable_header(
            "管理器",
            SortKey::Manager,
            sort_key,
            sort_dir,
            80.0,
            this.clone(),
        ))
        // 环境列不参与排序（短名仅是展示标注，完整路径才是归属铁证）
        .child(div().w(px(76.0)).child("环境"))
        .child(sortable_header(
            "判定",
            SortKey::Verdict,
            sort_key,
            sort_dir,
            110.0,
            this.clone(),
        ))
        .child(sortable_header(
            "输入次数",
            SortKey::Typed,
            sort_key,
            sort_dir,
            80.0,
            this.clone(),
        ))
        .child(sortable_header(
            "最近运行",
            SortKey::LastRun,
            sort_key,
            sort_dir,
            90.0,
            this.clone(),
        ))
        .child(sortable_header(
            "大小",
            SortKey::Size,
            sort_key,
            sort_dir,
            80.0,
            this,
        ))
}

/// 单个可排序表头：点击切换排序列；同列再点一次反向。
fn sortable_header(
    label: &str,
    key: SortKey,
    cur_key: SortKey,
    cur_dir: SortDir,
    width: f32,
    this: Entity<AppState>,
) -> impl IntoElement {
    let active = key == cur_key;
    let arrow = if active {
        match cur_dir {
            SortDir::Asc => " ▲",
            SortDir::Desc => " ▼",
        }
    } else {
        ""
    };
    div()
        .id(SharedString::from(format!("sort-{key:?}")))
        .cursor_pointer()
        .w(px(width))
        .flex()
        .items_center()
        .rounded_md()
        .text_xs()
        .text_color(if active { rgb(0x7aa2f7) } else { rgb(0x757d87) })
        .hover(|s| s.text_color(rgb(0x9aa2ac)))
        .child(SharedString::from(format!("{label}{arrow}")))
        .on_click(
            move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                this.update(app, move |state: &mut AppState, cx| {
                    if state.sort_key == key {
                        state.sort_dir = match state.sort_dir {
                            SortDir::Asc => SortDir::Desc,
                            SortDir::Desc => SortDir::Asc,
                        };
                    } else {
                        state.sort_key = key;
                        state.sort_dir = SortDir::Asc;
                    }
                    state.rebuild();
                    cx.notify();
                });
            },
        )
}

/// 表格主体：加载中显示转圈动画，否则渲染数据行。
fn table_rows(
    loading: bool,
    rows: &[Row],
    selected_idx: Option<usize>,
    checked: &HashSet<usize>,
    this: Entity<AppState>,
) -> Vec<gpui::AnyElement> {
    if loading {
        vec![div()
            .flex()
            .justify_center()
            .py_8()
            .child(audit_spinner())
            .into_any_element()]
    } else {
        rows.iter()
            .enumerate()
            .map(|(pos, r)| {
                let checked = checked.contains(&r.index);
                package_row(r, pos, selected_idx, checked, this.clone()).into_any_element()
            })
            .collect()
    }
}

/// 转圈加载动画：8 个圆点围成环，亮度相位随帧推进形成追逐效果。
/// 用不透明度而不是 transform——本版 gpui 没有 Div 旋转 API；闭包每帧
/// 在干净的基元素上重建 children（AnimationElement 每帧来自新的 render）。
fn audit_spinner() -> impl IntoElement {
    div().size(px(22.0)).with_animation(
        "audit-spinner-anim",
        Animation::new(Duration::from_millis(900)).repeat(),
        |ring, delta| {
            const N: f32 = 8.0;
            let mut dots = div().relative().size_full();
            for i in 0..8 {
                let frac = i as f32 / N;
                let angle = frac * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                // 相位差：领头点最亮，身后渐暗
                let phase = (delta - frac + 1.0) % 1.0;
                let opacity = 0.15 + 0.85 * (1.0 - phase);
                dots = dots.child(
                    div()
                        .absolute()
                        .left(px(9.25 + 7.25 * angle.cos()))
                        .top(px(9.25 + 7.25 * angle.sin()))
                        .size(px(3.5))
                        .rounded_full()
                        .bg(rgb(0x7aa2f7))
                        .opacity(opacity),
                );
            }
            ring.child(dots)
        },
    )
}

/// 勾选框（视觉件）：13px 方框，选中蓝底白勾；点击行为由外层单元格承担。
fn checkbox_box(checked: bool) -> impl IntoElement {
    div()
        .flex()
        .size(px(13.0))
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(if checked {
            rgb(0x7aa2f7)
        } else {
            rgb(0x3a4049)
        })
        .bg(if checked {
            rgb(0x7aa2f7)
        } else {
            rgba(0x00000000)
        })
        .when(checked, |el| {
            el.child(
                div()
                    .text_size(px(9.0))
                    .line_height(relative(1.0))
                    .text_color(rgb(0x14171b))
                    .child("✓"),
            )
        })
}

/// 单个数据行：斑马纹底色、判定色点、管理器标签；选中时左侧显示 3px 档位色条
/// （常驻占位保持列对齐）。`pos` 是当前视图内的位置（斑马纹用），与 `row.index`
/// （results 索引，选中态用）不同。行首勾选框与「点行看证据」是并列的可点区域
/// （gpui 点击沿祖先冒泡，拆开避免勾选时误触行选中）。
fn package_row(
    row: &Row,
    pos: usize,
    selected_idx: Option<usize>,
    checked: bool,
    this: Entity<AppState>,
) -> impl IntoElement {
    let is_selected = selected_idx == Some(row.index);
    let idx = row.index;
    let color = verdict_color(row.verdict);
    let zebra = if pos.is_multiple_of(2) {
        rgb(0x14171b)
    } else {
        rgb(0x191d22)
    };
    div()
        .flex()
        .items_center()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(rgb(0x242932))
        .bg(if is_selected { rgb(0x2a3138) } else { zebra })
        .hover(|s| s.bg(rgb(0x22262d)))
        // 勾选格：core 给得出卸载命令的包才有框（go 无清单包不可卸），热区随行高
        .child(
            div()
                .id(("chk", idx))
                .w(px(28.0))
                .self_stretch()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .when(row.removable, |el| {
                    el.cursor_pointer()
                        .hover(|s| s.bg(rgb(0x2a3138)))
                        .child(checkbox_box(checked))
                        .on_click({
                            // 勾选闭包与行选中闭包都要用 this，各持一份
                            let this = this.clone();
                            move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                                this.update(app, move |state: &mut AppState, cx| {
                                    state.toggle_removal(idx);
                                    cx.notify();
                                });
                            }
                        })
                }),
        )
        .child(
            div()
                .id(("pkg-row", idx))
                .cursor_pointer()
                .flex()
                .flex_1()
                .items_center()
                .on_click(
                    move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                        this.update(app, move |state: &mut AppState, cx| {
                            state.selected = Some(idx);
                            cx.notify();
                        });
                    },
                )
                .child(
                    div()
                        .w(px(3.0))
                        .bg(if is_selected { color } else { rgba(0x00000000) }),
                )
                .child(
                    div()
                        .w(px(230.0))
                        .flex()
                        .items_center()
                        .gap_1()
                        .overflow_hidden()
                        .child(row.name.clone()),
                )
                .child(
                    div()
                        .w(px(80.0))
                        .text_xs()
                        .text_color(rgb(0x9aa2ac))
                        .child(row.manager.clone()),
                )
                // 归属环境短名（单环境也显示）；无环境语义的管理器显示「—」，
                // 完整路径仍是证据面板的归属铁证
                .child(
                    div()
                        .w(px(76.0))
                        .text_xs()
                        .text_color(rgb(0x9aa2ac))
                        .child(row.env_label.clone().unwrap_or_else(|| "—".into())),
                )
                .child(
                    div()
                        .w(px(110.0))
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().size(px(8.0)).rounded_full().bg(color))
                        .child(div().text_color(color).child(verdict_cn(row.verdict))),
                )
                .child(
                    div()
                        .w(px(80.0))
                        .text_color(rgb(0x9aa2ac))
                        .child(row.typed.clone()),
                )
                .child(
                    div()
                        .w(px(90.0))
                        .text_color(rgb(0x9aa2ac))
                        .child(row.last_run.clone()),
                )
                .child(
                    div()
                        .w(px(80.0))
                        .text_color(rgb(0x9aa2ac))
                        .child(row.size.clone()),
                ),
        )
}

// --- 证据面板 ---

/// 证据面板：包名 + 档位副标题（带五档 ？）、证据卡（badge + ？）、观察期标注、
/// 忽略按钮。未选中时显示引导文案。
fn evidence_panel(row: Option<&Row>, this: Entity<AppState>) -> impl IntoElement {
    let mut col = div().flex().flex_col().gap_2().overflow_hidden();
    match row {
        Some(r) => {
            col = col
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(r.name.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().text_xs().text_color(rgb(0x757d87)).child(format!(
                            "{} {}（{}），置信度 {}",
                            r.manager,
                            verdict_cn(r.verdict),
                            r.verdict.as_str(),
                            r.confidence
                        )))
                        .child(help_icon(
                            "help-verdict".into(),
                            "五档判定",
                            &verdict_tiers_help(),
                        )),
                )
                // 安装位置是归属环境的铁证（多环境同名包靠它分清卸载对象）；
                // 等宽字体呈现——是文件系统路径，不是正文；path_display 在
                // 反斜杠后给断点，长路径按目录边界折行（裸串会在 C: 后碎断）
                .when_some(r.install_path.clone(), |el, p| {
                    el.child(
                        div()
                            .text_xs()
                            .font_family("Consolas")
                            .text_color(rgb(0x757d87))
                            .child(path_display(&p)),
                    )
                });
            for e in &r.evidence {
                col = col.child(evidence_card(e));
            }
            if r.evidence.is_empty() {
                col = col.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x757d87))
                        .p_2()
                        .rounded_md()
                        .bg(rgb(0x22262d))
                        .child("审计窗口内没有查到任何使用记录。"),
                );
            }
            // 新装观察期：不是信号证据，单独一张琥珀色卡片
            if r.observation {
                col = col.child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(rgb(0x2b2416))
                        .border_1()
                        .border_color(rgb(0x5a4a1e))
                        .text_xs()
                        .text_color(rgb(0xd9c58a))
                        .child("新装观察期：安装不满一个观察窗口，暂不进入清理候选。"),
                );
            }
            // 忽略此包：对应 CLI --ignore（会话级）。重跑后该包整包退出审计，
            // 选中态随之清空；工具条的「已忽略」标签可随时恢复
            let name = r.name.to_string();
            col = col.child(
                div()
                    .id("ignore-this-pkg")
                    .cursor_pointer()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x2a2f37))
                    .bg(rgb(0x22262d))
                    .text_xs()
                    .text_color(rgb(0x9aa2ac))
                    .child("忽略此包（退出本次会话的审计）")
                    .on_click(
                        move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                            // on_click 是 Fn（可多次触发），String 先复制再移交内层闭包
                            let name = name.clone();
                            this.update(app, move |state: &mut AppState, cx| {
                                if !state.ignored.iter().any(|n| n == &name) {
                                    state.ignored.push(name);
                                }
                                // 该包即将从结果中消失，选中态指向的索引会失效
                                state.selected = None;
                                state.start_audit(cx);
                                cx.notify();
                            });
                        },
                    ),
            );
        }
        None => {
            // 空态不放占位说明——下方常驻的「信号与诊断」总览即是内容
        }
    }
    col
}

/// 单条证据卡：来源 badge（源色）+ 「？」说明 + 细节文案。
fn evidence_card(e: &EvidenceLine) -> impl IntoElement {
    let (fg, bg) = source_palette(e.source);
    let (title, lines) = signal_help(e.source);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .bg(rgb(0x22262d))
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .bg(bg)
                        .text_xs()
                        .text_color(fg)
                        .child(source_cn(e.source)),
                )
                .child(help_icon(
                    SharedString::from(format!("ev-{}", e.source.as_str())),
                    title,
                    &lines,
                )),
        )
        .child(
            div()
                .text_xs()
                .line_height(relative(1.5))
                .text_color(rgb(0xcdd3db))
                .child(e.text.clone()),
        )
}

/// 勾选卸载的悬停说明。
fn remove_help() -> Vec<&'static str> {
    vec![
        "卸载命令由 core 按包所在环境生成（与 CLI clean 同源）：pip 定位到所在解释器、npm 非 prefix 显式 --prefix，不会卸错位置。",
        "第一次点「移除所选」只进入确认态（按钮变红），再点同一按钮才执行；执行完自动重新审计核验。",
        "安全软件可能吞掉卸载进程的输出，「卸没卸掉」以审计后的清单为准——未消失的包会在这里点名。",
    ]
}

/// 底部卸载操作条：有勾选或刚执行完卸载时出现。两步确认防误删——
/// 第一次点「移除所选」只进入确认态（按钮变红、点名数量），再点同一按钮才执行。
fn removal_bar(
    count: usize,
    size_note: SharedString,
    non_unused: usize,
    confirm: bool,
    removing: bool,
    status: Option<SharedString>,
    this: Entity<AppState>,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_5()
        .py_2()
        .border_t_1()
        .border_color(rgb(0x2a2f37))
        .bg(rgb(0x181b20))
        .when(count > 0, |el| {
            el.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xcdd3db))
                    .child(SharedString::from(format!(
                        "已勾选 {count} 个包{size_note}"
                    ))),
            )
            .when(non_unused > 0, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xd9c58a))
                        .child(format!("含 {non_unused} 个非「未用」档，请先核对判定依据")),
                )
            })
        })
        .child(div().flex_1())
        .children(status.map(|s| div().text_xs().text_color(rgb(0x9aa2ac)).child(s)))
        .when(!removing && count > 0, |el| {
            el.child(if confirm {
                div()
                    .id("confirm-remove")
                    .cursor_pointer()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0xe24b4a))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x14171b))
                    .child(SharedString::from(format!(
                        "确认移除 {count} 个包（不可撤销）"
                    )))
                    .on_click({
                        let this = this.clone();
                        move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                            this.update(app, |state: &mut AppState, cx| {
                                state.remove_selected(cx);
                                cx.notify();
                            });
                        }
                    })
            } else {
                div()
                    .id("arm-remove")
                    .cursor_pointer()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x5a3030))
                    .bg(rgb(0x22262d))
                    .text_xs()
                    .text_color(rgb(0xe24b4a))
                    .child("移除所选")
                    .on_click({
                        let this = this.clone();
                        move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                            this.update(app, |state: &mut AppState, cx| {
                                state.removal_confirm = true;
                                cx.notify();
                            });
                        }
                    })
            })
            .child(
                div()
                    .id("cancel-remove")
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(0x757d87))
                    .hover(|s| s.text_color(rgb(0x9aa2ac)))
                    .child("取消")
                    .on_click({
                        let this = this.clone();
                        move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                            this.update(app, |state: &mut AppState, cx| {
                                state.removal_selected.clear();
                                state.removal_confirm = false;
                                cx.notify();
                            });
                        }
                    }),
            )
        })
        .child(help_icon("help-remove".into(), "勾选卸载", &remove_help()))
}

/// 卸载残骸操作条（残骸存在时出现在警告横幅下方）：说明 + 两步确认删除。
/// 「残骸」= 包已卸载但启动器文件（X / X.cmd / X.ps1）仍留在 PATH 目录，
/// 敲命令会报错；删除命令与 clean 脚本的逐文件删除行同源。
fn shims_bar(
    shims: &[OrphanShim],
    confirm: bool,
    removing: bool,
    status: Option<SharedString>,
    this: Entity<AppState>,
) -> impl IntoElement {
    let files: usize = shims.iter().map(|s| s.files.len()).sum();
    let mut names: Vec<&str> = shims.iter().map(|s| s.bin.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_5()
        .py_2()
        .bg(rgb(0x211c12))
        .border_b_1()
        .border_color(rgb(0x5a4a1e))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xe0af68))
                        .child("卸载残骸"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xd9c58a))
                        .child(SharedString::from(format!(
                            "{} 组 / {} 个文件：{}（包已卸载，启动器仍在 PATH 目录，敲命令会报错）",
                            shims.len(),
                            files,
                            names.join("、")
                        ))),
                )
                .child(help_icon(
                    "help-shims".into(),
                    "卸载残骸",
                    &[
                        "npm 卸载包后，全局 bin 目录里的启动器（X / X.cmd / X.ps1）可能残留。",
                        "「删除残骸文件」逐个移除（与 clean 脚本的逐文件删除行同一来源）；完成后自动重审计核验清零。",
                        "想先审阅再动手：用工具条「导出脚本」生成带逐文件删除行的 PowerShell 脚本。",
                    ],
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .children(status.map(|s| div().text_xs().text_color(rgb(0x9aa2ac)).child(s)))
                .when(!removing, |el| {
                    el.child(if confirm {
                        div()
                            .id("confirm-shims")
                            .cursor_pointer()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0xe24b4a))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0x14171b))
                            .child(SharedString::from(format!(
                                "确认删除 {files} 个文件（不可撤销）"
                            )))
                            .on_click({
                                let this = this.clone();
                                move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                                    this.update(app, |state: &mut AppState, cx| {
                                        state.remove_shims(cx);
                                        cx.notify();
                                    });
                                }
                            })
                    } else {
                        div()
                            .id("arm-shims")
                            .cursor_pointer()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(0x5a3030))
                            .bg(rgb(0x22262d))
                            .text_xs()
                            .text_color(rgb(0xe24b4a))
                            .child("删除残骸文件")
                            .on_click({
                                let this = this.clone();
                                move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                                    this.update(app, |state: &mut AppState, cx| {
                                        state.shim_confirm = true;
                                        cx.notify();
                                    });
                                }
                            })
                    })
                    .child(
                        div()
                            .id("cancel-shims")
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(0x757d87))
                            .hover(|s| s.text_color(rgb(0x9aa2ac)))
                            .child("取消")
                            .on_click({
                                let this = this.clone();
                                move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                                    this.update(app, |state: &mut AppState, cx| {
                                        state.shim_confirm = false;
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                }),
        )
}

/// 右栏证据链（默认面板）：判定依据逐条列出；底部常驻「信号与诊断」总览
/// （含归组的审计警告——警告随选中状态始终可见，不再横贯顶部）。
fn evidence_column(
    selected: Option<&Row>,
    warnings: &[String],
    etw_on: bool,
    this: Entity<AppState>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xe3e6ea))
                        .child("证据链"),
                )
                .child(help_icon(
                    "help-evidence".into(),
                    "证据链",
                    &[
                        "判定依据逐条列出：每条 = 信号来源 badge + 细节 + 最近时间。",
                        "悬停每条的「？」可查看该路信号的原理、判定方式与局限。",
                        "判定规则按「活跃 → 间接 → 偶用 → 集成 → 未用」的顺序命中即停（详见副标题的「？」）。",
                    ],
                )),
        )
        .child(evidence_panel(selected, this))
        // 选中包与空态之间留出分隔，总览常驻底部
        .child(div().border_t_1().border_color(rgb(0x2a2f37)))
        .child(signal_overview(warnings, etw_on))
}

/// 长期对比面板（右栏）：月度快照两两对比，看 active/unused 两档的增删。
fn drift_panel(
    snapshots: &[String],
    old: &Option<String>,
    new: &Option<String>,
    result: &Option<Result<Vec<DriftDiff>, String>>,
    busy: bool,
    this: Entity<AppState>,
) -> Div {
    let mut col = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xe3e6ea))
                        .child("长期对比"),
                )
                .child(help_icon(
                    "help-drift".into(),
                    "长期对比",
                    &[
                        "每月首次审计自动存一份月度快照；选两个月份对比 active / unused 两档的新增与移除。",
                        "中间档（间接 / 偶用 / 集成）随观察窗口波动，不参与对比，避免淹没真实增删。",
                        "与 CLI drift 子命令同一实现（store::drift_diff）。",
                    ],
                ))
                .child(
                    div()
                        .id("drift-refresh")
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(0x757d87))
                        .hover(|s| s.text_color(rgb(0x9aa2ac)))
                        .child("刷新")
                        .on_click({
                            let this = this.clone();
                            move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                                this.update(app, |state: &mut AppState, cx| {
                                    state.load_snapshots(cx);
                                    cx.notify();
                                });
                            }
                        }),
                ),
        );
    if snapshots.len() < 2 {
        col = col.child(
            div()
                .text_xs()
                .line_height(relative(1.5))
                .text_color(rgb(0x9aa2ac))
                .child("快照不足：每月首次审计自动存一份月度快照，次月起就能对比两月之间的增删。"),
        );
        return col;
    }
    col = col.child(snapshot_row(
        "旧",
        snapshots,
        old.clone(),
        true,
        this.clone(),
    ));
    col = col.child(snapshot_row(
        "新",
        snapshots,
        new.clone(),
        false,
        this.clone(),
    ));
    col = col.child(if busy {
        div()
            .id("drift-busy")
            .text_xs()
            .text_color(rgb(0x757d87))
            .child("处理中…")
    } else if old.is_some() && new.is_some() {
        div()
            .id("run-drift")
            .cursor_pointer()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(rgb(0x2a3138))
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(0x7aa2f7))
            .child("对比")
            .on_click({
                let this = this.clone();
                move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                    this.update(app, |state: &mut AppState, cx| {
                        state.run_drift(cx);
                        cx.notify();
                    });
                }
            })
    } else {
        div()
            .id("drift-hint")
            .text_xs()
            .text_color(rgb(0x757d87))
            .child("各选一个旧 / 新快照后点「对比」")
    });
    match result {
        Some(Ok(diffs)) => {
            let lines = drift_lines(diffs);
            if lines.iter().all(|(_, a, r)| a == "—" && r == "—") {
                col = col.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x9aa2ac))
                        .child("两个快照之间无增删"),
                );
            }
            for (v, added, removed) in lines {
                col = col.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .p_2()
                        .rounded_md()
                        .bg(rgb(0x1c2026))
                        .border_1()
                        .border_color(rgb(0x2a2f37))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(0xcdd3db))
                                .child(SharedString::from(format!("{v}档"))),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x8cc26e))
                                .child(SharedString::from(format!("新增：{added}"))),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xe24b4a))
                                .child(SharedString::from(format!("移除：{removed}"))),
                        ),
                );
            }
        }
        Some(Err(e)) => {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xe24b4a))
                    .child(SharedString::from(format!("对比失败：{e}"))),
            );
        }
        None => {}
    }
    col
}

/// 长期对比的一行快照芯片（旧 / 新）：点击选择，同侧再点取消；新旧不得同选。
fn snapshot_row(
    label: &'static str,
    snapshots: &[String],
    current: Option<String>,
    is_old: bool,
    this: Entity<AppState>,
) -> impl IntoElement {
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .child(div().text_xs().text_color(rgb(0x757d87)).child(label));
    for s in snapshots {
        let active = current.as_deref() == Some(s.as_str());
        let s_clone = s.clone();
        let this = this.clone();
        row = row.child(
            div()
                .id(SharedString::from(format!("snap-{label}-{s}")))
                .cursor_pointer()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .border_1()
                .border_color(if active { rgb(0x33415e) } else { rgb(0x2a2f37) })
                .bg(if active { rgb(0x2a3138) } else { rgb(0x1c2026) })
                .text_xs()
                .text_color(if active { rgb(0x7aa2f7) } else { rgb(0x9aa2ac) })
                .child(s.clone())
                .on_click(move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                    let s = s_clone.clone();
                    this.update(app, move |state: &mut AppState, cx| {
                        // 新旧不得同选：同侧再点取消，撞上对面已选的月份则清对面
                        if is_old {
                            state.drift_old = if state.drift_old.as_deref() == Some(s.as_str()) {
                                None
                            } else {
                                Some(s.clone())
                            };
                            if state.drift_new == state.drift_old {
                                state.drift_new = None;
                            }
                        } else {
                            state.drift_new = if state.drift_new.as_deref() == Some(s.as_str()) {
                                None
                            } else {
                                Some(s.clone())
                            };
                            if state.drift_new == state.drift_old {
                                state.drift_old = None;
                            }
                        }
                        state.drift_result = None;
                        cx.notify();
                    });
                }),
        );
    }
    row
}

/// 钩子管理面板（右栏）：安装 / 卸载 / 状态（import 级使用证据的采集端）。
fn hooks_panel(
    status: Option<&HooksStatus>,
    busy: bool,
    confirm: bool,
    feedback: Option<SharedString>,
    this: Entity<AppState>,
) -> Div {
    let mut col = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xe3e6ea))
                        .child("钩子管理"),
                )
                .child(help_icon(
                    "help-hooks".into(),
                    "钩子管理",
                    &[
                        "钩子补上 import 级使用证据：python 经 .pth 钩子记录 import 的顶层包，node 经 NODE_OPTIONS 记录 require。",
                        "让「被脚本 / 程序调用」的包不被误判为未用；与 CLI hooks 子命令同一实现（core hooks）。",
                        "import 日志只是使用证据，清理动作不会触碰它。",
                    ],
                )),
        )
        // 反馈可能含多行（[hooks] 诊断行）：gpui 不渲染 \n，按行拆 div
        .children(feedback.map(|f| {
            div().flex().flex_col().children(
                f.lines()
                    .map(|l| div().text_xs().text_color(rgb(0x9aa2ac)).child(l.to_string()))
                    .collect::<Vec<_>>(),
            )
        }));
    match status {
        None => {
            col = col.child(div().text_xs().text_color(rgb(0x757d87)).child(if busy {
                "正在探测…"
            } else {
                "尚未探测"
            }));
        }
        Some(st) => {
            for line in hooks_lines(st) {
                col = col.child(
                    div()
                        .text_xs()
                        .line_height(relative(1.5))
                        .text_color(rgb(0xcdd3db))
                        .child(SharedString::from(line)),
                );
            }
        }
    }
    // 操作行：安装（单次点击）/ 卸载（两步确认）/ 刷新；执行中整体换为过程态
    let actions = div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("hooks-install")
                .cursor_pointer()
                .px_3()
                .py_1()
                .rounded_md()
                .bg(rgb(0x233a2a))
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0x8cc26e))
                .child("安装钩子")
                .on_click({
                    let this = this.clone();
                    move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                        this.update(app, |state: &mut AppState, cx| {
                            state.install_hooks(cx);
                            cx.notify();
                        });
                    }
                }),
        )
        .child(if confirm {
            div()
                .id("hooks-confirm-uninstall")
                .cursor_pointer()
                .px_3()
                .py_1()
                .rounded_md()
                .bg(rgb(0xe24b4a))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0x14171b))
                .child("确认卸载（移除全部钩子）")
                .on_click({
                    let this = this.clone();
                    move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                        this.update(app, |state: &mut AppState, cx| {
                            state.uninstall_hooks(cx);
                            cx.notify();
                        });
                    }
                })
        } else {
            div()
                .id("hooks-arm-uninstall")
                .cursor_pointer()
                .px_3()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(rgb(0x5a3030))
                .bg(rgb(0x22262d))
                .text_xs()
                .text_color(rgb(0xe24b4a))
                .child("卸载钩子")
                .on_click({
                    let this = this.clone();
                    move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                        this.update(app, |state: &mut AppState, cx| {
                            state.hooks_confirm = true;
                            cx.notify();
                        });
                    }
                })
        })
        .child(
            div()
                .id("hooks-refresh")
                .cursor_pointer()
                .text_xs()
                .text_color(rgb(0x757d87))
                .hover(|s| s.text_color(rgb(0x9aa2ac)))
                .child("刷新")
                .on_click({
                    let this = this.clone();
                    move |_e: &ClickEvent, _w: &mut Window, app: &mut App| {
                        this.update(app, |state: &mut AppState, cx| {
                            state.refresh_hooks(cx);
                            cx.notify();
                        });
                    }
                }),
        );
    col = col.child(if busy {
        div().text_xs().text_color(rgb(0x757d87)).child("处理中…")
    } else {
        actions
    });
    col
}

// --- root view ---

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.summary.clone();
        let status = self.status_line.clone();
        // selected 是 results 索引；排序/筛选后行位置会变，必须按 index 查找而非按位置取
        let selected_row = self
            .selected
            .and_then(|i| row_by_index(&self.rows, i).cloned());
        let rows = self.rows.clone();
        let loading = self.loading;
        let verdict_filter = self.verdict_filter;
        let manager_filter = self.manager_filter;
        let managers = manager_counts(&self.results);
        let envs = self.detected_envs.clone();
        let sort_key = self.sort_key;
        let sort_dir = self.sort_dir;
        let etw_on = self.etw_on;
        let warnings = self.warnings.clone();
        let since_days = self.since_days;
        let ignored = self.ignored.clone();
        let panel = self.right_panel;
        let orphan_shims = self.orphan_shims.clone();
        let shim_confirm = self.shim_confirm;
        let shim_removing = self.shim_removing;
        let shim_status = self.shim_status.clone();
        let snapshots = self.snapshots.clone();
        let drift_old = self.drift_old.clone();
        let drift_new = self.drift_new.clone();
        let drift_result = self.drift_result.clone();
        let drift_busy = self.drift_busy;
        let hooks_status = self.hooks_status.clone();
        let hooks_busy = self.hooks_busy;
        let hooks_confirm = self.hooks_confirm;
        let hooks_feedback = self.hooks_feedback.clone();
        // 卸载操作条数据：勾选数、合计大小（有未知就整个不显示）、非「未用」档提示
        let removal_count = self.removal_selected.len();
        let mut size_total = 0u64;
        let mut size_known = removal_count > 0;
        let mut non_unused = 0usize;
        for (i, r) in self.results.iter().enumerate() {
            if self.removal_selected.contains(&i) {
                if r.verdict != Verdict::Unused {
                    non_unused += 1;
                }
                match r.package.size_bytes {
                    Some(b) => size_total += b,
                    None => size_known = false,
                }
            }
        }
        let size_note = if removal_count > 0 && size_known {
            format!("，合计 {:.1} MB", size_total as f64 / 1024.0 / 1024.0)
        } else {
            String::new()
        };
        let removal_confirm = self.removal_confirm;
        let removing = self.removing;
        let removal_status = self.removal_status.clone();
        // 捕获自身句柄，供事件闭包更新状态（GPUI 0.3.12 惯用模式）
        let this = cx.entity();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x14171b))
            .text_color(rgb(0xe3e6ea))
            .child(title_bar(status))
            .child(dashboard(&summary, verdict_filter, this.clone()))
            .child(manager_bar(&managers, manager_filter, this.clone()))
            // 检测环境行（多环境时才出现）：npm prefix / python 解释器一览
            .children(env_bar(&envs))
            .child(toolbar(
                since_days,
                verdict_filter,
                &ignored,
                panel,
                this.clone(),
            ))
            // 审计警告不再横贯顶部：已按 [tag] 归入右栏「信号与诊断」的对应信号组下方
            // 卸载残骸操作条（有残骸才出现）：两步确认删除 + 重审计核验
            .when(!orphan_shims.is_empty(), |el| {
                el.child(shims_bar(
                    &orphan_shims,
                    shim_confirm,
                    shim_removing,
                    shim_status,
                    this.clone(),
                ))
            })
            // 主体：表格 + 证据面板
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(200.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .id("pkg-table")
                            .flex_1()
                            .overflow_y_scroll()
                            .child(table_header(
                                &rows,
                                sort_key,
                                sort_dir,
                                &self.removal_selected,
                                this.clone(),
                            ))
                            .children(table_rows(
                                loading,
                                &rows,
                                selected_row.as_ref().map(|x| x.index),
                                &self.removal_selected,
                                this.clone(),
                            )),
                    )
                    .child(
                        // 右栏随面板切换：证据链（默认）/ 长期对比 / 钩子管理；
                        // 证据 + 常驻信号总览可能超过一屏，允许纵向滚动
                        div()
                            .id("evidence-col")
                            .overflow_y_scroll()
                            .w(px(360.0))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .border_l_1()
                            .border_color(rgb(0x2a2f37))
                            .bg(rgb(0x181b20))
                            .p_4()
                            .child(match panel {
                                RightPanel::Evidence => evidence_column(
                                    selected_row.as_ref(),
                                    &warnings,
                                    etw_on,
                                    this.clone(),
                                ),
                                RightPanel::Drift => drift_panel(
                                    &snapshots,
                                    &drift_old,
                                    &drift_new,
                                    &drift_result,
                                    drift_busy,
                                    this.clone(),
                                ),
                                RightPanel::Hooks => hooks_panel(
                                    hooks_status.as_ref(),
                                    hooks_busy,
                                    hooks_confirm,
                                    hooks_feedback,
                                    this.clone(),
                                ),
                            }),
                    ),
            )
            // 底部卸载操作条（有勾选或刚执行完卸载时出现）
            .when(removal_count > 0 || removal_status.is_some(), |el| {
                el.child(removal_bar(
                    removal_count,
                    size_note.into(),
                    non_unused,
                    removal_confirm,
                    removing,
                    removal_status,
                    this,
                ))
            })
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, gpui::size(px(1180.0), px(760.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // 原生标题栏标题（缺省时标题栏是空的，任务栏/Alt-Tab 也跟着无名）
                titlebar: Some(TitlebarOptions {
                    title: Some("orphan-audit".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|cx| {
                    // 窗口先出现，审计移入后台线程（npm/python 等子进程探测耗时数秒，
                    // 同步执行会让窗口迟迟不出现，观感为"卡死"）
                    let mut state = AppState::loading();
                    state.start_audit(cx);
                    state
                })
            },
        )
        .unwrap();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use orphan_audit_core::model::{Confidence, Manager, Package, Verdict, VerdictResult};
    use std::path::PathBuf;

    fn vr(name: &str, install_path: Option<&str>) -> VerdictResult {
        VerdictResult {
            package: Package {
                name: name.into(),
                manager: Manager::Npm,
                version: None,
                binaries: Vec::new(),
                install_path: install_path.map(PathBuf::from),
                installed_at: None,
                size_bytes: None,
                confidence: Confidence::Exact,
                is_dependency: false,
            },
            verdict: Verdict::Unused,
            interactive_hits: 0,
            last_exec_days_ago: None,
            in_shell_config: false,
            newly_installed: false,
            evidence: Vec::new(),
        }
    }

    #[test]
    fn verify_removal_counts_pairs_not_deduped_names() {
        // 双 prefix 同名包：两份都在新清单 → gone 必须 0（不能按去重后的名字数算）
        let pending = vec![
            (
                "opencli".into(),
                Some(r"C:\a\node_modules\@s\opencli".into()),
            ),
            (
                "opencli".into(),
                Some(r"C:\b\node_modules\@s\opencli".into()),
            ),
        ];
        let results = vec![
            vr("opencli", Some(r"C:\a\node_modules\@s\opencli")),
            vr("opencli", Some(r"C:\b\node_modules\@s\opencli")),
        ];
        let (gone, still) = verify_removal(&pending, &results);
        assert_eq!((gone, still), (0, vec!["opencli".to_string()]));
    }

    #[test]
    fn verify_removal_mixed_and_path_mismatch() {
        // 两份同名卸掉一份（路径对账），另一包未卸
        let pending = vec![
            (
                "opencli".into(),
                Some(r"C:\a\node_modules\@s\opencli".into()),
            ),
            (
                "opencli".into(),
                Some(r"C:\b\node_modules\@s\opencli".into()),
            ),
            ("tokentracker".into(), Some(r"C:\a\node_modules\t".into())),
        ];
        let results = vec![
            vr("opencli", Some(r"C:\b\node_modules\@s\opencli")),
            vr("tokentracker", Some(r"C:\a\node_modules\t")),
        ];
        let (gone, still) = verify_removal(&pending, &results);
        assert_eq!(gone, 1);
        assert_eq!(
            still,
            vec!["opencli".to_string(), "tokentracker".to_string()]
        );
        // 全部消失
        let (gone, still) = verify_removal(&pending, &[]);
        assert_eq!((gone, still), (3, Vec::<String>::new()));
    }

    #[test]
    fn hooks_lines_cover_interpreters_counts_and_log_states() {
        let st = HooksStatus {
            interpreters: vec![
                (PathBuf::from(r"C:\Py313\python.exe"), true),
                (PathBuf::from(r"C:\Py312\python.exe"), false),
            ],
            tool_venvs: (1, 3),
            node_hooked: false,
            log: Some((
                PathBuf::from(r"C:\log\oa-import.txt"),
                ImportLogState::Present(42),
            )),
        };
        let lines = hooks_lines(&st);
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("Py313") && lines[0].contains("已挂"));
        assert!(lines[1].contains("Py312") && lines[1].contains("未挂钩"));
        assert!(
            lines[2].contains("python 解释器 1/2")
                && lines[2].contains("工具 venv 1/3")
                && lines[2].contains("node 未接")
        );
        assert!(lines[3].contains("42 字节"));
        // 被隔离与未生成两个分支（R14 语义：缺失 ≠ 被隔离）
        let mut q = st.clone();
        q.log = Some((
            PathBuf::from(r"C:\log\oa-import.txt"),
            ImportLogState::Quarantined(PathBuf::from(r"C:\log\oa-import.txt.IPGSD")),
        ));
        assert!(hooks_lines(&q)[3].contains("隔离"));
        let mut m = st;
        m.log = Some((
            PathBuf::from(r"C:\log\oa-import.txt"),
            ImportLogState::Missing,
        ));
        assert!(hooks_lines(&m)[3].contains("尚未生成"));
    }

    #[test]
    fn hooks_feedback_merges_diagnostic_lines() {
        // 无诊断行：原文案原样返回
        assert_eq!(
            hooks_feedback_text("钩子已安装，下方为最新状态", "python 2/2 已挂\nnode 已接"),
            "钩子已安装，下方为最新状态"
        );
        // 有 [hooks] 诊断行：结果行在前，诊断逐行并入
        assert_eq!(
            hooks_feedback_text(
                "done",
                "summary\n[hooks] Python312: cannot write .pth and version unknown"
            ),
            "done（部分失败）：\n[hooks] Python312: cannot write .pth and version unknown"
        );
    }

    #[test]
    fn warning_signal_maps_tags_to_sources() {
        // 执行观测三路的诊断各有归属；signal 前缀（零 ETW 事件）同属 ETW
        assert_eq!(
            warning_signal("[etw] 70 次单次执行疑似自动化批量探测"),
            Some(SignalSource::Etw)
        );
        assert_eq!(warning_signal("[signal] 0 events"), Some(SignalSource::Etw));
        assert_eq!(
            warning_signal("[atime] 28 个访问时间命中疑似安全软件扫盘"),
            Some(SignalSource::Atime)
        );
        assert_eq!(
            warning_signal("[imports] import 日志被安全软件整文件隔离"),
            Some(SignalSource::Import)
        );
        // 无对应信号：npm 残骸（操作条处理）、store 库故障、无 tag 文本
        assert_eq!(warning_signal("[npm] 4 orphan shim set(s)"), None);
        assert_eq!(warning_signal("[store] inventory save failed"), None);
        assert_eq!(warning_signal("没有标签的警告"), None);
    }

    #[test]
    fn drift_lines_pair_verdicts_with_added_removed() {
        let diffs = vec![
            DriftDiff {
                verdict: Verdict::Active,
                added: vec![],
                removed: vec!["gone".into()],
            },
            DriftDiff {
                verdict: Verdict::Unused,
                added: vec!["new1".into(), "new2".into()],
                removed: vec![],
            },
        ];
        let lines = drift_lines(&diffs);
        assert_eq!(
            lines[0],
            ("活跃".to_string(), "—".to_string(), "gone".to_string())
        );
        assert_eq!(
            lines[1],
            (
                "未用".to_string(),
                "new1、new2".to_string(),
                "—".to_string()
            )
        );
    }
}

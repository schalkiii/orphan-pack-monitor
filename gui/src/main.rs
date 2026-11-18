//! orphan-audit GPUI 桌面界面。
//! 布局：标题栏、仪表盘统计条、工具条（筛选 / 使用说明 / 清理预览）、使用说明面板、
//! 包表格（点击表头排序）、证据面板、清理预览。界面文案中文，证据细节由 view 层翻译。
//! 数据来自 core 的 audit()；GUI 只负责展示与交互，不重算判定。

mod view;

use gpui::{
    div, prelude::*, px, rgb, App, Bounds, ClickEvent, Context, Entity, FontWeight, IntoElement,
    SharedString, Styled, Window, WindowBounds, WindowOptions,
};
use orphan_audit_core::model::{AuditConfig, Verdict, VerdictResult};
use view::{build_rows, row_by_index, verdict_cn, Row, SortDir, SortKey, Summary};

/// 应用状态：所有可变数据集中在 root view 中。
struct AppState {
    /// 原始审计结果（筛选/排序重建视图用，避免重复跑外部命令）
    results: Vec<VerdictResult>,
    summary: Summary,
    rows: Vec<Row>,
    selected: Option<usize>, // 对应 results 中的索引
    only_removable: bool,
    sort_key: SortKey,
    sort_dir: SortDir,
    status_line: SharedString,
    cleanup_preview: SharedString,
    /// 审计诊断（atime 跳过 / 采集器失败 / store 写失败等）——降级状态必须对用户可见
    warnings: Vec<String>,
    /// 后台审计进行中：表格区显示占位，完成前无数据
    loading: bool,
    /// 使用说明面板展开状态（默认展开：首次使用需要引导）
    show_help: bool,
    /// ETW 追踪是否可用（帮助面板据此提示运行采集器）
    etw_on: bool,
    /// 观察窗口（天）：切换后按该窗口重新审计，与 CLI --since 对齐
    since_days: u32,
    /// 会话级忽略列表（对应 CLI --ignore）：证据面板「忽略此包」追加，
    /// 工具条标签可移出；每次变更后重跑审计
    ignored: Vec<String>,
}

impl AppState {
    /// 初始加载态：窗口立即可见，审计在后台线程进行（冷启动不阻塞 UI）。
    fn loading() -> Self {
        Self {
            results: Vec::new(),
            summary: Summary::default(),
            rows: Vec::new(),
            selected: None,
            only_removable: false,
            sort_key: SortKey::Verdict,
            sort_dir: SortDir::Asc,
            status_line: "正在审计…".into(),
            cleanup_preview: "".into(),
            warnings: Vec::new(),
            loading: true,
            show_help: true,
            etw_on: false,
            since_days: 30,
            ignored: Vec::new(),
        }
    }

    /// 后台审计完成后回填状态（成功与失败路径共用）。
    /// 错误已在 spawn 侧格式化为字符串（gui 不直接依赖 anyhow）。
    fn apply_report(&mut self, report: Result<orphan_audit_core::AuditReport, String>) {
        self.loading = false;
        match report {
            Ok(report) => {
                self.warnings = report.warnings;
                self.etw_on = report.tracker.available;
                self.summary = Summary::from_results(&report.results);
                self.rows =
                    build_rows(&report.results, false, None, SortKey::Verdict, SortDir::Asc);
                let sources = if report.history_sources.is_empty() {
                    "无".to_string()
                } else {
                    report.history_sources.join(", ")
                };
                self.status_line = format!(
                    "{} 个包 · 历史条目 {} · 来源 {} · ETW {}",
                    report.results.len(),
                    report.history_total,
                    sources,
                    if report.tracker.available {
                        "开启"
                    } else {
                        "关闭（降级）"
                    }
                )
                .into();
                self.results = report.results;
                self.cleanup_preview = "点击「预览清理脚本」查看未用包将生成的卸载命令。".into();
            }
            Err(e) => {
                self.warnings = Vec::new();
                self.status_line = format!("审计失败：{e}").into();
            }
        }
    }

    fn rebuild(&mut self) {
        self.summary = Summary::from_results(&self.results);
        self.rows = build_rows(
            &self.results,
            self.only_removable,
            None,
            self.sort_key,
            self.sort_dir,
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
    fn start_audit(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.status_line = "正在审计…".into();
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
                state.apply_report(report.map_err(|e| format!("{e:#}")));
                cx.notify();
            });
        })
        .detach();
    }

    fn preview_cleanup(&mut self) {
        // 观察期新装包不进清理预览（装了还没来得及用不算弃用证据）；
        // 窗口天数跟随当前档位（此前硬编码 30，与 CLI --since 不对齐）
        let removable: Vec<VerdictResult> = self
            .results
            .iter()
            .filter(|r| r.verdict == Verdict::Unused && !r.newly_installed)
            .cloned()
            .collect();
        let script = orphan_audit_core::actions::generate_script(&removable, self.since_days);
        let first_lines: Vec<&str> = script.lines().take(6).collect();
        self.cleanup_preview = first_lines.join("  ⏎  ").into();
    }
}

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.summary.clone();
        let status = self.status_line.clone();
        let preview = self.cleanup_preview.clone();
        // selected 是 results 索引；排序/筛选后行位置会变，必须按 index 查找而非按位置取
        let selected_row = self
            .selected
            .and_then(|i| row_by_index(&self.rows, i).cloned());
        let rows = self.rows.clone();
        let loading = self.loading;
        let only_removable = self.only_removable;
        let sort_key = self.sort_key;
        let sort_dir = self.sort_dir;
        let show_help = self.show_help;
        let etw_on = self.etw_on;
        let warnings = self.warnings.clone();
        let since_days = self.since_days;
        let ignored = self.ignored.clone();
        // 捕获自身句柄，供事件闭包更新状态（GPUI 0.3.12 惯用模式）
        let this = cx.entity();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x12151c))
            .text_color(rgb(0xe6e9ef))
            // 标题栏
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_5()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(0x2b313d))
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("orphan-audit")
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x76808f))
                                    .child("Windows 孤儿 CLI 工具审计器 —— 找出装了却不用的命令行工具"),
                            ),
                    )
                    .child(div().text_sm().text_color(rgb(0xa9b1c0)).child(status)),
            )
            // 仪表盘统计条
            .child(
                div()
                    .flex()
                    .gap_3()
                    .px_5()
                    .py_3()
                    .child(stat_card("总数", summary.total, rgb(0x7aa2f7)))
                    .child(stat_card("活跃", summary.active, rgb(0x5dcaa5)))
                    .child(stat_card("间接", summary.indirect, rgb(0xb48ead)))
                    .child(stat_card("偶用", summary.rare, rgb(0xef9f27)))
                    .child(stat_card("集成", summary.integration, rgb(0xe0af68)))
                    .child(stat_card("未用", summary.unused, rgb(0xe24b4a))),
            )
            // 工具条：筛选 / 使用说明开关 / 清理预览（排序已改为点击表头）
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_5()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0x2b313d))
                    .child(
                        div()
                            .id("filter-removable")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(if only_removable {
                                rgb(0x7aa2f7)
                            } else {
                                rgb(0x2b313d)
                            })
                            .bg(if only_removable {
                                rgb(0x243049)
                            } else {
                                rgb(0x1d212b)
                            })
                            .text_sm()
                            .text_color(if only_removable {
                                rgb(0x7aa2f7)
                            } else {
                                rgb(0xa9b1c0)
                            })
                            .child("仅看可清理（未用）")
                            .on_click({
                                let this = this.clone();
                                move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                                    this.update(app, |state: &mut AppState, cx| {
                                        state.only_removable = !state.only_removable;
                                        state.rebuild();
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                    // 观察窗口档位：切换即按该窗口重新审计（与 CLI --since 对齐）
                    .child(div().text_xs().text_color(rgb(0x76808f)).child("窗口"))
                    .child(since_tab("30 天", 30, since_days, this.clone()))
                    .child(since_tab("90 天", 90, since_days, this.clone()))
                    .child(since_tab("1 年", 365, since_days, this.clone()))
                    // 已忽略包标签（点击 × 移出忽略列表并重跑审计）；
                    // 忽略语义与 CLI --ignore 一致：整包退出审计
                    .when(!ignored.is_empty(), |el| {
                        el.child(div().text_xs().text_color(rgb(0x76808f)).child("已忽略"))
                            .children(
                                ignored
                                    .iter()
                                    .map(|n| ignored_chip(n, this.clone()))
                                    .collect::<Vec<_>>(),
                            )
                    })
                    .child(
                        div()
                            .id("toggle-help")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(if show_help {
                                rgb(0x7aa2f7)
                            } else {
                                rgb(0x2b313d)
                            })
                            .bg(if show_help {
                                rgb(0x243049)
                            } else {
                                rgb(0x1d212b)
                            })
                            .text_sm()
                            .text_color(if show_help {
                                rgb(0x7aa2f7)
                            } else {
                                rgb(0xa9b1c0)
                            })
                            .child(if show_help {
                                "使用说明 ▴"
                            } else {
                                "使用说明 ▾"
                            })
                            .on_click({
                                let this = this.clone();
                                move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                                    this.update(app, |state: &mut AppState, cx| {
                                        state.show_help = !state.show_help;
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                    .child(div().flex_1().child(""))
                    .child(
                        div()
                            .id("re-audit")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(0x2b313d))
                            .bg(rgb(0x1d212b))
                            .text_sm()
                            .text_color(rgb(0xa9b1c0))
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
                    .child(
                        div()
                            .id("preview-cleanup")
                            .px_4()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x7aa2f7))
                            .text_sm()
                            .text_color(rgb(0x0f1115))
                            .font_weight(FontWeight::MEDIUM)
                            .child("预览清理脚本")
                            .on_click({
                                let this = this.clone();
                                move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                                    this.update(app, |state: &mut AppState, cx| {
                                        state.preview_cleanup();
                                        cx.notify();
                                    });
                                }
                            }),
                    ),
            )
            // 审计警告横幅（有诊断才出现）：atime 跳过 / 采集器失败等降级信息不可丢
            .when(!warnings.is_empty(), |el| {
                el.child(warnings_banner(&warnings))
            })
            // 使用说明面板（默认展开，描述运作方式与操作）
            .when(show_help, |el| el.child(help_panel(etw_on)))
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
                            .child(table_header(sort_key, sort_dir, this.clone()))
                            .children(table_rows(
                                loading,
                                &rows,
                                selected_row.as_ref().map(|x| x.index),
                                this.clone(),
                            )),
                    )
                    .child(
                        div()
                            .w(px(360.0))
                            .flex()
                            .flex_col()
                            .border_l_1()
                            .border_color(rgb(0x2b313d))
                            .bg(rgb(0x171a21))
                            .p_4()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x7aa2f7))
                                    .child("证据"),
                            )
                            .child(evidence_panel(selected_row.as_ref(), this.clone())),
                    ),
            )
            // 底部清理预览
            .child(
                div()
                    .h(px(96.0))
                    .flex()
                    .flex_col()
                    .px_5()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(0x2b313d))
                    .bg(rgb(0x171a21))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x76808f))
                            .child("清理脚本预览（审阅后再执行；本工具不会自动删除，完整脚本请运行 orphan-audit clean 生成）"),
                    )
                    .child(div().text_xs().text_color(rgb(0xa9b1c0)).child(preview)),
            )
    }
}

// --- 组件 helpers ---

fn stat_card(label: &'static str, value: usize, color: gpui::Rgba) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(0x2b313d))
        .bg(rgb(0x1d212b))
        .min_w(px(96.0))
        .child(div().text_xs().text_color(rgb(0x76808f)).child(label))
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(value.to_string()),
        )
}

/// 使用说明：运作方式、五档判定、操作方式、安全边界。
/// ETW 不可用时追加采集器提示（用户据此知道如何补上执行信号）。
fn help_panel(etw_on: bool) -> impl IntoElement {
    let section = |t: &str| {
        div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(0x7aa2f7))
            .child(t.to_string())
    };
    let line = |t: &str| {
        div()
            .text_xs()
            .text_color(rgb(0xa9b1c0))
            .child(t.to_string())
    };
    let mut col = div()
        .flex()
        .flex_col()
        .gap_1()
        .px_5()
        .py_2()
        .border_b_1()
        .border_color(rgb(0x2b313d))
        .bg(rgb(0x171a21));
    col = col
        .child(section("运作方式"))
        .child(line("扫描 cargo / npm / pip / uv / pipx 安装的全局包，结合五路信号——shell 历史、ETW 执行日志、文件访问时间、shell 配置、别名——给出五档判定与可反驳的证据链。"))
        .child(section("五档判定"))
        .child(line("活跃＝经常手动输入　间接＝近期被执行过（脚本/程序调用）　偶用＝历史用过、近期没跑"))
        .child(line("集成＝被 shell 配置引用　未用＝五路信号全静默（唯一进入清理候选）"))
        .child(section("操作"))
        .child(line("点击表头排序（再点一次反向）；点击任意一行，在右侧查看该包的判定证据；「仅看可清理」只显示未用包；证据面板可「忽略此包」（会话内退出审计，对应 CLI --ignore，工具条标签可恢复）；「预览清理脚本」查看将生成的卸载命令。"))
        .child(section("安全"))
        .child(line("本工具绝不自动删除。卸载脚本需在终端运行 orphan-audit clean 生成，由你审阅后自行执行。"));
    if !etw_on {
        col = col.child(div().text_xs().text_color(rgb(0xe0af68)).child(
            "提示：ETW 执行日志未开启。以管理员身份运行 etw-capture install（一次部署、登录自启、常驻采集）即可；也可前台运行 etw-capture（Ctrl+C 停止）。开启后，被脚本/程序调用的工具会被记为「间接」。",
        ));
    }
    col
}

/// 审计警告横幅：逐条展示 core 产出的诊断（降级状态对用户可见，不可只在 CLI stderr）。
fn warnings_banner(warnings: &[String]) -> impl IntoElement {
    let mut col = div()
        .flex()
        .flex_col()
        .gap_1()
        .px_5()
        .py_2()
        .bg(rgb(0x2b2416))
        .border_b_1()
        .border_color(rgb(0x5a4a1e));
    col = col.child(
        div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(0xe0af68))
            .child("审计警告"),
    );
    for w in warnings {
        col = col.child(div().text_xs().text_color(rgb(0xd9c58a)).child(w.clone()));
    }
    col
}

/// 观察窗口档位标签：点击即按该窗口重新审计（当前档位高亮）。
fn since_tab(label: &str, days: u32, current: u32, this: Entity<AppState>) -> impl IntoElement {
    let active = current == days;
    div()
        .id(SharedString::from(format!("since-{days}")))
        .px_2()
        .py_1()
        .rounded_md()
        .text_xs()
        .border_1()
        .border_color(if active { rgb(0x7aa2f7) } else { rgb(0x2b313d) })
        .text_color(if active { rgb(0x7aa2f7) } else { rgb(0xa9b1c0) })
        .child(label.to_string())
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

/// 已忽略包标签：点击即移出忽略列表并重跑审计（恢复该包的判定与展示）。
fn ignored_chip(name: &str, this: Entity<AppState>) -> impl IntoElement {
    let n = name.to_string();
    div()
        .id(SharedString::from(format!("ignored-{n}")))
        .px_2()
        .py_1()
        .rounded_md()
        .text_xs()
        .border_1()
        .border_color(rgb(0x5a4a1e))
        .bg(rgb(0x2b2416))
        .text_color(rgb(0xd9c58a))
        .child(format!("{n} ×"))
        .on_click(
            move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                // 同上：Fn 闭包内 String 先复制再移交
                let n = n.clone();
                this.update(app, move |state: &mut AppState, cx| {
                    state.ignored.retain(|x| x != &n);
                    state.start_audit(cx);
                    cx.notify();
                });
            },
        )
}

/// 表头：每一列都可点击排序（当前排序列高亮并显示方向箭头）。
fn table_header(sort_key: SortKey, sort_dir: SortDir, this: Entity<AppState>) -> impl IntoElement {
    div()
        .flex()
        .px_4()
        .py_2()
        .bg(rgb(0x1d212b))
        .border_b_1()
        .border_color(rgb(0x2b313d))
        .text_xs()
        .text_color(rgb(0x76808f))
        .child(sortable_header(
            "包名",
            SortKey::Name,
            sort_key,
            sort_dir,
            240.0,
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
        .child(sortable_header(
            "判定",
            SortKey::Verdict,
            sort_key,
            sort_dir,
            120.0,
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
            100.0,
            this.clone(),
        ))
        .child(sortable_header(
            "大小",
            SortKey::Size,
            sort_key,
            sort_dir,
            90.0,
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
        .w(px(width))
        .flex()
        .items_center()
        .rounded_md()
        .text_xs()
        .text_color(if active { rgb(0x7aa2f7) } else { rgb(0x76808f) })
        .hover(|s| s.text_color(rgb(0xa9b1c0)))
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

/// 表格主体：加载中显示占位文案，否则渲染数据行。
fn table_rows(
    loading: bool,
    rows: &[Row],
    selected_idx: Option<usize>,
    this: Entity<AppState>,
) -> Vec<gpui::AnyElement> {
    if loading {
        vec![div()
            .px_4()
            .py_6()
            .text_sm()
            .text_color(rgb(0x76808f))
            .child("正在审计……读取 shell 历史、包管理器清单与配置（约数秒）")
            .into_any_element()]
    } else {
        rows.iter()
            .map(|r| package_row(r, selected_idx, this.clone()).into_any_element())
            .collect()
    }
}

fn package_row(row: &Row, selected_idx: Option<usize>, this: Entity<AppState>) -> impl IntoElement {
    let is_selected = selected_idx == Some(row.index);
    let idx = row.index;
    let verdict_color = match row.verdict {
        Verdict::Active => rgb(0x5dcaa5),
        Verdict::Indirect => rgb(0xb48ead),
        Verdict::Rare => rgb(0xef9f27),
        Verdict::Integration => rgb(0xe0af68),
        Verdict::Unused => rgb(0xe24b4a),
    };
    div()
        .id(("pkg-row", idx))
        .flex()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(rgb(0x22262f))
        .bg(if is_selected {
            rgb(0x243049)
        } else {
            rgb(0x12151c)
        })
        .hover(|s| s.bg(rgb(0x1d212b)))
        .on_click(
            move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
                this.update(app, move |state: &mut AppState, cx| {
                    state.selected = Some(idx);
                    cx.notify();
                });
            },
        )
        .child(div().w(px(240.0)).child(row.name.clone()))
        .child(
            div()
                .w(px(80.0))
                .text_color(rgb(0xa9b1c0))
                .child(row.manager.clone()),
        )
        .child(
            div()
                .w(px(120.0))
                .text_color(verdict_color)
                .child(verdict_cn(row.verdict)),
        )
        .child(
            div()
                .w(px(80.0))
                .text_color(rgb(0xa9b1c0))
                .child(row.typed.clone()),
        )
        .child(
            div()
                .w(px(100.0))
                .text_color(rgb(0xa9b1c0))
                .child(row.last_run.clone()),
        )
        .child(
            div()
                .w(px(90.0))
                .text_color(rgb(0xa9b1c0))
                .child(row.size.clone()),
        )
}

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
                .child(div().text_xs().text_color(rgb(0x76808f)).child(format!(
                    "{} · {}（{}） · 置信度 {}",
                    r.manager,
                    verdict_cn(r.verdict),
                    r.verdict.as_str(),
                    r.confidence
                )));
            for e in &r.evidence {
                col = col.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xc8d3f5))
                        .p_2()
                        .rounded_md()
                        .bg(rgb(0x1d212b))
                        .child(e.clone()),
                );
            }
            // 忽略此包：对应 CLI --ignore（会话级）。重跑后该包整包退出审计，
            // 选中态随之清空；工具条的「已忽略」标签可随时恢复
            let name = r.name.to_string();
            col = col.child(
                div()
                    .id("ignore-this-pkg")
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x2b313d))
                    .bg(rgb(0x1d212b))
                    .text_xs()
                    .text_color(rgb(0xa9b1c0))
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
            col = col.child(
                div()
                    .text_sm()
                    .text_color(rgb(0x76808f))
                    .child("点击任意一行，查看该包的判定依据。"),
            );
        }
    }
    col
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, gpui::size(px(1180.0), px(760.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
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

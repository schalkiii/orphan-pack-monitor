//! orphan-audit GPUI 桌面界面。
//! 布局：标题栏 + 仪表盘统计条 + 筛选/排序工具条 + 包表格 + 证据面板 + 清理预览。
//! 数据来自 core 的 audit()；GUI 只负责展示与交互，不重算判定。

mod view;

use gpui::{
    div, prelude::*, px, rgb, App, Bounds, ClickEvent, Context, Entity, FontWeight,
    IntoElement, SharedString, Styled, Window, WindowBounds, WindowOptions,
};
use orphan_audit_core::model::{AuditConfig, Verdict, VerdictResult};
use view::{build_rows, Row, SortKey, Summary};

/// 应用状态：所有可变数据集中在 root view 中。
struct AppState {
    /// 原始审计结果（筛选/排序重建视图用，避免重复跑外部命令）
    results: Vec<VerdictResult>,
    summary: Summary,
    rows: Vec<Row>,
    selected: Option<usize>, // 对应 results 中的索引
    only_removable: bool,
    sort_key: SortKey,
    status_line: SharedString,
    cleanup_preview: SharedString,
}

impl AppState {
    fn load() -> Self {
        let config = AuditConfig::default();
        match orphan_audit_core::audit(&config) {
            Ok(report) => {
                let summary = Summary::from_results(&report.results);
                let rows = build_rows(&report.results, false, None, SortKey::Verdict);
                let sources = if report.history_sources.is_empty() {
                    "none".to_string()
                } else {
                    report.history_sources.join(", ")
                };
                let status = format!(
                    "{} packages · {} history entries · history: {} · ETW: {}",
                    report.results.len(),
                    report.history_total,
                    sources,
                    if report.tracker.available { "on" } else { "off (degraded)" }
                );
                Self {
                    results: report.results,
                    summary,
                    rows,
                    selected: None,
                    only_removable: false,
                    sort_key: SortKey::Verdict,
                    status_line: status.into(),
                    cleanup_preview: "Click 'Preview cleanup' to generate a reviewable script.".into(),
                }
            }
            Err(e) => Self {
                results: Vec::new(),
                summary: Summary::default(),
                rows: Vec::new(),
                selected: None,
                only_removable: false,
                sort_key: SortKey::Verdict,
                status_line: format!("Audit failed: {e:#}").into(),
                cleanup_preview: "".into(),
            },
        }
    }

    fn rebuild(&mut self) {
        self.summary = Summary::from_results(&self.results);
        self.rows = build_rows(&self.results, self.only_removable, None, self.sort_key);
        self.selected = None;
    }

    fn preview_cleanup(&mut self) {
        let removable: Vec<VerdictResult> = self
            .results
            .iter()
            .filter(|r| r.verdict == Verdict::Unused)
            .cloned()
            .collect();
        let script = orphan_audit_core::actions::generate_script(&removable, 30);
        let first_lines: Vec<&str> = script.lines().take(6).collect();
        self.cleanup_preview = first_lines.join("  ⏎  ").into();
    }
}

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.summary.clone();
        let status = self.status_line.clone();
        let preview = self.cleanup_preview.clone();
        let selected_row = self.selected.and_then(|i| self.rows.get(i).cloned());
        let rows = self.rows.clone();
        let only_removable = self.only_removable;
        let sort_key = self.sort_key;
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
                                    .child("Windows orphan CLI tool detector"),
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
                    .child(stat_card("Total", summary.total, rgb(0x7aa2f7)))
                    .child(stat_card("Active", summary.active, rgb(0x5dcaa5)))
                    .child(stat_card("Indirect", summary.indirect, rgb(0xb48ead)))
                    .child(stat_card("Rare", summary.rare, rgb(0xef9f27)))
                    .child(stat_card("Integration", summary.integration, rgb(0xe0af68)))
                    .child(stat_card("Unused", summary.unused, rgb(0xe24b4a))),
            )
            // 工具条
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
                            .border_color(if only_removable { rgb(0x7aa2f7) } else { rgb(0x2b313d) })
                            .bg(if only_removable { rgb(0x243049) } else { rgb(0x1d212b) })
                            .text_sm()
                            .text_color(if only_removable { rgb(0x7aa2f7) } else { rgb(0xa9b1c0) })
                            .child("Only removable")
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
                    .child(sort_button("Verdict", SortKey::Verdict, sort_key, this.clone()))
                    .child(sort_button("Name", SortKey::Name, sort_key, this.clone()))
                    .child(sort_button("Manager", SortKey::Manager, sort_key, this.clone()))
                    .child(sort_button("Last run", SortKey::LastRun, sort_key, this.clone()))
                    .child(div().flex_1().child(""))
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
                            .child("Preview cleanup")
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
                            .child(table_header())
                            .children(rows.iter().map(|r| {
                                package_row(r, selected_row.as_ref().map(|x| x.index), this.clone())
                            })),
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
                                    .child("Evidence"),
                            )
                            .child(evidence_panel(selected_row.as_ref())),
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
                            .child("Cleanup script preview (review before running)"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xa9b1c0))
                            .child(preview),
                    ),
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

fn table_header() -> impl IntoElement {
    div()
        .flex()
        .px_4()
        .py_2()
        .bg(rgb(0x1d212b))
        .border_b_1()
        .border_color(rgb(0x2b313d))
        .text_xs()
        .text_color(rgb(0x76808f))
        .child(div().w(px(240.0)).child("Package"))
        .child(div().w(px(80.0)).child("Manager"))
        .child(div().w(px(120.0)).child("Verdict"))
        .child(div().w(px(80.0)).child("Typed"))
        .child(div().w(px(100.0)).child("Last run"))
        .child(div().w(px(90.0)).child("Size"))
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
        .bg(if is_selected { rgb(0x243049) } else { rgb(0x12151c) })
        .hover(|s| s.bg(rgb(0x1d212b)))
        .on_click(move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
            this.update(app, move |state: &mut AppState, cx| {
                state.selected = Some(idx);
                cx.notify();
            });
        })
        .child(div().w(px(240.0)).child(row.name.clone()))
        .child(div().w(px(80.0)).text_color(rgb(0xa9b1c0)).child(row.manager.clone()))
        .child(div().w(px(120.0)).text_color(verdict_color).child(row.verdict.as_str()))
        .child(div().w(px(80.0)).text_color(rgb(0xa9b1c0)).child(row.typed.clone()))
        .child(div().w(px(100.0)).text_color(rgb(0xa9b1c0)).child(row.last_run.clone()))
        .child(div().w(px(90.0)).text_color(rgb(0xa9b1c0)).child(row.size.clone()))
}

fn evidence_panel(row: Option<&Row>) -> impl IntoElement {
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
                        .text_xs()
                        .text_color(rgb(0x76808f))
                        .child(format!(
                            "{} · {} · confidence {}",
                            r.manager, r.verdict.as_str(), r.confidence
                        )),
                );
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
        }
        None => {
            col = col.child(
                div()
                    .text_sm()
                    .text_color(rgb(0x76808f))
                    .child("Select a package to see the evidence behind its verdict."),
            );
        }
    }
    col
}

fn sort_button(
    label: &'static str,
    key: SortKey,
    current: SortKey,
    this: Entity<AppState>,
) -> impl IntoElement {
    let active = key == current;
    div()
        .id(SharedString::from(format!("sort-{key:?}")))
        .px_3()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(if active { rgb(0x7aa2f7) } else { rgb(0x2b313d) })
        .bg(if active { rgb(0x243049) } else { rgb(0x1d212b) })
        .text_sm()
        .text_color(if active { rgb(0x7aa2f7) } else { rgb(0xa9b1c0) })
        .child(label)
        .on_click(move |_e: &ClickEvent, _window: &mut Window, app: &mut App| {
            this.update(app, move |state: &mut AppState, cx| {
                state.sort_key = key;
                state.rebuild();
                cx.notify();
            });
        })
}

fn main() {
    let state = AppState::load();
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, gpui::size(px(1180.0), px(760.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|_| state),
        )
        .unwrap();
    });
}

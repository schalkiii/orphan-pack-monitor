# AGENTS.md — orphan-audit

## 定位
Windows 孤儿 CLI 工具审计器：找出装了却长期不用的全局包（cargo/npm/pip/uv/pipx），
五档判定（active/indirect/rare/integration/unused）+ 可反驳证据；只生成卸载脚本，**绝不自动删**。

## 怎么跑
- 测试基线：`cargo test --workspace`（27 用例）；GUI 另建：`cd gui && cargo test`
- 质量门禁：`cargo clippy --workspace --all-targets -- -D warnings`（gui/ 目录内同样跑 clippy）
- 格式：`cargo fmt`（workspace 与 gui/ 各跑一次；spikes/ 冻结不动）
- CLI：`cargo run -p orphan-audit -- audit`（子命令 audit/status/clean/drift，全局 --json/--since/--ignore/--manager）
- GUI：`cd gui && cargo run`（独立 workspace，避免 CLI 编译拖入 GPUI）

## 技术栈
Rust 1.85+（实测 1.99.1）；rusqlite(bundled) 存储；clap 4；GUI 用 gpui-pre 0.3.12
（package 重命名 gpui，平台入口 `gpui_platform::application()`）；Windows 优先（ETW/atime/PSReadLine）。

## 目录与约定
- `crates/core` 引擎（model/inventory/signals/verdict/store/actions/etw），无 GUI 依赖；
  `crates/cli` CLI；`gui/` 独立 workspace；`spikes/` 是 GPUI 可行性证据（exclude，勿改）。
- 判定规则顺序即正确性：active→indirect→rare→integration→unused，重排必产生误报（见 verdict.rs 模块注释）。
- 安全红线：不自动卸载；依赖包（is_dependency）与 ignore 列表永不进清理候选。
- 改动纪律：最小改动、逐点验证（每改一点跑全测）、注释同步；提交前 lint 清零。
- ETW 现状：只消费 `events.jsonl`，采集端是规划组件（AUDIT-REPORT-2026-11-18.md S17），
  文档/文案不得宣称「提权即开启追踪」。

## 当前状态与下一步
- v0.1.0 golden：27 测试全绿、clippy 清零、真机验证（9 包/6664 历史/1 active 8 unused）。
- 已知未修缺陷与改进清单见 `AUDIT-REPORT-2026-11-18.md`（P0：GUI 选中索引错位、shell-config 大小写；
  功能性修复走架构分支，等价清理已做）。

# AGENTS.md — orphan-audit

## 定位
Windows 孤儿 CLI 工具审计器：找出装了却长期不用的全局包（cargo/npm/pip/uv/pipx），
五档判定（active/indirect/rare/integration/unused）+ 可反驳证据；只生成卸载脚本，**绝不自动删**。

## 怎么跑
- 测试基线：`cargo test --workspace`；GUI 另建：`cd gui && cargo test`；采集端另建：`cd etw-capture && cargo test`
- 质量门禁：`cargo clippy --workspace --all-targets -- -D warnings`（gui/、etw-capture/ 目录内同样跑 clippy）
- 格式：`cargo fmt`（workspace 与 gui/、etw-capture/ 各跑一次；spikes/ 冻结不动）
- CLI：`cargo run -p orphan-audit -- audit`（子命令 audit/status/clean/drift，全局 --json/--since/--ignore/--manager）
- GUI：`cd gui && cargo run`（独立 workspace，避免 CLI 编译拖入 GPUI）
- ETW 采集：`cd etw-capture && cargo run`（**管理员**；订阅 Kernel-Process 写 events.jsonl，Ctrl+C 停；设计见 etw-capture/DESIGN.md）

## 技术栈
Rust 1.85+（实测 1.99.1）；rusqlite(bundled) 存储；clap 4；GUI 用 gpui-pre 0.3.12
（package 重命名 gpui，平台入口 `gpui_platform::application()`）；Windows 优先（ETW/atime/PSReadLine）。

## 目录与约定
- `crates/core` 引擎（model/inventory/signals/verdict/store/actions/etw），无 GUI 依赖；
  `crates/cli` CLI；`gui/`、`etw-capture/` 各为独立 workspace；`spikes/` 是 GPUI 可行性证据（exclude，勿改）。
- 判定规则顺序即正确性：active→indirect→rare→integration→unused，重排必产生误报（见 verdict.rs 模块注释）。
- 两条安全增强：新装观察期（installed_at 在 since 窗口内的包不进清理候选）、
  漂移证据（连续 ≥3 个月快照均判 unused → 证据链追加漂移行，档位不变）。
- 安全红线：不自动卸载；依赖包（is_dependency）与 ignore 列表永不进清理候选。
- 改动纪律：最小改动、逐点验证（每改一点跑全测）、注释同步；提交前 lint 清零。
- ETW 现状：`audit` 只消费 `events.jsonl`（`read_etw_log_since` 只取 `--since` 窗口内
  事件）；采集端 `etw-capture/`（ferrisetw UserTrace，订阅 Kernel-Process）支持**常驻**
  （`install` 子命令注册 ONLOGON 计划任务，本机已部署；任务设置已加固为电池容忍 +
  无时限，日志 64MB 轮转）与前台手动两种形态；运行中重复 `install` 即升级；
  `uninstall`/`status` 管理任务。`audit` 本身不开启追踪——文档/文案不得宣称
  「提权即开启追踪」。

## 当前状态与下一步
- 62 测试全绿（core 50 + cli 3 + gui 8 + etw-capture 1）、三 workspace clippy/fmt 清零、
  真机验证（28 包；ETW 采集闭环 + 常驻化 install/status/升级路径实测通过，2026-11-18）。
- 第二轮审计见 `doc/AUDIT-REPORT-2026-11-18.md`：A6/A9/A10 已修（code-quality 分支），
  **A1（P1）atime 本地化解析已修**（parse_disablelastaccess 纯函数 + 多语言 fixture）。
  准确性增强批：atime 安装期过滤、PSReadLine 全 host、zsh EXTENDED_HISTORY、
  wrapper 扩充、新装观察期、漂移信号。
- 第三轮审计见 `doc/AUDIT-REPORT-2026-11-18-R3.md`：**R1-R12 全部执行完毕**
  （事件管道治理、install 升级路径+任务加固、uv/pipx 观察期兜底、CI fmt、GUI
  警告横幅+审计窗口档位、漂移测试、ETW 计数限窗、文档漂移、TrackerStatus 精简、
  drift 校验、小项清理）。
- 后续：R3 报告中明确跳过的 C12（跨 crate 测试辅助，见报告第五节）、发布打包、
  Security 4688 信号源（需用户确认审计策略改动）。第二轮审计 P2（degraded 文案）与
  GUI --ignore 入口已于 2026-11-18 补齐（降级语义改为按窗口内事件判定 + 证据面板
  「忽略此包」）。

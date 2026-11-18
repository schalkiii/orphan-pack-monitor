# orphan-audit 审计报告（第二轮，2026-11-18）

> 本报告是当日第二轮全面审计。第一轮为根目录 `AUDIT-REPORT-2026-11-18.md`（S1–S18，
> 已全部执行完毕，保留为历史记录）。本轮覆盖 S17 交付后的完整现状，供后续执行 Agent 使用。
> 审计纪律：本轮只登记问题与建议，不修改任何代码。

## 一、现状描述

- **规模**：11 个 Rust 源文件，约 3,700 非空行。三个 workspace：
  根（`crates/core` 2,605 行 8 模块 + `crates/cli` 276 行）、`gui/`（732 行）、
  `etw-capture/`（114 行）。另有 `spikes/`（冻结的 GPUI 可行性验证，不入构建）。
- **结构**：core 分层 inventory → signals → verdict → store → actions → etw，无 GUI 依赖；
  cli 与 gui 各自只做编排与展示；etw-capture 独立二进制复用 core 的 events.jsonl 契约。
- **质量水平**：42 个单元测试全绿（core 33 + cli 3 + gui 4；etw-capture 0）；
  三 workspace clippy `-D warnings` 与 `cargo fmt --check` 清零；CI（GitHub Actions
  windows-latest）覆盖三 workspace 的 test+clippy。README/AGENTS/CHANGELOG/DESIGN
  与代码基本同步（本轮发现 3 处过期文案，见 A2/A6/A7）。ETW 采集端已实测闭环
  （2026-11-18：捕获 cargo-binstall → audit 判 indirect）。
- **依赖**：core 仅 serde/serde_json/chrono/anyhow/rusqlite(bundled)/dirs/shell-words；
  采集端额外 ferrisetw 1.2 + ctrlc。无重型依赖泄漏到 CLI 构建链。

## 二、问题清单

> **执行状态（2026-11-18 当日更新）**：A6/A9/A10 已由 code-quality 分支执行完毕
> （见 CHANGELOG [Unreleased] Changed/Removed）；**A1（atime 本地化解析 bug，高）仍待执行**，
> 其余 A2–A5/A7/A8/A11–A14 待排期。

| # | 位置 | 问题 | 严重度 | 影响 |
|---|------|------|--------|------|
| A1 | `crates/core/src/signals.rs:381-400` `atime_enabled` | fsutil 输出带本地化括号后缀（本机实测：`DisableLastAccess = 2  (托管的系统，上次访问时间更新已启用)`），`u32::from_str_radix` 对整串 `val` 解析失败返回 None → **非英文 Windows 上 atime 信号被静默跳过**。本机 2026-11-18 审计即出现 `[atime] could not determine last-access state`，而 atime 实际处于启用状态（值 2） | **高** | 功能错误：中文/法文等本地化 Windows 全部丢失 atime 执行证据。atime 本可把「近期跑过但历史无记录」的包从 unused 救成 indirect——信号缺失直接推高误删风险（本项目唯一目标是安全清理） |
| A2 | `crates/core/src/lib.rs:123-127` | degraded 警告文案写 "(ETW capture component is not bundled; see README)" —— `etw-capture/` 现已随仓库提供，文案过时 | 中 | 误导：非提权用户被告知采集器「未随仓库提供」，实际指引应是「以管理员运行随仓库的 etw-capture」 |
| A3 | `.github/workflows/ci.yml` | CI 只有 test+clippy，**没有 `cargo fmt --check`**。仓库纪律（AGENTS.md「提交前 lint 清零」）含 fmt，但 CI 不拦截 | 中 | fmt 回归可合入 main 而门禁全绿；本地 fmt diff 已在本轮发生过一次（signals.rs/verdict.rs 测试代码） |
| A4 | `gui/src/main.rs` `apply_report` | `report.warnings` 被 GUI 完全丢弃（CLI 会打印）。采集器失败（如 `[pip] collector failed`）、ETW 日志读取失败、降级原因等对 GUI 用户不可见 | 中 | GUI 用户基于不完整数据做删除决策却不自知——与「可反驳证据」的产品原则冲突 |
| A5 | `gui/src/main.rs:99` `preview_cleanup` | 硬编码 `generate_script(&removable, 30)`；GUI 亦无 --since/--ignore 配置入口（AGENTS.md 路线图项） | 中低 | 预览脚本声称 since=30d，但用户无法在 GUI 内改变窗口；与 CLI 行为不一致 |
| A6 | `crates/core/src/model.rs:154` | 注释「ETW 提供完整命令行」与实证矛盾：Win11 build 28000 的 Kernel-Process event 1 无 CommandLine 属性（TDH NotFound，2026-11-18 实测，见 etw-capture/DESIGN.md） | 低 | 后续贡献者会尝试依赖 cmd 字段做归因/展示，浪费排查时间 |
| A7 | `AGENTS.md` 当前状态节 | 「v0.1.0 golden：42 测试全绿」——42 是当前数（v0.1.0 当时为 27），标签与数字混搭 | 低 | 轻微误导；数字与标签应其一更正 |
| A8 | `crates/core/src/verdict.rs:206-222` `is_interactive_shell` | 硬编码名单缺常见 shell：`nu.exe`、`csh`/`tcsh`、`powershell_ise.exe`、`openconsole.exe` 等 | 低 | 不影响判定档位（ETW 在窗口内即 indirect），只影响证据文案：这些 shell 下的执行被记成 `via <shell>` 而非 `executed Nx` |
| A9 | `crates/core/src/signals.rs:363-376` `probe_atime` | 返回占位 `binary="atime"` 的事件，由 lib.rs:83 调用方覆写为真实命令名。契约脆弱：调用方漏覆写即产生永远匹配不到任何包的垃圾事件 | 低 | 维护陷阱：函数签名暗示可直接使用，实际必须二次加工 |
| A10 | `crates/core/src/store.rs:185-208` | `ignores` 表只写不读：`sync_ignores` 每次全量 DELETE+INSERT，`ignores()` 无任何消费方（CLI/GUI/audit 路径都不读） | 低 | 死代码/死表：占用迁移逻辑与维护注意力，给人「忽略项已持久化生效」的错觉 |
| A11 | `crates/core/src/verdict.rs:178-183` `evaluate_all` | 事件匹配 O(包数×事件数)：每个包线性过滤全部事件。当前规模（28 包/7k 事件）无感；十万级事件 × 千级包时明显变慢 | 低 | 规模化瓶颈（可先按 binary 建 HashMap 索引一次性分组） |
| A12 | `etw-capture/src/main.rs` | 0 个单元测试：PID→父名缓存、event_id 过滤、strip_exe 映射等逻辑无自动化覆盖（契约写入 core 侧有测试，采集侧映射没有） | 低 | 回归风险：改采集逻辑只能靠提权手工验证 |
| A13 | `crates/cli/src/main.rs:68-95` | `ManagerArg` 手写复制 Manager 九个变体 + From 映射；新增管理器须改两处 | 低 | 双维护点，漏改在编译期即可发现（属可接受的样板） |
| A14 | `crates/core/src/inventory.rs:166` | `npm_prefix().unwrap_or_default()` 失败时 prefix 为空 → install_path 变成相对路径 `node_modules/<pkg>`（垃圾数据入库并显示） | 低 | 罕见路径（npm root -g 失败）下的脏数据；应失败时置 None |

**七维结论**：①文档一致性——A2/A6/A7 三处过期文案；②文档缺漏——store schema 无说明文档（低，可从 store.rs 读懂）、GUI 使用仅 README 一句话（低，可接受）；③架构——A9/A10/A11/A13，分层清晰整体良好；④代码写法——A14，整体密度与命名良好；⑤测试覆盖率——A12（etw-capture 0 测试）、atime_enabled 解析无测试（A1 的修复应补）、is_interactive_shell/generate_report 无直接测试（低）；⑥测试严谨性——未发现「跑通即通过」用例，断言均检查具体值，本维度未发现问题；⑦工程规范——A3（CI 缺 fmt 门禁）；无 MSRV/release job（可选优化，非缺陷）。

## 三、改进建议

### B1（对应 A1，P1）atime_enabled 支持本地化输出

- **做法**：抽出纯函数 `parse_disablelastaccess(out: &str) -> Option<bool>`（放 signals.rs）：
  取 `=` 后**第一个空白分隔 token**（剥掉 `0x` 前缀）再做 `u32::from_str_radix(.., 16)`；
  映射 0/2→Some(true)，1/3→Some(false)，其余 None。`atime_enabled` 改为调用它。
  补测试 fixture：英文 `DisableLastAccess = 2`、十六进制 `= 0x00000002`、
  中文带括号后缀 `= 2  (托管的系统，上次访问时间更新已启用)`、垃圾输入、缺失 `=`。
- **文件**：`crates/core/src/signals.rs`
- **验证**：`cargo test --workspace`（新 fixture 测试通过）；本机 `cargo run -p orphan-audit -- audit`
  不再出现 `[atime] could not determine last-access state`，且 pip 包出现 atime/间接证据
  （atime 当前为系统管理启用态）。

### B2（对应 A2，P2）更新 degraded 警告文案

- **做法**：lib.rs:123-127 文案改为指向随仓库的采集器，例如
  `"[signal] no ETW events available — verdicts rely on shell history + atime only (run the bundled etw-capture component as admin to record executions; see README)"`。
- **文件**：`crates/core/src/lib.rs`
- **验证**：`grep -rn "not bundled" crates/` 无残留；非提权终端跑 audit，警告为新文案；`cargo test --workspace`。

### B3（对应 A3，P2）CI 补 fmt 门禁

- **做法**：ci.yml 为三个 workspace 各加一步 `cargo fmt --check`
  （root / working-directory: gui / working-directory: etw-capture）。
- **文件**：`.github/workflows/ci.yml`
- **验证**：本地等价命令三 workspace `cargo fmt --check` 全过（CI 无自托管 runner，无法本地跑 Actions；推送后以 Actions 日志为准）。

### B4（对应 A4+A5，P2）GUI 展示警告

- **做法**：`AppState` 增加 `warnings: Vec<String>`（`apply_report` 从 report.warnings 填充，
  Err 分支填单条错误）；在状态栏下方或证据面板顶部渲染（有警告时黄色小字，逐条列出）。
  顺手把 `preview_cleanup` 的 30 改为引用 `report.config.since_days`（apply_report 时暂存）。
  view 层新增 `has_warnings` 渲染分支的纯测试。
- **文件**：`gui/src/main.rs`、`gui/src/view.rs`
- **验证**：`cd gui && cargo test && cargo clippy --all-targets -- -D warnings`；
  手动构造 warning（如临时改名 pip 探测）确认 GUI 可见。注意：GUI 完整配置入口（--since/--ignore
  输入框）是路线图功能，不在本建议范围。

### B5（对应 A6/A7/A8/A9/A10/A13/A14，P3）小修集

- **做法**（各自独立，可任选执行）：
  - A6：model.rs:154 注释改为「ETW 提供父进程名；命令行在较新 Windows 构建缺失（见 etw-capture/DESIGN.md）」
  - A7：AGENTS.md 改为「golden 基线 27 测试 → 当前 42 测试全绿」或直接删版本标签
  - A8：is_interactive_shell 补 `nu.exe`/`csh`/`tcsh`/`powershell_ise.exe`/`openconsole.exe`
  - A9：probe_atime 签名改为 `probe_atime(binaries: &[PathBuf], binary_name: &str) -> Option<UsageEvent>` 直接产出正确事件
  - A10：删除 ignores 表与 sync_ignores/ignores()（或改为 audit 时读取合并——当前无消费需求，倾向删除）
  - A13：保持现状（编译期可发现的样板，不动）
  - A14：`npm_prefix()` 失败时返回 None 并让 install_path 置 None
- **文件**：`crates/core/src/model.rs`、`AGENTS.md`、`crates/core/src/verdict.rs`、`crates/core/src/signals.rs`、`crates/core/src/store.rs`、`crates/core/src/inventory.rs`
- **验证**：每项改后 `cargo test --workspace` + clippy + fmt；A9/A10 需同步调用方（lib.rs）。

## 四、优先级与并行性

| 优先级 | 建议 | 可并行性 |
|--------|------|----------|
| P1 | B1 atime 本地化解析 | **可并行**（signals.rs） |
| P2 | B2 degraded 文案（lib.rs） | **可并行**（与 B1/B3/B4 文件无重叠） |
| P2 | B3 CI fmt 门禁（ci.yml） | **可并行** |
| P2 | B4 GUI 警告展示（gui/） | **可并行**（独立 workspace） |
| P3 | B5 小修集 | A6/A7/A8/A14 相互独立可并行；A9 与 A10 都触及 store/signals 调用链，建议串行在 B1 之后做（signals.rs 冲突） |

执行顺序建议：B1 单独先行（高严重度、改动最小、验证路径明确）→ B2/B3/B4 并行 → B5 按需。
全部完成后统一跑三 workspace 门禁（test/clippy/fmt）并同步 CHANGELOG [Unreleased]。

---
*审计范围：全部 11 个 Rust 源文件逐行通读 + 全部 5 个 Markdown 文档 + CI 配置；
运行态验证基于 2026-11-18 本机实测（audit CLI、ETW 采集端到端、GUI 前一轮会话验证）。*

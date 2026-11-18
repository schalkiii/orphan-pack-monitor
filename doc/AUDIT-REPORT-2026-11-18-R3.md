# orphan-audit 审计报告（第三轮，2026-11-18）

> 本报告是当日第三轮全面审计，覆盖「准确性增强批」（A1 修复 / atime 安装期过滤 /
> PSReadLine 通配 / zsh EXTENDED_HISTORY / wrapper 扩充 / 新装观察期 / 漂移信号 /
> ETW 常驻化）合入后的完整现状。前两轮：根目录 `AUDIT-REPORT-2026-11-18.md`
> （第一轮 S1–S18，历史）、`doc/AUDIT-REPORT-2026-11-18.md`（第二轮 A1–A14/B1–B5）。
> 审计纪律：本轮只登记问题与建议，不修改任何代码。
> 执行任一建议前先跑测试基线：根目录 `cargo test --workspace`（47 通过）+
> `cd gui && cargo test`（8 通过）+ 三处 `cargo clippy --all-targets -- -D warnings`。

## 一、现状描述

- **规模**：Rust 源码约 4864 行。三个独立 workspace：根（`crates/core` 引擎 + `crates/cli`）、
  `gui/`（GPUI 界面）、`etw-capture/`（ETW 采集端）。另有 `spikes/`（GPUI 可行性证据，冻结）。
- **结构**：core 分层 model / inventory / signals / verdict / store / actions / etw；
  GUI 分 main（状态与渲染）+ view（core→UI 映射，纯函数可测）；etw-capture 单二进制
  （前台采集 + install/uninstall/status 子命令）。
- **质量水平**：55 个单元测试（core 44 + cli 3 + gui 8）全绿；三 workspace clippy
  `-D warnings` 与 fmt 清零；文档（README/AGENTS/CHANGELOG/DESIGN）与当前实现同步
  （本轮同步于增强批收尾）；实机验证数据在 README「实测数据」节。
- **运行态**：本机 ETW 常驻采集已部署（计划任务 `orphan-audit-etw-capture`），
  GUI release exe 已重建冒烟。实机判定分布 1 active / 11 indirect / 16 unused。

## 二、问题清单

### C1（高）events.jsonl 无限增长 + 全量消费，常驻化后持续恶化

- **位置**：`crates/core/src/etw.rs`（read_etw_log 全量读入）、`crates/core/src/lib.rs`
  （audit_with_paths 把全部事件写入 usage_events 表，每次审计 DELETE+INSERT 全表）、
  `etw-capture/src/main.rs`（append_etw_event 只追加，无上限）。
- **描述**：常驻采集把全机进程创建都落盘——实测部署后约 20 分钟即 2264 行 / 209KB
  （本机活跃开发时段）。文件无任何滚动/老化机制；`audit` 每次全量 `read_to_string`
  并把全部事件持久化进 SQLite。数月后：文件数十 MB、audit 变慢、usage_events 表
  无意义膨胀（绝大多数事件与已装包无关）。
- **影响范围**：审计性能与存储随时间线性恶化；离线模式（report_from_store）同样
  读全量事件。

### C2（中）`install` 覆盖正在运行的部署 exe 会失败

- **位置**：`etw-capture/src/main.rs` install()。
- **描述**：常驻采集进程从 `%APPDATA%\orphan-audit\etw-capture.exe` 运行时，再次
  `install`（升级场景）`fs::copy` 目标被 Windows 锁定 → 拒绝访问报错。失败是显式的
  （不损坏数据），但用户没有可操作的指引，且这是升级的唯一路径。
- **影响范围**：etw-capture 升级流程（采集器更新后无法就地重装）。

### C3（中）uv / pipx 无 install_path → atime 与新装观察期双双失效

- **位置**：`crates/core/src/inventory.rs` parse_uv_list（丢弃头行 `(path)` 后缀）、
  parse_pipx_json（未设置 venv 路径）。
- **描述**：`atime_candidate_paths` 与 `fill_installed_at` 都依赖 install_path。uv 工具
  的路径信息在 `uv tool list` 输出里就有（`name vX.Y.Z (C:\path)`），解析后被丢弃；
  pipx venvs 在固定布局 `%LOCALAPPDATA%\pipx\venvs\<name>`（或 pipx home）。结果是
  uv/pipx 管理的包既无 atime 证据也无新装观察期（例：本机 `harbor` 只能靠 ETW）。
- **影响范围**：uv/pipx 包的信号覆盖与安全性增强均缺失。

### C4（中）CI 缺 fmt 门禁（第二轮 A3 遗留，未修）

- **位置**：`.github/workflows/ci.yml`。
- **描述**：有 test + clippy（3 个 workspace），无 `cargo fmt --check`。本地 fmt 清零
  靠自觉，PR 无机械门禁。
- **影响范围**：格式回归可进入主干。

### C5（中）GUI 丢弃 audit warnings（第二轮 A4 遗留，未修）

- **位置**：`gui/src/main.rs` apply_report()（完全不读 `report.warnings`）。
- **描述**：atime 被禁用/解析失败、采集器失败、store 写入失败等诊断只在 CLI stderr
  可见；GUI 用户看到的是无警告的完整界面，可能不知道信号缺了一路。
- **影响范围**：GUI 用户对降级状态不可见（误导清理决策的风险）。

### C6（中）enrich_with_drift 与 fill_installed_at 零测试

- **位置**：`crates/core/src/lib.rs`。
- **描述**：增强批新增的两条核心编排逻辑没有单元测试。`unused_streak_months` 有测试，
  但「streak ≥ 3 才追加证据、档位不变、非 unused 不加」的富化逻辑只在实机跑过一次
  （当前库无历史快照，实际未触发任何路径）。installed_at 的持久化 roundtrip
  （store 写读）同样无测试。
- **影响范围**：漂移信号的回归无护栏；下月快照出现后若有 bug 无法提前发现。

### C7（中）GUI 清理预览硬编码 30 天 + 无刷新入口（第二轮 A5 遗留，扩大）

- **位置**：`gui/src/main.rs` preview_cleanup()（`generate_script(&removable, 30)`）、
  启动流程（audit 只跑一次）。
- **描述**：GUI 用默认 AuditConfig（30d），预览脚本再次硬编码 30；用户在 GUI 内无法
  切换观察窗口（CLI 有 --since）也无法重新审计（shell 历史更新后须重启 GUI）。
- **影响范围**：GUI 与 CLI 能力不对等；长观察窗口用户的 GUI 预览失真。

### C8（低）ETW 证据计数包含 since 窗口外事件

- **位置**：`crates/core/src/verdict.rs` evaluate()（etw_hits 对全部 Etw 事件累加）。
- **描述**：判定档位正确（indirect 需窗口内执行），但证据行 `executed Nx` 的 N 统计
  窗口外旧事件——半年前的 5 次执行显示为「执行 5 次（经 X 触发）」。
- **影响范围**：证据语义与窗口不一致，可误导用户对「近期」的理解。

### C9（低）三处文档漂移：ETW 命令行说法过时 + GUI 提示缺常驻形态

- **位置**：`crates/core/src/model.rs` SignalSource::Etw 变体注释（仍称「含命令行」）、
  `crates/core/src/etw.rs` 模块头（同）、`gui/src/main.rs` help_panel（ETW 提示只讲
  「管理员运行 etw-capture（Ctrl+C 停止）」，未提 `install` 常驻——现在是推荐形态）。
- **影响范围**：与「CommandLine 已从内核事件移除」的实证相悖；GUI 用户不知道有
  常驻选项。

### C10（低）probe_status 全量读日志只为计数，且计数结果 UI 未用

- **位置**：`crates/core/src/etw.rs` probe_status()（read_etw_log 后 `.len()`）；
  `TrackerStatus.events_captured` 无任何展示方。
- **影响范围**：每次 audit（GUI 启动）多一次全文件读+解析；events_captured 是死数据。

### C11（低）根 Cargo.toml 未显式 exclude gui/ 与 etw-capture/

- **位置**：根 `Cargo.toml`（exclude 只有 `spikes/*`）。
- **描述**：两个子目录自带 `[workspace]`，目前不会被根 workspace 收编；但显式
  exclude 能防未来子目录 Cargo.toml 丢失 workspace 表时被意外拉入根构建
  （届时 GPUI/ferrisetw 依赖会拖慢 CLI 构建）。
- **影响范围**：构建边界的鲁棒性。

### C12（低）generate_script 的观察期留痕两分支重复；VerdictResult 测试构造散落 5 处

- **位置**：`crates/core/src/actions.rs`（candidates 为空/非空两个分支各写一遍
  skipped 留痕）；store/actions/view/cli 测试各自手写 VerdictResult 字面量
  （新增 newly_installed 字段时需 5 处同步——本次增强批已实际付出该成本）。
- **影响范围**：维护摩擦；下次加字段同样要改 5 处。
- **备注**：跨 crate 测试 helper 需要公开 API，收益存疑——建议只做 actions.rs 去重，
  字面量构造保持现状。

### C13（低）CLI drift 子命令对不存在的快照名静默按空集对比

- **位置**：`crates/cli/src/main.rs` Commands::Drift。
- **描述**：`verdict_names(不存在的ID)` 返回空集，diff 结果显示全部包为 removed，
  无任何报错；用户拼错月份会得到误导性输出。
- **影响范围**：CLI 易用性/正确性。

### C14（低）README 架构图 etw-capture 行未反映常驻形态

- **位置**：`README.md` 架构小节。
- **描述**：etw-capture 行仍写「管理员订阅 Kernel-Process 写 events.jsonl」，未提
  install/常驻；正文「ETW 与降级」已更新，架构图滞后。
- **影响范围**：文档内部小的不一致。

## 三、改进建议

### R1（对应 C1）：事件管道加窗 + 日志滚动

- **做法**：
  1. **消费端窗口过滤**：`read_etw_log` 加参数 `since: Option<DateTime<Utc>>`（None
     保留旧行为，兼容既有测试与 probe_status 计数语义）；`audit_with_paths` 传入
     `now - since_days`（放宽 1 天容差），窗口外 ETW 事件不再进入事件集——usage_events
     表、判定、离线重估全部只含窗口内事件。
  2. **采集端启动时滚动**：etw-capture 启动时若 events.jsonl 超过阈值（建议
     `MAX_LOG_BYTES = 64MB`），轮转为 `events.jsonl.1`（覆盖旧轮转文件）后新建。
     追加路径不变，崩溃安全语义不变。窗口过滤保证 64MB 旧事件丢弃不损失判定能力
     （默认 30d 窗口的事件量远小于 64MB）。
- **涉及文件**：`crates/core/src/etw.rs`、`crates/core/src/lib.rs`、
  `etw-capture/src/main.rs`。
- **验证**：`read_etw_log` 新增单测（窗口过滤保留/剔除旧行）；etw-capture 编译 +
  手动造大文件验证轮转；实机 `audit` 后查 usage_events 行数 ≈ 窗口内事件数。

### R2（对应 C2）：install 先停再拷

- **做法**：install() 在 copy 前检测 dest 进程在跑（tasklist 判定，复用 status 的
  检测逻辑抽成 `capture_running()`）；在跑则 `schtasks /End /TN` + `stop_trace_by_name`
  + 等待退出（轮询 tasklist 最多 ~10s），再执行 copy。拷贝完成后照常 `/Run` 启动。
- **涉及文件**：`etw-capture/src/main.rs`。
- **验证**：常驻运行中重跑 `install` 全流程成功（本机实测）；status 确认新版在跑。

### R3（对应 C3）：uv/pipx 补 install_path

- **做法**：parse_uv_list 解析头行 `(path)` 括号内容设为 `install_path`（uv 的 path
  即 venv 目录，bin 在其 `Scripts/`（Windows）或 `bin/`——直接把 install_path 设为
  path，atime_candidate_paths 的 Pip 分支语义正好匹配 venv 布局，uv 复用该布局推断）；
  parse_pipx_json 从 pipx home（`%LOCALAPPDATA%\pipx` 或 `PIPX_HOME`）推
  `venvs/<name>` 设 install_path。`atime_candidate_paths` 增补 Uv/Pipx 分支
  （venv 根 → Scripts/ 与 bin/，与 Pip 同式）。
- **涉及文件**：`crates/core/src/inventory.rs`、`crates/core/src/lib.rs`。
- **验证**：parse_uv_list 单测补 `(path)` 用例断言 install_path；
  atime_candidate_paths 单测补 Uv/Pipx 用例；实机 audit 后 uv/pipx 包出现 installed_at
  （--json 检查）。

### R4（对应 C4）：CI 加 fmt 门禁

- **做法**：ci.yml 每个 workspace 的 test 步骤前各加
  `cargo fmt --all -- --check`（根/gui/etw-capture 三处）。
- **涉及文件**：`.github/workflows/ci.yml`。
- **验证**：本地三处 fmt --check 已清零（当前基线满足）；YAML 语法检查
  （`python -c "import yaml,sys;yaml.safe_load(open(sys.argv[1]))"` 或等价）。

### R5（对应 C5）：GUI 展示 warnings

- **做法**：AppState 加 `warnings: Vec<String>`；apply_report 存入；渲染在状态行下方
  或帮助面板顶部——取简单方案：status_line 拼接警告数，帮助面板顶部逐条黄色展示。
  空则不渲染。
- **涉及文件**：`gui/src/main.rs`。
- **验证**：gui workspace 测试（若抽纯函数 warnings_render 则直接测）；手动运行 GUI
  观察无警告时不占位。

### R6（对应 C6）：补 enrich_with_drift / fill_installed_at / installed_at roundtrip 测试

- **做法**：lib.rs 测试模块新增：(a) in-memory store 写 2026-06 active + 07/08/09
  unused 快照 → 构造 Unused 的 VerdictResult → enrich_with_drift 断言 streak=3 追加
  证据、streak=2 不追加、verdict 保持 Unused；(b) temp 目录创建假二进制文件 →
  fill_installed_at 断言 installed_at 被填充、已有值不覆盖；(c) store 写带 installed_at
  的 package → packages() 读回一致。
- **涉及文件**：`crates/core/src/lib.rs`（测试模块）、`crates/core/src/store.rs`（如需
  测试辅助）。
- **验证**：`cargo test -p orphan-audit-core` 全绿且新用例真实断言。

### R7（对应 C7）：GUI 观察窗口切换 + 重新审计

- **做法**：AppState 加 `since_days: u32`（默认 30）；工具条加一组档位切换
  （30 天 / 90 天 / 1 年）；切换后用**已缓存的 results 不可行**（since 影响判定），须
  重跑 audit——同时加「重新审计」按钮（复用启动时的后台审计 spawn 模式，期间置
  loading）。preview_cleanup 与报告传 `self.since_days`。若一次性改动过大，最小版本：
  只加「重新审计」+ since 档位，事件缓存不引入。
- **涉及文件**：`gui/src/main.rs`（必要时 view.rs）。
- **验证**：gui 测试；手动切换档位后 Summary/判定变化、预览脚本 since 值随之变化。

### R8（对应 C8）：ETW 证据计数限窗口内

- **做法**：evaluate() 中 Etw 分支仅当事件时间在 since 窗口内才计入 etw_hits 与
  last_exec（has_recent_exec 语义不变）；窗口外事件完全忽略（不进证据链）。
  注意：这同时让 R1 的窗口过滤成为双保险。
- **涉及文件**：`crates/core/src/verdict.rs`。
- **验证**：verdict 单测新增：窗口外 ETW 事件不产生 executed 证据行、不判 indirect。

### R9（对应 C9）：文档漂移修复

- **做法**：model.rs SignalSource::Etw 注释与 etw.rs 模块头去掉「含命令行」表述
  （改为「含父进程与时间戳；命令行在较新构建已移除」）；gui help_panel ETW 提示改为
  优先推荐 `etw-capture install`（常驻，一次部署登录自启），前台运行为备选。
- **涉及文件**：`crates/core/src/model.rs`、`crates/core/src/etw.rs`、`gui/src/main.rs`。
- **验证**：`grep -rn "含命令行\|命令行、父进程" crates/` 无残留；gui 测试不受影响。

### R10（对应 C10）：probe_status 轻量化

- **做法**：events_captured 改为 BufReader 逐行计数（不解析 JSON）；或直接删除该
  字段与 reason 之外的死数据（若删字段需同步 TrackerStatus 使用方——GUI/CLI 只读
  available，可安全删）。推荐：删字段，简化结构。
- **涉及文件**：`crates/core/src/etw.rs`、引用处（grep TrackerStatus）。
- **验证**：全 workspace 编译 + 测试。

### R11（对应 C13）：drift 校验快照存在性

- **做法**：Drift 命令在 old/new 任一不在 `snapshot_ids()` 时打印错误与可用列表并
  返回非零（anyhow bail）。
- **涉及文件**：`crates/cli/src/main.rs`。
- **验证**：手动 `orphan-audit drift 2025-01 2026-11` 报错并列出可用快照。

### R12（对应 C12/C11/C14）：小项清理

- **做法**：actions.rs 观察期留痕抽成局部闭包/函数消除两分支重复；根 Cargo.toml
  exclude 补 `gui`、`etw-capture`；README 架构图 etw-capture 行补「install 可常驻」。
- **涉及文件**：`crates/core/src/actions.rs`、根 `Cargo.toml`、`README.md`。
- **验证**：对应 workspace 测试 + fmt + clippy。

## 四、优先级与并行性

| 优先级 | 条目 | 可并行性 | 依据 |
|---|---|---|---|
| P0 | R1（事件管道加窗+滚动） | **需串行先行**：改 core etw.rs/lib.rs，R6/R8 依赖其稳定 | 常驻化后存储/性能持续恶化 |
| P1 | R2（install 先停再拷） | 可并行（仅 etw-capture） | 升级路径当前是坏的 |
| P1 | R3（uv/pipx install_path） | 可并行（inventory.rs + lib.rs 的 atime_candidate_paths，与 R1 同文件 lib.rs —— 与 R1 串行） | 信号覆盖缺口 |
| P1 | R5（GUI warnings） | 可并行（仅 gui/main.rs） | 降级不可见 |
| P1 | R6（漂移/兜底测试） | 依赖 R1 后执行（同文件 lib.rs） | 新逻辑无护栏 |
| P2 | R4（CI fmt） | 可并行（仅 ci.yml） | 机械门禁 |
| P2 | R7（GUI since+重审） | 可并行（gui，与 R5 同文件——与 R5 串行或合并执行） | GUI/CLI 能力对等 |
| P2 | R8（ETW 计数限窗） | 依赖 R1（verdict.rs 独立文件，实际可并行，但语义上 R1 先定窗口口径） | 证据语义 |
| P2 | R9（文档漂移） | 可并行 | 低成本 |
| P3 | R10/R11/R12 | 可并行（不同文件） | 小项 |

**执行顺序建议**：R1 → (R2 ∥ R3 ∥ R5 ∥ R4 ∥ R9) → (R6 ∥ R8) → R7 → (R10 ∥ R11 ∥ R12)。
每步完成后跑对应 workspace 测试再进入下一步（逐点验证纪律）。

## 五、执行状态（2026-11-18 当日追加）

**R1-R12 全部执行完毕**，门禁全绿：61 测试（core 49 + cli 3 + gui 8 + etw-capture 1）、
三 workspace clippy `-D warnings` 0 错、`fmt --check` 干净。

| 条目 | 状态 | 备注 |
|---|---|---|
| R1 窗口过滤 + 64MB 轮转 | ✅ 已执行+测试 | `read_etw_log_since` / `rotate_log_if_huge` |
| R2 install 先停再拷 | ✅ 已执行+**实机验证** | 运行中升级路径实测通过（停旧→拷贝→重启） |
| R3 uv/pipx install_path | ✅ 已执行+测试 | 观察期兜底覆盖全部五管理器 |
| R4 CI fmt | ✅ 已执行 | 三 workspace 各 fmt/test/clippy |
| R5 GUI 警告横幅 | ✅ 已执行 | 黄条常驻 + 测试 |
| R6 漂移/兜底测试 | ✅ 已执行 | streak 3 触发 / 2 不触发等 4 例 |
| R7 GUI since 档位 | ✅ 已执行 | 30d/90d/1y 切换重跑 + 重新审计按钮 |
| R8 ETW 计数限窗 | ✅ 已执行+测试 | 窗口外事件不进 etw_hits/last_exec/证据 |
| R9 文档漂移 | ✅ 已执行 | etw.rs/model.rs 注释与 DESIGN.md 对齐 |
| R10 TrackerStatus 精简 | ✅ 已执行 | 删 write-only 字段，probe 不再解析日志 |
| R11 drift 快照校验 | ✅ 已执行 | 不存在的快照报错并列出可用 |
| R12 小项清理 | ✅ 已执行（部分） | actions 去重 ✓；**C12 跨 crate 测试辅助明确跳过**（改动面/收益比不划算，维持各 workspace 测试自包含） |

**计划外发现与加固**（R2 实机验证时暴露）：首次常驻部署的采集进程于 07:50 意外停止，
复盘为计划任务默认设置「电池模式停止 + 72 小时执行时限」（本机为笔记本）。已在
install 中用 PowerShell `Set-ScheduledTask` 加固（`-AllowStartIfOnBatteries
-DontStopIfGoingOnBatteries -ExecutionTimeLimit Zero`），实机验证
`DisallowStartIfOnBatteries=False / StopIfGoingOnBatteries=False / PT0S`。

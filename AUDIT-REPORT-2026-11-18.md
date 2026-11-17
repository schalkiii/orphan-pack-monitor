# orphan-audit 项目审计报告（2026-11-18）

> 面向执行修改的 AI Agent。本报告只含发现与建议，**不含任何代码改动**。
> 项目根：`orphan-audit/`，下述路径均相对此根。执行任一建议前先跑测试基线：
> `cargo test --workspace` + `cd gui && cargo test`（GUI 目前无测试，仅编译校验）+ 两处 `cargo clippy --all-targets -- -D warnings`。

---

## 一、现状描述

- **规模**：12 个文本源文件、3,262 行（core 2,392 / cli 232 / gui 560 / README 78）。单元测试 27 个（core 26 + cli 1），全部通过；`clippy -D warnings` 在 core/cli workspace 与 gui workspace 均为零告警。
- **结构**：Cargo workspace（`crates/core` + `crates/cli`）+ `gui/`（独立 workspace，依赖 gpui-pre 0.3.12）+ `spikes/gpui-spike`（Phase 0 可行性证据，被 workspace exclude）。core 分层：`inventory`（采集）→ `signals`（信号）→ `verdict`（五档判定）→ `store`（SQLite 快照）+ `actions`（脚本/报告）+ `etw`（事件日志消费）。core 无 GUI 依赖。
- **质量水平**：判定引擎（verdict.rs）测试充分、注释与实现一致、规则顺序有明确设计信条；采集层与信号层主体可用（真机跑通 9 包 / 6,664 条历史 / 1 active / 8 unused，正向追踪验证判定正确）。短板集中在：① ETW 能力的陈述与实现落差；② GUI 展示层一个索引错位缺陷；③ 死配置/死 API/未使用依赖等残留；④ 采集器解析与 GUI 视图层缺测试；⑤ 发布工程件（LICENSE/CI/CHANGELOG）缺失。
- **版本管理**：git 已 init，单 commit（`b2b57ee`），无 tag、无远端、无 CI。

## 二、问题清单（七维度逐项结论）

### 1. 文档一致性 —— 3 项

**D1 [高] README 与代码对 ETW 能力的陈述过度承诺。**
位置：`README.md`「ETW 与降级」节（约 59–63 行）、`crates/core/src/lib.rs`（degraded 警告文案 "run elevated for precise execution tracking"）、`crates/core/src/etw.rs::probe_status`。
描述：README 称「有管理员权限时：可启用 Kernel-Process provider 追踪，记录命令行、父进程、时间戳」。实际仓库**没有任何 ETW 捕获实现**（无 etw-rs 依赖、无 session 启动、无守护进程），`etw.rs` 只是 JSONL 日志的**消费端**（读取外部写入的 `events.jsonl`）。`probe_status` 在提权时返回 `available: true`，但即使 available 也没有任何组件在捕获事件。
影响范围：产品核心差异化（"能归因谁在调用"）被陈述为已就绪；用户提权运行后误以为追踪已开启，实际 ETW 信号恒为 0；README 的降级说明（"无权限时自动降级"）暗示有权限时不会降级，与事实不符。

**D2 [中] etw.rs 模块注释描述的实现策略不存在。**
位置：`crates/core/src/etw.rs:11`（"优先使用 etw-rs 高层绑定；编译期特性 etw 可关闭"）。
描述：无 etw-rs 依赖、无 `etw` feature、无绑定代码。真实实现是 JSONL 读写 + 权限探测。
影响范围：接手 Agent 会按注释去找不存在的绑定与开关，浪费排障时间。

**D3 [低] inventory.rs 文档注释内部自相矛盾。**
位置：`crates/core/src/inventory.rs:64`（写 `"binaries": [...]`）vs `:80-81`（"实际字段：bins（不是 binaries）"）。
影响范围：低；同段注释前后不一致，读者需读到函数体才知真字段。

### 2. 文档缺漏 —— 3 项

**M1 [低] 无 Agent 规则文件（AGENTS.md / CLAUDE.md）。**
位置：仓库根（仅 .gitignore / Cargo.toml / Cargo.lock / README.md）。
影响范围：新会话/新 Agent 无「怎么跑、约定、红线」入口，只能通读 README 推断。

**M2 [低] ETW JSONL 数据契约无独立文档。**
位置：格式仅藏于 `etw.rs:39` 的 doc 注释（`{"binary","ts","parent","cmd"}`）。
影响范围：未来采集器/守护进程作者与消费端对不上字段时无契约可查。

**M3 [低] "Last run = never" 与 "Typed Nx" 并存的语义无文档。**
位置：`gui/src/view.rs:92-95`、`crates/core/src/actions.rs:105-108`、`crates/core/src/verdict.rs`（last_exec 仅取 ETW/atime，不取 history）。
描述：PSReadLine 历史无时间戳，history 命中计入 Typed 但不更新 last_exec，界面出现 "Typed 25x · Last run never"。这是设计取舍（时间不可考），但无任何文档解释，易被读成数据错误。
影响范围：用户对报告/ GUI 信任度。

### 3. 架构优化点 —— 4 项

**A1 [高] ETW 捕获端缺失，核心差异化未闭环。**
位置：`crates/core/src/etw.rs`（全文件仅读写/探测），全仓库无采集组件。
描述：与 D1 同源的实现面问题。路线 B 的护城河（执行归因）目前只消费外部日志，仓库内无法自产 ETW 事件。
影响范围：产品能力与竞品分析报告中「Phase 0：ETW 抓一次 cargo 启动」的承诺之间存在缺口；无管理员权限与有权限的输出在当前实现下**没有实质差别**。

**A2 [中] `report_from_store` 离线模式产出错误判定。**
位置：`crates/core/src/lib.rs:152-174`；`crates/core/src/store.rs`（`usage_events` 表只写不读，无 `events()` 方法）。
描述：该 API 用**空事件集**调用 `evaluate_all`，所有包必然判 `Unused`。同时它当前没有任何调用者（GUI 走 `audit()`）。
影响范围：公开 API 陷阱——谁接上离线模式谁就向用户展示「全部可删」。

**A3 [中] GUI 启动时同步执行完整审计，阻塞 UI。**
位置：`gui/src/main.rs:28-68`（`AppState::load()` 内调 `audit()`，spawn npm/python 等子进程）。
影响范围：冷启动可达数秒，窗口无加载态，用户观感为"卡死"。

**A4 [低] atime 路径推断只对 cargo 正确。**
位置：`crates/core/src/lib.rs:77-98`。
描述：`install_path.join(bin + ".exe")` 对 cargo（`~/.cargo/bin`）成立；npm 的 bin 在 `<prefix>/bin`、pip 的在 `Scripts/`，`install_path` 却分别是 `node_modules/<pkg>` 与 site-packages——拼出的路径不存在，atime 信号**静默缺失**。另外 `PathBuf::join` 永不失败，`.or_else` 回退分支是死代码；多 bin 包只探测第一个 bin。
影响范围：npm/pip/uv 包的弱执行证据恒缺失 → 进入 unused 的概率偏高（安全方向偏差，但削弱 indirect 档的召回）。

### 4. 代码写法 —— 8 项

**C1 [高] GUI 选中索引错位：点击行会在排序/筛选后显示错误包的证据。**
位置：`gui/src/main.rs:94`（`let selected_row = self.selected.and_then(|i| self.rows.get(i).cloned());`）与 `:327-331`（`state.selected = Some(idx)`，`idx` 是 **results 索引**）。
描述：`selected` 存的是 `results` 中的索引（`Row.index`），却用它索引**排序/筛选后**的 `self.rows` 向量。默认 Verdict 排序 + 无筛选时二者巧合对齐；一旦按 Name/Manager/Last run 排序或启用 Only removable，`rows.get(i)` 取到的是另一行——证据面板与高亮都指错包。
影响范围：用户可能基于**错误包的证据**做删除决策，属可误导安全决策的缺陷。

**C2 [中] shell 配置索引大小写处理不一致，integration 判定漏报。**
位置：`crates/core/src/signals.rs:265`（alias 分支按原大小写建键）vs `:284`（裸词分支 `to_ascii_lowercase` 建键）；查询侧 `crates/core/src/verdict.rs:185`（`shell_config_index.get(basename(key))`，不统一小写）。
描述：Windows 命令大小写不敏感，但索引键与查询键大小写不统一 → 配置里 `Bat`/`BAT` 等写法对应的包查不到 integration 证据。
影响范围：本应判 `integration` 的包被判 `unused`（误入清理候选）。

**C3 [中] `loose_integration` 是死配置：只写不读。**
位置：定义 `crates/core/src/model.rs:293-294` + `:304`；写入 `crates/cli/src/main.rs:134`。全仓库无任何读取/分支。
描述：字段文档声称控制「裸词匹配宽严」，实际 collect_shell_config 永远做宽松匹配，verdict 永远不区分宽严。CLI 也未暴露开关。
影响范围：API 误导；调用方传 `false` 无效。

**C4 [低] `last_exec.filter(|_| false)` 诡谲表达式。**
位置：`crates/core/src/verdict.rs:81`。
描述：语义等于永远 `None`（因纯文本历史时间戳不可靠，故意不展示 last_seen），但写法令人怀疑是笔误。应显式 `.with_last(None)` 并注释原因（Atuin 有真实时间戳，当前也一并丢弃，可顺带评估）。

**C5 [低] `split_commands` 把 `2>&1` 拆出伪命令段。**
位置：`crates/core/src/signals.rs:177-188`。
描述：`cmd 2>&1` 在 `&` 处切分 → 产生段 `"1"`，`real_command_name("1")` 返回 `"1"`，制造名为 `1` 的垃圾事件。不影响判定（无包叫 `1`），但污染事件集。

**C6 [低] `&PathBuf` 参数应为 `&Path`。**
位置：`crates/core/src/etw.rs:40`、`:67`。

**C7 [低] `Untransaction` trait 命名费解。**
位置：`crates/core/src/store.rs:291-299`。仅为 `unchecked_transaction()` 起别名，命名与语义相反（un-transaction 实为 begin-transaction）。

**C8 [低] GUI 排序解析自己生成的显示字符串。**
位置：`gui/src/view.rs:120-130`（`last_run.strip_suffix("d ago").and_then(parse)`）。
描述：`Row.last_run` 存显示字符串，排序时再解析回数字，展示格式与逻辑耦合，改文案即坏排序。

### 5. 测试覆盖率 —— 5 项

**T1 [中] 采集器解析逻辑测试缺口。** `collect_uv`（"name vX.Y.Z (path)" 行解析）、`collect_pipx`（venvs JSON 结构）、`collect_pip`（entry_points 输出消费）均无单测；现仅 `npm_bin_names` 与 `.crates2.json` 形状测试。位置：`crates/core/src/inventory.rs`。

**T2 [中] store 事件回写/读取与 replace 边界无测试。** `replace_inventory` 全量替换语义、`usage_events` 读回、`report_from_store` 场景均无覆盖（部分因 A2 功能缺失）。位置：`crates/core/src/store.rs`。

**T3 [低] `collect_shell_config` / `probe_atime` / `atime_enabled` 无单测。** 位置：`crates/core/src/signals.rs`。

**T4 [低] CLI 仅 `parse_since` 有测试。** Drift 输出、`--json` 形状、default_db 无覆盖。位置：`crates/cli/src/main.rs`。

**T5 [低] GUI `build_rows` / `Summary` 纯函数零测试。** 位置：`gui/src/view.rs`。

### 6. 测试严谨性 —— 3 项

**R1 [中] npm 字符串形 bin 测试固化了与 npm 实际行为不符的断言。**
位置：`crates/core/src/inventory.rs:378`（`assert_eq!(npm_bin_names(&pj), vec!["single.js"])`）。
描述：npm 对 `{"bin": "bin/single.js"}` 创建的可执行名是**包名**而非脚本文件名。当前实现返回 `"single.js"`，测试把它固化。缓解：`owns_binary` 的包名兜底通常能对上；但 scoped 包/特殊命名时会失配。

**R2 [低] `cargo_meta_parsing_shape` 依赖本机真实文件，缺失时静默通过。**
位置：`crates/core/src/inventory.rs:383-399`（`if !meta.exists() { return; }`）。CI/他人机器上可能空转，无 fixture。

**R3 [低] `parse_plain_history_counts_and_parses` 断言宽松。** 位置：`crates/core/src/signals.rs:372-384`。只 `contains` 三个名字，未断言事件总数与 `total_entries` 的对应关系。

### 7. 工程规范 —— 4 项

**E1 [中] `license = "MIT"` 声明但仓库无 LICENSE 文件。** 位置：`Cargo.toml:10` vs 仓库根。push GitHub 前必须补许可证正文。

**E2 [低] 7 个未使用依赖。** `crates/core/Cargo.toml`：`regex`、`walkdir`、`unicode-width`、`tracing`（源码零引用；`tracing` 连带 workspace.dependencies 中的声明也无消费者）；`gui/Cargo.toml`：`serde_json`、`dirs`、`chrono`（gui/src 零引用）。影响：构建产物与依赖树虚胖、审计噪音。

**E3 [低] 无 CI。** 无 `.github/workflows`，测试/clippy 靠手工。位置：仓库根。

**E4 [低] 无 CHANGELOG、无版本 tag。** 位置：仓库根。README 的「实测数据」段承担了部分变更记录职能，不适合作长期版本叙事。

## 三、改进建议（逐条：做法 / 涉及文件 / 验证方式）

> 每条建议独立可派发。「验证方式」是完成判据，缺一不可。

**S1 修 GUI 选中索引错位（对应 C1）**
- 做法：把 `selected` 的语义改为「results 索引」并用它**查找**行：`selected_row = self.selected.and_then(|i| self.rows.iter().find(|r| r.index == i).cloned())`；或在点击时存 `rows` 中的位置。推荐前者（筛选/排序重建后仍保持选中同一包）。顺带把 `rebuild()` 清空选中的行为改为「保留选中（若仍在视图内）」。
- 涉及文件：`gui/src/main.rs`
- 验证方式：单测（把「按 index 从 rows 解析 Row」抽成纯函数测）：构造 results=[A(active), B(unused)]，Verdict 排序后 rows=[B,A]，解析 selected=0（A）必须返回 A 而非 B。另手工验证：GUI 中按 Name 排序后点击任意行，证据面板包名与行一致。

**S2 统一 shell 配置索引与查询的大小写（对应 C2）**
- 做法：`collect_shell_config` 两处建键都 `to_ascii_lowercase()`；`verdict.rs` 查询侧 `shell_config_index.get(basename(key).to_ascii_lowercase())`。三处变更一次做完。
- 涉及文件：`crates/core/src/signals.rs`、`crates/core/src/verdict.rs`
- 验证方式：新增测试：索引键 "bat"（由 alias `Set-Alias Cat BAT` 建立），包 binaries=["Bat"]，evaluate 后 `in_shell_config=true` 且 verdict=Integration。`cargo test -p orphan-audit-core` 全绿。

**S3 补 MIT LICENSE 文件（对应 E1）**
- 做法：新增 `LICENSE`（MIT 正文，copyright 行用 "orphan-audit contributors"，与 Cargo.toml authors 一致）。
- 涉及文件：`LICENSE`（新建）
- 验证方式：文件存在且为标准 MIT 全文；与 `Cargo.toml` 的 `license = "MIT"` 一致。

**S4 移除 7 个未使用依赖（对应 E2）**
- 做法：从 `crates/core/Cargo.toml` 删 `regex`/`walkdir`/`unicode-width`/`tracing`；从 `gui/Cargo.toml` 删 `serde_json`/`dirs`/`chrono`；从根 `Cargo.toml` 的 `[workspace.dependencies]` 删 `tracing`。**不要**顺手动其他依赖。
- 涉及文件：`Cargo.toml`、`crates/core/Cargo.toml`、`gui/Cargo.toml`
- 验证方式：`cargo build --workspace`、`cd gui && cargo build` 全绿；`cargo tree` 无上述包；测试全绿。

**S5 ETW 能力陈述纠偏（对应 D1、D2；A1 的文档面）**
- 做法：① README「ETW 与降级」改写为现状：当前版本消费 `events.jsonl`（格式见下），**自带采集器尚未提供**；提权仅影响未来采集器，当前不改变信号集。② `etw.rs:11` 注释改为如实描述（JSONL 消费端 + probe）。③ `probe_status` 的 `available` 语义修正：无日志且无采集器时不应仅因提权就报 available——建议改字段名为 `elevated`/`log_present` 或在 reason 里写明 "no active capture component; reading log only"。④ lib.rs 的 degraded 警告文案去掉 "run elevated for precise execution tracking" 的暗示。
- 涉及文件：`README.md`、`crates/core/src/etw.rs`、`crates/core/src/lib.rs`、（可选）`crates/core/src/model.rs`（如改 TrackerStatus 字段名需同步 GUI/CLI 展示处：`gui/src/main.rs`、`crates/cli/src/main.rs`）
- 验证方式：通读 README 不再出现「可启用追踪」类承诺；GUI/CLI 状态行在无日志时显示「no ETW capture（消费日志）」；测试全绿。

**S6 修 `report_from_store` 离线判定（对应 A2）**
- 做法（二选一，推荐 a）：a) `store.rs` 增加 `events()` 读取方法（按 usage_events 表还原 Vec<UsageEvent>），`report_from_store` 用真实事件 + 真实 `history_total`（可存 metadata 表或以事件数近似）重新 `evaluate_all`；b) 直接删除 `report_from_store`（当前无调用者）。
- 涉及文件：`crates/core/src/store.rs`、`crates/core/src/lib.rs`
- 验证方式：方案 a 需新增测试：写入 1 个包 + 3 条 History 事件 → report_from_store 判定非 Unused；方案 b 需 `cargo build` 全绿且 GUI/CLI 不受影响。

**S7 处置 `loose_integration` 死配置（对应 C3）**
- 做法（二选一，推荐 a）：a) 实现：`collect_shell_config` 标记每个命中是「alias/精确引用」还是「裸词」，`evaluate` 在 `loose_integration=false` 时只认前者；CLI 暴露 `--strict-integration` 反向开关。b) 删除字段及三处写入。
- 涉及文件：`crates/core/src/model.rs`、`crates/core/src/signals.rs`、`crates/core/src/verdict.rs`、`crates/cli/src/main.rs`
- 验证方式：方案 a 测试两态（宽松判 Integration、严格判 Unused）；方案 b 构建全绿且 README 无该词。

**S8 修 npm 字符串形 bin 映射并改测试断言（对应 R1）**
- 做法：`npm_bin_names` 增加包名参数（或返回 Option 由调用方对字符串形回退为包名 basename）；`{"bin": "..."}` 形态的 bin 名 = `basename(package name)`（scoped 包取末段）；同步修正 `npm_bin_string_and_object_shapes` 断言。
- 涉及文件：`crates/core/src/inventory.rs`
- 验证方式：测试改为断言包名（如包名 "single" → bin "single"）；object 形态行为不变；全测全绿。

**S9 修 atime 路径按管理器推断（对应 A4）**
- 做法：按 manager 组装候选路径：cargo→`install_path/bin`；npm→`install_path` 的 `../../bin`（即 prefix/bin）；pip→同级 `Scripts/`；uv/pipx→各自 venv `Scripts/`。去掉永不失败的 `.or_else` 死分支；对**全部** binaries 探测而非第一个。探测不到就跳过（保持静默降级），不要报错。
- 涉及文件：`crates/core/src/lib.rs`（抽 `atime_candidate_paths(p: &Package) -> Vec<PathBuf>` 便于测试）
- 验证方式：单测各管理器的路径构造；真机 npm 包出现 atime 事件（可选）。

**S10 修 `split_commands` 重定向边界（对应 C5）**
- 做法：`&` 后若紧跟 `>` 或数字+`>`（重定向 fd），不作为段分隔；或切分后丢弃纯数字/纯重定向残段。保守起见只处理 `2>&1`/`1>&2` 两类双字符重定向。
- 涉及文件：`crates/core/src/signals.rs`
- 验证方式：`split_commands("cmd 2>&1")` 返回 `["cmd 2>&1"]` 或 `["cmd 2>", "1"]` 被过滤后不再产出 binary="1" 的事件；新增单测。

**S11 小项清理打包（对应 C4、C6、C7 + lib.rs:22 风格）**
- 做法：`verdict.rs:81` 改 `.with_last(None)` + 注释说明历史时间戳不可靠；`etw.rs` 两处 `&PathBuf`→`&Path`（调用点不动）；`store.rs` 删除 `Untransaction` trait，直接调 `self.conn.unchecked_transaction()?`（或私有方法 `fn tx(&self)`）；`lib.rs:22` `use chrono::{Utc}`→`use chrono::Utc`。
- 涉及文件：`crates/core/src/verdict.rs`、`crates/core/src/etw.rs`、`crates/core/src/store.rs`、`crates/core/src/lib.rs`
- 验证方式：`cargo test --workspace` 全绿 + 两处 clippy `-D warnings` 零告警；判定测试不变仍通过。

**S12 GUI Row 结构化天数 + 视图层单测（对应 C8、T5）**
- 做法：`Row` 增 `last_run_days: Option<u32>`（另存显示字符串），`SortKey::LastRun` 按数值排序；`view.rs` 加 `#[cfg(test)]` 覆盖 `build_rows`（筛选/排序/映射）与 `Summary::from_results`。
- 涉及文件：`gui/src/view.rs`、`gui/src/main.rs`
- 验证方式：`cd gui && cargo test` 新增≥4 个用例全绿（排序、筛选、Summary 计数、"never" 首位）。

**S13 补采集器/存储/信号测试（对应 T1、T2、T3、R2、R3）**
- 做法：uv/pipx 解析函数改为「纯函数：&str → Vec<Package>」后加行级单测（含 "(path)" 后缀、空行、多包）；store 加 events 回环测试（配合 S6）；`cargo_meta_parsing_shape` 用内置 fixture JSON（临时文件）替代真机文件；`parse_plain_history` 断言 `events.len()` 与 `total_entries` 精确值。
- 涉及文件：`crates/core/src/inventory.rs`、`crates/core/src/store.rs`、`crates/core/src/signals.rs`
- 验证方式：`cargo test -p orphan-audit-core` 新增≥6 用例全绿；不依赖本机 ~/.cargo。

**S14 CLI Drift/JSON 测试（对应 T4）**
- 做法：把 Drift 的差异计算抽纯函数 `diff_names(a: BTreeSet, b: BTreeSet)` 测试；`--json` 形状用 serde_json roundtrip 断言关键字段。
- 涉及文件：`crates/cli/src/main.rs`
- 验证方式：`cargo test -p orphan-audit` 新增≥2 用例。

**S15 GUI 异步审计与加载态（对应 A3）**
- 做法：`AppState` 增 `loading: bool`，审计移入后台线程（`std::thread` + channel 或 GPUI executor），完成前表格区显示 "Auditing…" 占位；错误路径已有 status_line 复用。
- 涉及文件：`gui/src/main.rs`
- 验证方式：手工冷启动 GUI，窗口立即出现并显示加载态，数据到达后刷新；无死锁。

**S16 工程件：CI + CHANGELOG（对应 E3、E4）**
- 做法：`.github/workflows/ci.yml`（push/PR 触发：`cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cd gui && cargo clippy --all-targets -- -D warnings`；GUI 只 clippy 不跑窗口测试）；根目录 `CHANGELOG.md` 记 0.1.0 初始能力。
- 涉及文件：`.github/workflows/ci.yml`（新建）、`CHANGELOG.md`（新建）
- 验证方式：workflow YAML 语法有效（act 或推后看 Actions 绿）；CHANGELOG 与 README 能力一致。

**S17 ETW 采集组件立项（对应 A1 实现面）**
- 做法（规划级，不在本轮执行）：新增 `crates/etw-capture`（或独立仓库）：管理员权限下用 windows crate / ETW API 订阅 Kernel-Process provider，进程创建写 `events.jsonl`（契约见 M2），配 Windows 服务或计划任务常驻。先写设计短文再实现。
- 涉及文件：（新 crate）+ `README.md` 架构图更新
- 验证方式：提权环境下真实捕获一次 `cargo --version` 并被 `audit` 判为 indirect/active。

**S18 文档小项打包（对应 D3、M2、M3）**
- 做法：`inventory.rs:64` 注释改为 `bins`（与实现一致）；README 增「ETW events.jsonl 格式」小节（字段表）；README 增「判定字段语义」小节解释 Typed/Last run/never。
- 涉及文件：`crates/core/src/inventory.rs`、`README.md`
- 验证方式：通读无矛盾；与实现逐字段核对。

## 四、优先级与并行性

| 建议 | 优先级 | 并行分组 | affected_files（写集，门禁用） |
|---|---|---|---|
| S1 GUI 选中错位 | P0 | 组 A（可并行） | `gui/src/main.rs` |
| S2 大小写统一 | P0 | 组 A（可并行） | `crates/core/src/signals.rs`, `crates/core/src/verdict.rs` |
| S3 LICENSE | P0 | 组 A（可并行） | `LICENSE`（新建） |
| S4 删未使用依赖 | P0 | 组 A（可并行） | `Cargo.toml`, `crates/core/Cargo.toml`, `gui/Cargo.toml` |
| S5 ETW 陈述纠偏 | P1 | 组 B（可并行） | `README.md`, `crates/core/src/etw.rs`, `crates/core/src/lib.rs` |
| S6 report_from_store | P1 | 组 B（可并行） | `crates/core/src/store.rs`, `crates/core/src/lib.rs` |
| S7 loose_integration | P1 | 组 B（可并行） | `crates/core/src/model.rs`, `crates/core/src/signals.rs`, `crates/core/src/verdict.rs`, `crates/cli/src/main.rs` |
| S8 npm bin 映射 | P1 | 组 B（可并行） | `crates/core/src/inventory.rs` |
| S9 atime 路径 | P2 | 组 C | `crates/core/src/lib.rs` |
| S10 重定向边界 | P2 | 组 C | `crates/core/src/signals.rs` |
| S11 小项清理 | P2 | 组 C | `crates/core/src/verdict.rs`, `crates/core/src/etw.rs`, `crates/core/src/store.rs`, `crates/core/src/lib.rs` |
| S12 GUI 视图测试 | P2 | 组 C | `gui/src/view.rs`, `gui/src/main.rs` |
| S13 补核心测试 | P2 | 组 C | `crates/core/src/inventory.rs`, `crates/core/src/store.rs`, `crates/core/src/signals.rs` |
| S14 CLI 测试 | P2 | 组 C | `crates/cli/src/main.rs` |
| S15 GUI 异步 | P3 | 组 D | `gui/src/main.rs` |
| S16 CI+CHANGELOG | P3 | 组 D | `.github/workflows/ci.yml`, `CHANGELOG.md` |
| S17 ETW 采集立项 | P3 | 组 D | （新 crate）, `README.md` |
| S18 文档小项 | P3 | 组 D | `README.md`, `crates/core/src/inventory.rs` |

**并行门禁说明**（供主 Agent 校验）：
- **组 A**（S1–S4）：四条写集互不相交，可真并行。
- **组 B**（S5–S8）：组内互不相交（S5 收 etw/lib/README；S6 收 store/lib —— **S5 与 S6 同涉 `lib.rs`，须串行或同 lane**；S7 收 signals/verdict（与 S2 相交，**必须排在 S2 之后**）；S8 收 inventory）。**修正：组 B 实际可并行的是 {S8} 与 {S5|S6 二选一先行}，S7 必须在 S2 合并后**。稳妥执行序：S5→S6（lib.rs 串行）→S7（待 S2 落地）→S8 可与 S5 并行。
- **组 C**（S9–S14）：S9/S11 同涉 `lib.rs`、`store.rs`（与 S6 相交），S10/S13 同涉 `signals.rs`，S11 与 S2/S6/S7 均相交 `verdict.rs`/`store.rs` —— **组 C 整体建议在 A/B 之后串行执行**，内部无并行收益。
- **组 D**（S15–S18）：S15/S18 与 S1 相交 `gui/src/main.rs`/`README.md`（与 S5 相交）——排最后，按序执行。
- 建议执行序：**A（并行）→ B（S8‖S5→S6→S7）→ C（串行）→ D（串行）**。
- 每条执行遵守：最小改动、逐点验证（每条完成跑一次全测）、注释同步；完成后按提交纪律收尾（lint 清零 → 提交）。

---
自查：本报告所有问题定位到文件与行号/函数，建议含做法与验证方式，零背景 Agent 可直接按 S 编号拆派。

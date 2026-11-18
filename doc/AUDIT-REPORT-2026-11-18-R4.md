# orphan-audit 第四轮审计报告（发版前体检）

> 日期：2026-11-18。范围：全仓（root / gui / etw-capture 三个 workspace）+ 文档 + 发版就绪度。
> 背景：第三轮审计（R3 报告）R1-R12 已全部执行完毕；本轮新增 degraded 语义修正与
> GUI 忽略入口之后、**发版之前**的全面体检。前序报告：
> 根目录 `AUDIT-REPORT-2026-11-18.md`（第一轮，历史）、`doc/AUDIT-REPORT-2026-11-18.md`
> （第二轮，A1-A14）、`doc/AUDIT-REPORT-2026-11-18-R3.md`（第三轮，C1-C14/R1-R12）。

## 一、现状描述

- 规模：~5,380 行 Rust（core 3421 + cli 306 + gui 1275 + etw-capture 377），
  三个独立 workspace（root = crates/core + crates/cli；gui；etw-capture）。
- 测试：**62 个单元测试全绿**（core 50 + cli 3 + gui 8 + etw-capture 1）；
  三 workspace clippy `-D warnings` 0 错、`cargo fmt --check` 干净（2026-11-18 实测）。
- 结构：inventory（采集）/ signals（历史·配置·atime）/ etw（事件日志消费）/
  verdict（五档判定）/ store（SQLite 快照）/ actions（脚本生成）分层清晰；
  GUI 只做展示与交互（view 层纯函数可测，main 层状态机）。
- 运行态：ETW 常驻采集已在本机部署运行（计划任务已加固电池容忍 + 无时限，
  日志 64MB 轮转 + 消费端窗口过滤）；GUI release 冒烟通过。
- 质量水平：核心逻辑（verdict/store/signals 解析）测试密度高、断言具体值；
  文档（README/CHANGELOG/AGENTS/DESIGN）与前两轮修复保持同步，本轮发现少量计数漂移。

## 二、问题清单

| # | 位置 | 问题 | 严重度 | 影响 |
|---|------|------|--------|------|
| D1 | 根/gui/etw-capture `Cargo.toml`、`CHANGELOG.md`、`README.md`、`AGENTS.md` | **发版就绪缺口**：仓库无任何 git tag（CHANGELOG 的 [0.1.0] 节从未对应真实发布）；三 workspace 版本仍 0.1.0，而 [Unreleased] 已积累大量功能（ETW 采集端/常驻化/观察期/漂移/GUI 中文化等），按 SemVer 应为 0.2.0；README「实测数据」与 AGENTS「当前状态」的测试数写 61，实际 62（本轮 core 新增 tracker 语义测试） | **中** | 直接发布会把未定版的变更当 0.1.0 发出；文档计数漂移会累积（上一轮 55→61 已改过一次） |
| D2 | `crates/core/src/verdict.rs:149-152` `Verdict::Unused::description()` | unused 描述文案 "No signal from any source" 与**窗口外 atime 证据**自相矛盾：atime 事件不按窗口过滤（ETW 已限窗，R8），60 天前的访问仍进证据链（正确展示日期），verdict 落 unused——用户看到「未用」却带着一条「访问时间」证据行，文案与证据冲突 | 低 | 信任成本：证据面板自我矛盾会让用户怀疑判定正确性（判定本身正确——窗口内确实无执行） |
| D3 | `crates/core/src/inventory.rs:167` | **A14 遗留未修**：`npm_prefix().unwrap_or_default()` 失败时 prefix 为空 → `install_path` 变相对路径 `node_modules/<pkg>`（垃圾数据入库与显示；atime 候选路径也据此拼错）。第二轮审计 A14 登记，R3 批未包含 | 低 | 罕见路径（`npm root -g` 失败）下的脏数据；观察期兜底与 atime 探测失效 |
| D4 | `gui/src/main.rs` | GUI 状态机（start_audit/apply_report/忽略列表）无单元测试（8 个测试全在 view.rs）。GPUI 上下文难 headless 构造 | 低 | 已知可接受缺口：交互逻辑靠编译 + clippy + 冒烟覆盖；记录不修 |
| D5 | `etw-capture/src/main.rs` | A12 部分改善（rotate 有测试）；PID→父名缓存、事件映射逻辑仍无自动化测试（需提权真机验证） | 低 | 已知可接受缺口；记录不修 |
| D6 | `.github/workflows/ci.yml` | 无 release 构建 job（产物需本地构建上传）。对本次发版非阻塞（gh release + 本地产物即可） | 低 | 可选增强，记录不修 |
| D7 | `crates/core/src/verdict.rs` `evaluate_all` | A11 遗留：事件匹配 O(包数×事件数)。当前规模（28 包/数千事件）无感 | 低 | 已知可接受（HashMap 索引优化留待规模化需求出现）；记录不修 |
| D8 | `crates/cli/src/main.rs` `ManagerArg` | A13 遗留：手写九变体双维护点（编译期可发现） | 低 | 已判定可接受；记录不修 |

## 三、改进建议

### E1（对应 D1，P1）发版定版

- **做法**：
  1. 三个 `Cargo.toml` 版本 0.1.0 → 0.2.0（root workspace + gui + etw-capture）；
  2. `CHANGELOG.md`：`## [Unreleased]` → `## [0.2.0] - 2026-11-18`；
  3. README「实测数据」与 AGENTS「当前状态」测试数 61 → 62；
  4. （提交后）打 tag `v0.2.0`、push、`gh release create` 附三平台产物。
- **文件**：`Cargo.toml`、`gui/Cargo.toml`、`etw-capture/Cargo.toml`、`CHANGELOG.md`、`README.md`、`AGENTS.md`。
- **验证**：`cargo metadata` 版本一致；全量门禁重跑仍绿；`git tag` + GitHub release 页面可见。

### E2（对应 D2，P2）unused 描述与窗口语义对齐

- **做法**：`Verdict::Unused::description()` 改为
  `"No signal from any source in the audit window"`——窗口外 atime 证据展示保留
  （信息有价值且带日期），只修正文案口径；CLI 报告与 README 判定规则同步措辞
  （「五路信号全静默」→「审计窗口内五路信号全静默」）。
- **文件**：`crates/core/src/verdict.rs`、`README.md`。
- **验证**：`cargo test -p orphan-audit-core`（现有用例引用该文案的同步检查）；
  GUI 无需改动（view.rs 空证据行文案仅在证据链为空时出现，无冲突）。

### E3（对应 D3，P2）npm prefix 失败时 install_path 置 None

- **做法**：`collect_npm` 中 `npm_prefix()` 改为 `Option`，失败时 `install_path = None`
  （atime/观察期对无路径包自然跳过，不产生相对路径垃圾数据）。
- **文件**：`crates/core/src/inventory.rs`。
- **验证**：新增单测：模拟 prefix 失败 → `install_path.is_none()`；全量 core 测试。

## 四、优先级与并行性

| 优先级 | 条目 | 可并行性 | 依据 |
|---|---|---|---|
| P1 | E1 发版定版 | **需最后执行**（版本号/CHANGELOG 依赖其余改动全部完成） | 发版阻塞项 |
| P2 | E2 unused 文案 | 可并行（verdict.rs + README） | 证据一致性 |
| P2 | E3 npm prefix | 可并行（inventory.rs，与 E2 无文件重叠） | A14 收尾 |

**执行顺序**：E2 ∥ E3 → 全量门禁 → E1 → 提交/tag/push/release。
D4-D8 为记录不修项（前两轮已判定可接受或属可选增强），无需排期。

## 五、七维结论

①文档一致性——D1 计数漂移与版本缺口、D2 文案口径；②文档缺漏——无新增（store schema 仍可从代码读懂，维持二轮结论）；③架构——无新问题（D7 为已知规模化留待项）；④代码写法——D2/D3；⑤测试覆盖率——D4/D5 为已知缺口，core 覆盖良好；⑥测试严谨性——未发现「跑通即通过」用例，断言均检查具体值（含格式同步哨兵测试）；⑦工程规范——D1 发版流程缺口、D6 可选 release job。

## 六、执行状态（2026-11-18 当日追加）

| 条目 | 状态 | 备注 |
|---|---|---|
| E1 发版定版 | ✅ 已执行 | 三 workspace 版本 0.2.0；CHANGELOG [0.2.0] - 2026-11-18；README/AGENTS 测试数 62；tag/release 随后执行 |
| E2 unused 文案 | ✅ 已执行 | 文案实际位置 `model.rs:230`（报告 D2 误写 verdict.rs）；窗口外 atime 证据保留展示，仅对齐口径；core 50 测试全绿 |
| E3 npm prefix | ✅ 已执行（含偏差） | **偏差**：报告承诺的「模拟 prefix 失败」单测需向 `collect_npm` 注入子进程（超出最小改动），以全量 core 测试 + clippy 替代验证；修复本身 3 行，行为变化 = 失败时 install_path 为 None（原为相对路径垃圾数据） |

执行中额外发现并修复：E3 首次编辑漏改 `inventory.rs:179`（`install_path` 变 Option 后
`npm_bin_names` 调用点编译失败），逐点验证门禁当场捕获并修复，最终全绿。

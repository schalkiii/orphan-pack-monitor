# 审计报告 R14（2026-11-20，第十四轮）

主题：**收尾轮——CLI 输出层（crates/cli/src/main.rs 全文深审，v0.2.0 后未单独
深审）+ 全仓一致性扫尾**（TODO 残留 / 版本一致 / CI 基线漂移复核）。
用户指令「无限轮，止于至善」——本轮为全面收尾，若零 actionable 即宣告止于至善。

## 一、现状描述

- 深审对象：`crates/cli/src/main.rs`（829 行 = CLI 装配 + hooks 管理模块 +
  10 测试）。GUI（gui/）与采集端（etw-capture/）各自在其交付批次内逐轮验证
  （⑦-⑨ GUI 三轮、R6/R8 采集端两轮），本轮不重复深审。
- 一致性扫尾：无真实 TODO/FIXME 残留（grep 命中均为路径模式注释如
  `.opencli-XXXX`）；四 workspace 版本一致（0.2.0）；R13/R14 未新增本地门禁
  ——CI 与本地基线无漂移（R11 教训复核通过）。
- 基线：123 测试全绿（R13 复核）。

## 二、问题清单

| 编号 | 严重度 | 位置 | 问题 | 影响 |
|------|--------|------|------|------|
| F2 | 中低 | cli main.rs:552-556（Audit 的 `if cli.json` 分支） | **`--json audit` 模式下 warnings 全部丢失**——stdout 只打 results（文档化设计），但 stderr 也什么都不打（文本模式警告走 stderr，json 模式连 stderr 都没有）。R12/R13 刚把「证据源死亡必须显式」做进 warnings（import 隔离 / 零 ETW 事件），机器消费方（管道/jq/自动化）却一条都看不到——显式降级原则在机器路径上失效（R12 取证期间实测：--json 跑看不到隔离警告，要切回文本模式才见） | 自动化场景静默降级——正是 R12 修的那类问题，只是消费方从人换成机器 |
| F3 | 中低 | cli main.rs:470-474（hooks status 的 import log 行） | **日志被隔离时 status 误导**：`hooks status` 对缺失日志一律打 `(not created yet)`——本机实况：日志被安全代理整文件隔离只剩 .IPGSD，status 报 "not created yet"（实机正在发生）。这是用户排查 import 证据死亡时第一个会跑的诊断命令，而它在最需要说真话的场景说了假话 | 误导排障方向（用户以为钩子没生效/从没记过事件，实际是 R12 记录的 churn 隔离） |
| F1 | 低 | cli main.rs:42-43（--json 的 help 文本） | **help 声称 `--json` 对 drift 生效，实际 Drift 分支无 json 输出**（实机验证：`--json drift` 输出纯文本 "Available snapshots: …"）。R6 审计 R11「--json 覆盖范围补注」时把 drift 写进了生效列表——补注本身写错了 | 用户按 help 用 `--json drift` 会拿到不可解析的文本 |
| — | 低（记录，防复审） | cli main.rs:555 | `--json audit` 只含 results 是**文档化的设计**（help 明示「warnings 与 ETW 降级状态不在其中」）；F2 修复后机器消费方经 stderr 拿到全部 warnings，stdout 形状不变（非破坏），足够 | 无 |
| — | 低（既有记录） | cli main.rs:138-145（default_db 回退 "."）、Status 跑完整审计 | R6/R11 已记录（快路径是语义变化需拍板） | 无 |

### 七维度结论

1. **文档一致性**：F1（help vs 行为——文档错，行为对）。
2. **文档缺漏**：无。
3. **架构优化点**：无。
4. **代码写法**：F2/F3（输出分支的诚实性缺口）。
5. **测试覆盖率**：F3 修复需 `imports::shadow_path` 单测（从 import_log_warning
   提取）；F1/F2 是输出分支，按本仓库惯例做实机验证（文本模式对 json 模式、
   stdout 对 stderr 分流核对）。
6. **测试严谨性**：hooks 模块 8 测试（merge/strip 的斜杠形态/幂等/第三方保留/
   陈旧路径/溢出）与 parse_since 边界——充分。
7. **工程规范**：无（cli-only 改动，无部署红线）。

## 三、改进建议

| 优先级 | 建议 | 涉及文件 | 验证方式 |
|--------|------|----------|----------|
| P2 | **R1 json 模式警告分流 stderr**：`if cli.json` 分支在打印 results 之外，把 `report.warnings` 逐条 eprintln 到 stderr（与文本模式同一通道）；stdout JSON 形状不变 | cli/src/main.rs | 实机：`--json audit 2>/dev/null` 输出仍是纯 JSON 数组（jq 可解析）；`2>&1 >/dev/null` 恰好只含 warnings（含 R12 隔离警告） |
| P2 | **R2 hooks status 隔离感知**：core `imports.rs` 提取 `pub fn shadow_path(log: &Path) -> PathBuf`（import_log_warning 内部构造复用之）；CLI status 的日志行三分支：存在 → 字节数；缺失 + 影子在 → `quarantined by security software (<shadow>)`；缺失无影子 → `not created yet` | core/src/imports.rs + cli/src/main.rs | 单测 shadow_path；实机：本机正处隔离态，status 应显示 quarantined 行（直接复测 F3 现场） |
| P3 | **R3 help 文本纠偏**：`audit/status/drift 生效` → `audit/status 生效`（drift 无 json 分支；不实现 drift json——无消费方，实现即无主 feature；help 说真话即完成 R6 补注的本来意图） | cli/src/main.rs | `--help` 通读 |

### 执行序与并行性

R1 → R2 → R3 串行（同一文件为主）；每点即测。cli+core 改动（R2 触 imports.rs），
capture/hooks 无需重装（shadow_path 是审计侧读取助手，不改钩子行为）。

## 四、展望（不实施，记录）

- 多分钟探测爆发：继续等复发数据（R12/R13 均确认未复发）。
- `--json` 完整报告模式（含 warnings/degraded 的结构化输出）：F2 的 stderr 通道
  覆盖机器消费需求；若未来要结构化 warnings，改 stdout 形状是 breaking，需拍板。
- status 快路径 / lib-only npm 保护 / unix CI job 转正：既有记录不变。

## 五、执行记录（2026-11-20，R1/R2/R3 全部实施 + 实机验证）

- **R1** ✅：`--json audit` 分支在 stdout results 之外把 warnings 逐条
  `warning: <w>` 打到 stderr（与文本模式同通道同内容；stdout 形状不变）。
  实机：stdout 以 `[` 开头 823 行纯 JSON（jq 可解析），stderr 恰好 4 条
  warning（含 R12 隔离警告逐字核对）。
- **R2** ✅：core `imports::shadow_path(log)` 提取为 pub（import_log_warning
  内部复用，**绝不读影子内容**红线不变；+1 单测）；CLI hooks status 日志行
  三分支（在=字节数 / 缺失+影子在=quarantined by security software: <影子> /
  缺失无影子=not created yet）。实机：本机正处隔离态，status 显示 quarantined
  行（F3 现场直接复测）。
- **R3** ✅：help 纠偏为「audit/status 生效——drift 只输出文本；audit 的
  stdout 只含 results，warnings 经 stderr 另行输出」（实机 --help 核对）。
- **门禁**：core 92 + cli 10 + gui 16 + etw 6 = **124 全绿**（123→124），
  三 workspace clippy/fmt 清零。core+cli 改动（shadow_path 是审计侧读取
  助手，不改钩子行为），capture/hooks 无需重装。

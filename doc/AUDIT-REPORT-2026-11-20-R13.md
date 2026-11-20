# 审计报告 R13（2026-11-20，第十三轮）

主题：**焦点轮换——verdict.rs 判定引擎 + etw.rs 消费侧深审**（两文件自早期轮后
未单独深审；R6-R12 的改动都只旁触它们）+ 实机数据新鲜度复核。用户指令
「无限轮，止于至善」。

## 一、现状描述

- 深审对象：`crates/core/src/verdict.rs`（545 行，判定引擎 + 14 测试）、
  `crates/core/src/etw.rs`（233 行，事件日志消费端 + 5 测试）、
  `crates/core/src/lib.rs` audit_with_paths 主流程（信号编排 105-255 行）。
- 实机数据：events.jsonl 17.5MB（R12 取证时 16.9MB），最新事件 2026-11-20
  06:24:47Z（= 取证此刻）——常驻采集存活且新鲜；imports 目录仍只有
  events.jsonl.IPGSD（churn 死态，与 R12 结论一致）。
- 基线：123 测试全绿（R12 复核），三 workspace 门禁清零。

## 二、问题清单

| 编号 | 严重度 | 位置 | 问题 | 影响 |
|------|--------|------|------|------|
| M1 | 中低 | lib.rs:219-226（`if degraded` 警告条件） | **零 ETW 事件提示的提权盲区**：`degraded = !tracker.available`，而 `tracker_from_log_events` 在**提权**环境下 0 事件也返回 available（probe_status 只查提权不查采集）。提权 audit + 采集端未装/已挂 → 「ETW: available」+ 窗口内 0 事件 + **零警告**——与 R12 修复的 import 静默降级同类（证据源死亡无提示），只是触发面窄：习惯管理员终端跑 audit 的用户（新机器首跑最典型） | 工具核心差异化信号（ETW 执行证据）静默缺失，用户不知道要装 etw-capture；非提权路径有警告、提权路径反而没有——行为不一致 |
| L1 | 低 | etw.rs:129（is_elevated 注释） | 注释说「通过 fsutil 这种需要管理员的只读命令间接判断」，代码实际用 `net session`——陈旧注释（实现换过） | 误导维护者 |
| — | 低（记录，防复审） | verdict.rs:88-93 | **atime 证据的窗口不对称**：窗口外 atime 仍出证据行（带日期）；窗口外 ETW/import 完全不出（evaluate 内 `continue`）。判定不受影响（`has_recent_exec` 按 in_window 过滤，窗口外 atime 不救档），证据带真实日期可反驳，README「判定字段语义」已覆盖 Last run 语义 | 无（设计一致：atime 无计数语义、展示最后触碰日期本身是信息） |
| — | 低（记录） | etw.rs:70（read_etw_log_since） | ts 解析失败回退 `Utc::now()`——与 store.events() 同类（R11 已记录无现实触发路径）；且失败方向安全（假 indirect 不可删，unused 才是唯一可清理档） | 无 |
| — | 低（记录） | lib.rs:125-134（import 日志 16MiB 检查） | 只查明文 events.jsonl 的体积；TSD churn 死态下明文不存在、.IPGSD 影子无限增长（1.18MB+）对检查不可见。影子是安全代理的文件（红线：不可读不可动），工具侧无可做 | 无（环境约束） |
| — | 低（记录） | lib.rs:342-343（report_from_store） | 离线重估路径 tracker = probe_status()、degraded 硬编码 true——离线语义（重估历史数据不承诺采集），非缺陷 | 无 |

### 七维度结论

1. **文档一致性**：README「ETW: available/unavailable 反映运行环境（是否提权）
   与日志可读性」——读失败会产生 `[etw] log read failed` 警告（lib.rs:98），
   状态行 + 警告横幅共同呈现，判无矛盾。M1 属行为盲区而非文档失实。
2. **文档缺漏**：无（atime 窗口外语义已被「判定字段语义」段覆盖）。
3. **架构优化点**：无新问题。tracker_from_log_events 的消费端视角（读得到事件
   即可用）设计正确——M1 只是它的警告条件没跟上。
4. **代码写法**：M1 条件盲区、L1 陈旧注释。
5. **测试覆盖率**：M1 修复需补测试（零事件 + 提权 → 警告仍出）。
6. **测试严谨性**：verdict.rs 14 测试覆盖五档全边界（窗口内外/优先级/大小写/
   观察期/依赖忽略），etw.rs 5 测试覆盖往返/窗口/缺失——充分。
7. **工程规范**：无（core-only 改动，无部署红线）。

## 三、改进建议

| 优先级 | 建议 | 涉及文件 | 验证方式 |
|--------|------|----------|----------|
| P2 | **R1 零事件提示条件修正**：`if degraded` → `if etw_events == 0`（degraded ⇒ 0 事件恒成立——available 要求 events>0；反向不成立：提权 + 0 事件 available 但无警告，正是盲区）。degraded 字段本身不动（它是「环境能力」，GUI/报告状态行语义）。警告文案不变（对「没装采集」与「装了但窗口内没事件」两种形态都成立） | core/src/lib.rs | 单测：audit_with_paths 全链路（临时 db + 无 etw 日志路径）断言警告出现；现有 degraded 相关断言不回归 |
| P3 | **R2 陈旧注释修正**：fsutil → net session（纯注释，零行为） | core/src/etw.rs | 通读 |

### 执行序与并行性

R1 → R2 串行（同 workspace，几行改动）；每点即测。core-only，capture/hooks 无需重装。

## 四、展望（不实施，记录）

- 多分钟探测爆发：继续等复发数据（R12 已确认未复发）。
- .IPGSD import 影子增长：环境侧事项（OA_IMPORT_LOG 重定向是用户选项），工具侧不动。

## 五、执行记录（2026-11-20，R1/R2 全部实施 + 实机验证）

- **R1** ✅：`if degraded` → `if etw_events == 0`（附注释说明提权盲区成因）；
  stale_etw_warning 的文档注释同步（「由零事件警告路径负责」）。
- **R2** ✅：etw.rs is_elevated 注释 fsutil → net session（纯注释）。
- **验证方式（如实记录）**：
  - **超集证明**：OLD 触发 = (events==0 ∧ ¬提权)；NEW 触发 = events==0。
    NEW ⊇ OLD，差集恰为「提权 + 0 事件」盲区——条件翻转不可能引入回归。
  - **实机无回归**：`--since 1d` 真机跑（窗口内 46+ 事件）零事件警告正确
    **不出现**；R12 import 隔离警告照常触发（逐字核对）；折扣规则实捕自身
    探测流量（46 爆发 + 29 扫盘剔除——审计自身的 python 环境探测被正确
    识别为非交互爆发簇，按设计工作）。
  - **提权分支不可实测**：agent 树非提权、常驻采集存活且 `parse_since`
    钳制 ≥1d（main.rs:135 `.max(1)`），真机造不出「窗口内 0 事件」形态；
    该分支的保证来自超集证明，如实记录。
  - **验证技法沉淀（防复用踩坑）**：`APPDATA=<tmp> cargo run` 重定向**无效**
    ——`dirs::data_dir()` 走 Windows Known Folder API（SHGetKnownFolderPath），
    不读环境变量；db 可用 `--db` 重定向，ETW 日志无对应逃生舱
    （import 侧有 OA_IMPORT_LOG）。
- **门禁**：core 91 + cli 10 + gui 16 + etw 6 = **123 全绿**，三 workspace
  clippy/fmt 清零。core-only，capture/hooks 无需重装。

# 审计报告 R9（2026-11-20，第九轮）

主题：**准确性第四轮**（用户连续第四日要求「进一步提高准确性」）。当日已完成
R6/R7/R8 三轮，本轮聚焦**实机取证驱动**：对真实 events.jsonl（14.8MB）与
文件系统 atime 分布做全量聚类分析，复核 R7 折扣规则的实际覆盖面，并增量排查
未覆盖信号源。代码其余部分（store/actions/gui/cli）当日或多轮近期已审计，
本轮只做增量复核。

## 一、现状描述

- 规模：~9800 行，112 测试全绿（core 80 + cli 10 + gui 16 + etw-capture 6），
  三 workspace clippy/fmt 清零；ETW 常驻无窗口采集（R8 刚部署，真实时间戳生效）。
- 信号源现状（本轮逐一取证核实）：ETW（Kernel-Process + 4688 命令行）、
  atime（含安装触碰窗 + 日期级扫盘折扣）、shell 历史（PSReadLine **全部宿主**
  `*_history.txt` + bash + zsh EXTENDED + atuin）、import 钩子（python .pth +
  node registerHooks，node 26.12.1 支持 ESM 统一入口）、deep 模式（手动）。

## 二、问题清单

| 编号 | 严重度 | 位置 | 问题 | 影响 |
|------|--------|------|------|------|
| H1 | 高 | core verdict.rs（ETW 事件入档）+ lib.rs | **自动化冒烟探测爆发被当成使用证据**：实机取证 events.jsonl 发现 2026-11-18T06:09:24-29（UTC）6 秒内 `timeout.exe` 逐个拉起 9 个命令（pytest/tqdm/typer/playwright/uvicorn/jsonschema/pygmentize/harbor/timeout）——某 agent 的「CLI 工具可用性冒烟」批测。每个命令恰好 1 次、共享非交互父进程、秒级聚簇，但全部按普通 ETW 事件入档 | **8 个包被误判 indirect**（`executed 1x via timeout.exe`），阻断清理建议；本机 agent 密集（ETW 日志另有 DeepSeek Harness / codex 等父进程），此类探测**会复发** |
| M1 | 中 | core signals.rs `discount_swept_atimes` | **扫盘折扣两层盲区**（实机取证）：① 日期级阈值 ≥5 漏小聚类——实机 11-05 16:54:41（**同一秒**4 命令：pdfplumber×3 + pypdfium2）与 11-06 11:08:38-45（7 秒内 4 命令：runxlrd/f2py/numpy-config/curl-cffi）均为扫盘形态，只因恰逢新装、被「created+24h 安装触碰窗」兜住才没制造假证据；**老包被同规模扫盘时必漏**。② 无保护规则——被扫盘日上「有其他真实证据」的命令 atime 也被整日剔除（方向安全但证据展示失真） | 小规模扫盘（4 命令）→ 假 indirect 潜伏；真实使用包的证据链被误剔 atime 行 |
| — | 低（已排除，记录防复审） | — | 取证排除的候选：① PSReadLine 多宿主历史——已覆盖（signals.rs 按 `*_history.txt` 全量扫，VS Code 宿主文件自动纳入）；② bash/zsh/atuin 历史——已覆盖；③ winget 便携包 symlink atime——`std::fs::metadata` 本就跟随符号链接取**目标**atime，非 bug（且本机 0 个便携包）；④ node 钩子 ESM 覆盖——registerHooks 统一入口在 node 26.12.1 生效；⑤ cmd→timeout 每 30 分钟心跳（旧轮换 VBS）——binary "timeout" 无包归属，无判定影响 | 无 |

### 七维度结论（增量）

1. **文档一致性**：R8 后文档与实现同步；本轮新规则落地后需同步 README
   「两条安全增强」段（折扣语义扩展）。
2. **文档缺漏**：无新缺漏（capture.log/OA_CAPTURE_LOG 已在 R8 补齐）。
3. **架构优化点**：未发现新问题。
4. **代码写法**：H1/M1 如上；`is_interactive_shell` 为 verdict.rs 私有函数，
   供 signals.rs 复用需提为 `pub(crate)`。
5. **测试覆盖率**：缺口随问题——H1 需爆发折扣（含保护/交互父进程/分散使用
   三类反例）单测；M1 需分钟级聚类与保护规则单测。
6. **测试严谨性**：既有 112 测试断言充分。
7. **工程规范**：本轮只动 core（signals/verdict/lib），capture/hooks 无需重装；
   部署红线不涉及。

## 三、改进建议

| 优先级 | 建议 | 涉及文件 | 验证方式 |
|--------|------|----------|----------|
| P0 | **R1 探测爆发折扣 `discount_probe_bursts`**：同一**非交互**父进程 + 同一日历分钟内拉起 ≥5 个不同命令（ETW 事件，parent 字段），且这些命令在窗口内**无其他任何事件**（保护规则——有真实证据的命令一律保留）→ 剔除爆发事件并警告点名（`[etw] N commands probed in a burst via <parent>; discounted (automated smoke test?)`）。日历分钟分组的失败方向是「拆成 4+4 漏剔」＝保守保证据，安全。反例设计依据：headroom-ai（78× agent 使用、分散多天）必须不受影响；交互 shell 父进程（用户快速连打 5 个命令）不参与判定；构建流水线（node/npm 父进程 5 工具齐跑）由保护规则兜底（真被构建使用的工具必有其他证据或重复出现） | core/src/signals.rs（新函数+常量+测试）+ verdict.rs（`is_interactive_shell` 提 `pub(crate)`）+ lib.rs（audit_with_paths 与 report_from_store 两路接线+警告） | 单测五类：爆发剔除 / 交互父进程不剔 / 保护命令保留 / <5 命令不剔 / 分散使用不剔。**实机**：`audit --json --since 90d` 中 8 个假 indirect（pytest/tqdm/typer/playwright/uvicorn/jsonschema/Pygments/harbor）应全部回到 unused（各自除爆发外零证据）；headroom-ai 保持 indirect |
| P1 | **R2 atime 扫盘折扣 v2**：① 新增**分钟级层**——同一日历分钟 ≥4 个不同命令的 atime 聚类即剔除（实机三个逃逸聚类均为同秒/秒级；分钟分组失败方向同样是漏剔，安全）；② 新增**保护规则**——窗口内有任何非 atime 证据的命令，其 atime 一律保留（日期级与分钟级两层都适用）；③ 日期级 ≥5 层保留（慢扫盘兜底）。阈值依据：实机扫盘聚类 = 4/4/7/12 命令；真实使用同分钟 4 个全局 CLI 且零其他信号（ETW 常驻机器上历史/ETW 必有其一）不成立 | core/src/signals.rs（`discount_swept_atimes` 重构+测试）+ lib.rs（警告文案扩展） | 单测：4 命令同分钟剔除 / 3 命令不剔 / 保护命令保留 / 日期级慢扫仍剔除。**实机**：`--since 90d` 警告行出现分钟级剔除计数；`--since 30d` 对照无变化（11-05/11-06 聚类已被安装触碰窗过滤，不重复计） |
| P2 | **R3 文档同步**：README「两条安全增强」段补探测爆发折扣与分钟级扫盘聚类语义；CHANGELOG Fixed/Added 条目 | README.md + CHANGELOG.md | 通读 |

### 执行序与并行性

R1 → R2 串行（同文件 signals.rs + lib.rs 接线）；R3 收尾。每点即测
（software-improvement 纪律）。core-only，**无需重装 capture/hooks**。

## 四、展望（不实施，记录）

- 多分钟跨度的大型探测爆发（agent 逐个 `timeout 10 <tool>` 每分钟 2 个）——
  分钟级分组会拆成小簇漏剔，留待复发时按数据再定阈值。
- UserAssist / Prefetch / Amcache 兜底与长驻进程 import 盲区（R7/R8 已记录）。

## 五、执行记录（2026-11-20，3/3 全部实施 + 实机验证）

- **R1** ✅：`discount_probe_bursts` 落地（epoch 分钟 + 父进程聚类，阈值 5，
  非交互父进程判定复用 `is_interactive_shell`——提升为 `pub(crate)`）；
  lib.rs 两路接线，**爆发折扣先于扫盘折扣执行**（顺序是 R1 实施中发现的设计
  约束，见下）。4 个单测（爆发剔除 / 交互父进程不剔 / History 保护 / 小簇与
  分散使用不剔）。
- **实施中发现并修复「互为保护」缺陷（审计建议之上）**：首版保护规则互相救活
  ——探测执行既产生 ETW 爆发事件又触碰文件 atime（实机 11-18：执行
  06:09:24.823828、atime 惰性刷新 06:16:23.4268221，NTFS system-managed 的
  延迟批量刷新），atime 保护了爆发事件、爆发事件又保护了 atime，7 个包躲过
  两层折扣（首版实测仅 harbor 翻 unused）。修复 = **atime 不作为爆发保护**
  （保护只认历史/import/簇外 ETW）+ 爆发折扣先行 + 扫盘折扣只认存活 ETW；
  回归测试 `atime_does_not_protect_burst_events` 固化该场景。
- **R2** ✅：`discount_swept_atimes` v2（分钟层 ≥4 + 日期层 ≥5 + 保护规则）；
  2 个新测试（同分钟 4 命令剔除/3 命令保留/保护保留；慢扫盘日期层兜底）；
  既有两测试按 v2 语义更新（bin0 的 atime 因真实 ETW 保护保留；正常日 4 命令
  需分散分钟——同分钟 4 命令在新语义下本就是扫盘形态）。
- **R3** ✅：README 安全增强段重写（atime 两层语义 + 新增探测爆发折扣段）；
  CHANGELOG Fixed 两条 + 概览行；警告文案「same-day」改中性（含分钟层）。
- **门禁**：113 测试全绿（core 87 + cli 10 + gui 16 + etw-capture 6，+1 净增
  ——新增 7 个、既有断言适配）、三 workspace clippy/fmt 清零。core-only 改动，
  capture/hooks 无需重装（部署红线不涉及）。
- **实机验证（`--since 90d`）**：
  - 8 个假 indirect（pytest/tqdm/typer/playwright/uvicorn/jsonschema/Pygments/
    harbor）**全部回到 unused、证据清零**；pip+uv 非未用包 9 → 1（仅 headroom-ai）。
  - 防误伤：headroom-ai（ETW 80× 分散使用）、pnpm（ETW 6×）、tokentracker-cli
    （import 9×）证据全部保留——重复使用不成爆发簇。
  - 警告触发：`[etw] 39 one-shot executions … probe burst; discounted`（timeout
    爆发 9 条 + 其他 agent 宿主的小簇，非包命令占多数，无判定影响）+
    `[atime] 21 access-time hits look like scanner sweeps; discounted`。

# 审计报告 R12（2026-11-20，第十二轮）

主题：**准确性第五轮（实机取证驱动）**——用户指令「无限轮，止于至善」。
距 R9 取证数日，events.jsonl 增长 14.8→16.9MB（177,929 事件），复核折扣规则的
实际覆盖面与 R9 展望项；同时盘点 import 日志健康与信号源存活状态。

## 一、现状描述

- 基线：122 测试全绿（R11 复核），三 workspace 门禁清零，CI 已对齐本地基线（R11）。
- 取证数据：events.jsonl 16.9MB / 177,929 事件（11-20 之后 32,925 条）；
  **import 日志瘫痪**——`imports/` 目录只剩 `events.jsonl.IPGSD`（约 1MB，
  mtime 13:55 与本次取证的 python 进程同时刻：agent 树 python 触发 .pth 钩子
  追加 → 安全代理把**整个日志文件**隔离成 .IPGSD）。ETW 日志不受影响
  （写入方是计划任务服务进程，不在 agent 树）。
- 爆发形态复核（全量分钟聚类 + 10 分钟窗口聚类，非交互父进程过滤）：
  11-19/11-20 **无新探测爆发形态**——非交互簇全部是系统噪音（ECAgent/
  QQPYTask 等非全局包命令）、本工具自身构建（cargo→rustc/clippy-driver）或
  agent 真实使用（DeepSeek Harness→headroom、Doubao→headroom）。
  R9 展望的「多分钟跨度爆发」**未复发**，无数据支撑新阈值，继续观察。

## 二、问题清单

| 编号 | 严重度 | 位置 | 问题 | 影响 |
|------|--------|------|------|------|
| H1 | 高 | core imports.rs `read_import_events_since`（缺失即 `Ok(vec![])`）+ lib.rs | **import 日志缺失静默降级**：钩子在装、日志被安全代理整文件隔离（本机实锤），`read_import_events_since` 对缺失文件返回空且零警告——import 证据归零而用户毫无感知。ETW 侧有「degraded 必须显式」原则（README 明文），import 侧没有对称实现 | 危险方向的静默失真：靠 import 证据撑住 indirect 的包（如实测 tokentracker-cli import 9×）会无声滑向 unused——**误判可删**；用户以为钩子在采证据，实际已死 |
| — | 低（已排除，记录防复审） | events.jsonl | R9 展望「多分钟跨度探测爆发」未复发（取证见现状），无数据不定阈值 | 无 |
| — | 低（记录） | 采集性能 | 177,929 事件全量行过滤的审计耗时未量化（验证阶段顺带计时） | 无 |

### 七维度结论（增量）

1. **文档一致性**：R11 后同步；本批落地后 README import 段补隔离场景。
2. **文档缺漏**：OA_IMPORT_LOG 逃生舱在 README import 段未提（AGENTS 有）。
3. **架构优化点**：无新问题（信号源健康检查属采集层职责，放 imports.rs 恰当）。
4. **代码写法**：H1 如上；钩子写侧 append 模式自愈确认（无需工具侧重建）。
5. **测试覆盖率**：H1 需警告逻辑单测（隔离影子 / 无影子 / 日志在 / 未装钩子）。
6. **测试严谨性**：既有断言充分。
7. **工程规范**：core-only，无部署红线。

## 三、改进建议

| 优先级 | 建议 | 涉及文件 | 验证方式 |
|--------|------|----------|----------|
| P0 | **R1 import 日志缺失警告**：`imports::hooks_installed()`（钩子目录标记文件探测）+ `imports::import_log_warning(log, hooks_installed)`——日志缺失且钩子在装时：`.IPGSD` 影子存在 → 点名「被安全软件隔离，import 证据不可用，下次你自己的进程 import 时日志自愈重建」；无影子 → 「钩子在装但尚无 import 事件（或日志被移走）」。未装钩子不警告（正常态）。lib.rs `audit_with_paths` 接线 | core/src/imports.rs + lib.rs + 测试 ×4 | 单测；**实机**：当前本机正处于「钩子在装 + 日志被隔离」态，audit 应出现隔离警告；tokentracker-cli 的证据面变化如实呈现（不再含 import 行） |
| P2 | **R2 文档**：README import 段补隔离场景 + OA_IMPORT_LOG 逃生舱一句；AGENTS 批⑰ 记录事故链（agent 树 python 每次触发钩子写入 → 整文件隔离；ETW 不受影响因为服务进程写入） | README.md + AGENTS.md | 通读 |

### 执行序与并行性

R1 → R2 串行。每点即测。core-only，capture/hooks 无需重装。

## 四、展望（不实施，记录）

- 多分钟探测爆发：继续等复发数据（本轮已确认未复发）。
- import 日志的 TSD 长期共存：OA_IMPORT_LOG 重定向到非管控目录是环境侧选项
  （需用户进程统一设置环境变量，工具侧不做默认变更）。

## 五、执行记录（2026-11-20，R1/R2 全部实施 + 实机验证）

- **R1** ✅：`imports::hooks_installed()`（钩子目录标记文件探测）+
  `imports::import_log_warning(log, hooks_installed)`（.IPGSD 影子点名隔离
  成因 + 自愈说明；无影子 = 新装钩子的正常早期态；未装钩子不警告）；
  lib.rs audit_with_paths 接线（读取前检查）。单测 ×4（隔离影子/无影子/
  日志在/未装钩子），core 91 绿。
- **实机验证**：本机正处「钩子在装 + 日志被隔离」态——audit 警告精确出现
  （点名 .IPGSD 路径 + 自愈说明）；`--since 90d` 判定稳定：indirect 5 /
  rare 2 / unused 29。
- **取证深化（churn 机制，非一次性隔离）**：连续观察发现 import 日志在
  **明文/密文间闪烁**——钩子宿主进程 append 时明文 events.jsonl 短暂存在
  （审计恰在其后运行即读到：`--json` 复跑时 tokentracker-cli 仍报
  `imported 9x`），TSD 随即把整文件转成 .IPGSD（影子 1.00→1.18MB 持续
  增长）。**审计恰逢其死则 import 证据为零**——警告的价值正是把这个
  竞态窗口显式化。危险方向核查：tokentracker-cli 在日志死态下由 npm
  bin 脚本 atime 兜底，**仍 indirect，未发生误判可删的漂移**。
- **审计耗时（L2 记录项）**：全量审计（36 包、16.9MB 事件日志、4 个
  python 环境探测）实测 21.4s——大头是子进程探测等待而非 JSONL 解析，
  按需工具可接受，不优化。
- **R2** ✅：README import 段补隔离场景与 OA_IMPORT_LOG 逃生舱；AGENTS
  批⑰ 记录 churn 事故链。
- **门禁**：core 91 + cli 10 + gui 16 + etw 6 = **123 全绿**，三 workspace
  clippy/fmt 清零。core-only，capture/hooks 无需重装。

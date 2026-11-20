# 审计报告 R7（2026-11-20，第七轮）

主题：**还能不能更准**——聚焦判定准确性，七维度体检。上一轮（R6，同日）已实施 11 条建议；
本轮在新代码上复核并深挖准确性方向，全部结论以实机数据取证。

## 一、现状描述

- 规模：9730 行 Rust（core 4402 + cli 803 + gui 2933 + etw-capture 851 + build 脚本），
  三 workspace（根 / gui / etw-capture），104 测试全绿（core 74 + cli 9 + gui 16 + etw 5），
  clippy `-D warnings` + fmt 清零。
- 结构：core 分层 inventory → signals → verdict → store / actions，etw / imports 为两个
  采集日志的消费端；准确性链路 = 历史伪时间戳 + shell 配置 + ETW（含 4688 命令行）
  + import 钩子 + atime。
- 运行态：常驻 capture 运行中（events.jsonl 14MB、64MB 轮转）；hooks 覆盖 4 解释器 +
  2 uv 工具 venv + NODE_OPTIONS。
- 本轮取证方法：实机 `--json audit`（36 包）+ 全量 ETW 日志命令行扫描（172 条窗口内
  distinct 命令行 / 14MB 全量）+ 仅 atime 证据包的时间聚类分析。

## 二、问题清单

| 编号 | 严重度 | 位置 | 问题 | 影响 |
|------|--------|------|------|------|
| H1 | 高（已实证） | core lib.rs 2c 段 + signals.rs `probe_atime` | **atime 扫描器扫盘无折扣**。实机数据：11 个包（@wecom/cli、PyMuPDF、cffi、charset-normalizer、comtypes、httpx2、idna、markdown-it-py、mcp、python-dotenv、pywin32）的 atime **精确聚在同一天**（69 天前）且无任何其他信号——典型安全软件扫盘形态。当前窗口 30d 未爆发（69>30 全判 unused），但：① TSD 周期性重扫，下一次扫盘落入窗口时 11+ 包一夜翻成 indirect（假在用）；② 用户跑 `--since 90d/1y` 立即触发 | 扫盘日之后的所有审计窗口内，大批真 unused 包被误判 indirect，工具核心承诺（找出可删包）失效 |
| H2 | 中（潜伏，已核实未爆发） | core imports.rs `attribute_cmdline`（153-187 行） | **命令行路径归因两处缺陷**：① pip 包的 install_path 是**共享 site-packages 根**（同一解释器全部 pip 包同路径），任何含该路径的命令行会归因到全部 pip 包（本机 23 个）——实测窗口内 172 条 + 全量 14MB 日志 0 命中（潜伏）；② 子串匹配无边界，`node_modules\lodash` 是 `node_modules\lodash-cli` 的子串，前缀相邻包互相误归因（本机当前无碰撞对） | 一旦出现 `python C:\...\site-packages\...` 类命令行（VS Code 调试器 / 异常启动方式），全部 pip 包获得假 "imported" 证据 → 误判 indirect；证据链失真 |
| M1 | 中 | core lib.rs `tracker_from_log_events` | **ETW 事件新鲜度不可观测**：tracker 可用性只看「窗口内事件数 > 0」。capture 中途挂掉数天后，窗口内仍有旧事件 → 显示「ETW 开启」，静默盲区 | 用户以为全覆盖，实际近几天无采集；unused 判定在盲区内的置信度被高估 |
| M2 | 低 | core verdict.rs 规则 1 | Active 判定用**全量历史**计数（无窗口）——「2023 年密集敲过、此后零使用」仍判 active。方向性安全（偏保守不误删），且 PSReadLine 行级插值时间戳已具备窗口过滤的数据基础，但改动引入插值误差新风险 | 记录为设计权衡，不改动；文档明示即可 |
| L1 | 低 | hooks 写侧（assets/oa_import_hook.py / oa_node_hook.cjs） | **imports/events.jsonl 无轮转**（etw 侧有 64MB 轮转）。钩子各进程追加、审计全量读入，长期增长拖慢审计 | 数年后审计变慢；暂无准确性影响 |
| L2 | 低 | core verdict.rs evidence 文案 | `typed 6x` 未标注时段（全时段 vs 窗口内）——GUI 译「历史输入 6 次」尚可，但 CLI 英文输出有歧义 | 展示层小瑕疵 |

### 七维度结论

1. **文档一致性**：未发现问题。R6 收尾刚同步过 AGENTS/README/CHANGELOG/DESIGN；本轮抽查
   DESIGN.md 的 4688 契约（binary+parent 归因、只写解释器进程）与 main.rs 一致。
2. **文档缺漏**：L1（import 日志无轮转）与 H1（atime 语义边界）未在任何文档说明。
3. **架构优化点**：`attribute_cmdline` 的归因规则分散在 imports.rs，逻辑内聚，无需调整；
   本轮无架构级问题。
4. **代码写法**：H2（归因边界）、M1（新鲜度）如上；其余抽查（store/actions/inventory
   自 R6 后未变）无新发现。
5. **测试覆盖率**：缺口与问题一一对应——`attribute_cmdline` 无共享根/前缀碰撞用例；
   atime 无扫盘聚类用例；tracker 无陈旧事件用例。
6. **测试严谨性**：未发现「跑通即通过」用例；断言均验证具体行为。本轮新增的实机取证法
   （真实日志 + 模拟归因逻辑）可作为准确性回归的手工手段。
7. **工程规范**：三 workspace 门禁齐备；无新违规。

## 三、改进建议

| 优先级 | 建议 | 涉及文件 | 验证方式 |
|--------|------|----------|----------|
| P0 | **R1 atime 扫盘同日聚类折扣**：signals.rs 新增纯函数 `discount_swept_atimes(&mut Vec<UsageEvent>) -> usize`——按 UTC 日期直方图统计 Atime 事件的**不同 binary 数**，≥5 个 binary 同日即判定扫盘日，剔除该日全部 Atime 事件；lib.rs 在 atime 采集后、判定前调用（report_from_store 离线路径同样调用），剔除数 >0 时追加警告 `[atime] N same-day hits look like a scanner sweep; discounted`。阈值 5 的依据：实机聚类 11 个；真实用户单日用 5 个全局 CLI 且零 shell 历史 + 零 ETW + 零 import 的情形，在 ETW 常驻机器上不成立（ETW 看到一切进程创建，atime 本就是冗余弱信号；无 ETW 机器上 5+ 同日无其他信号也压倒性是扫盘） | core signals.rs + lib.rs | 新测试：6 个不同 binary 同日 atime → 全剔；4 个 → 保留；同日但 ETW/import 事件 → 非 atime 事件不动；同 binary 多次 → 计 1 个 binary。实机：`--since 90d` 跑修前/修后对比，11 个包从 indirect 回到 unused |
| P1 | **R2 命令行归因边界修复**（H2 两处）：① Pip 包跳过路径子串归因（install_path 为共享根，归因歧义；pip 的命令行归因由 `-m 模块` → top_level 映射承担）；② 非 pip 包的子串命中必须落在**边界**上（命中段后紧跟 `\` 或命令行结束） | core imports.rs | 新测试：共享 site-packages 命令行不归因任何 pip 包；`lodash` 与 `lodash-cli` 并存时只有边界命中才归因；既有 `cmdline_attribution_by_path_and_dash_m` 不回归 |
| P1 | **R3 ETW 事件新鲜度警告**：lib.rs 在读入 evs 后取 `max(timestamp)`，新增纯函数 `stale_etw_warning(newest, now, since_days) -> Option<String>`——newest 距今超过窗口的 1/4（且 ≥2 天）时返回警告 `[etw] newest event is Xd old — capture may be down`，挂入 warnings | core lib.rs | 新测试：newest=5 天前 + 窗口 30d → 警告；newest=1 天前 → 无；窗口 4d + newest=2 天前 → 无（未超 1/4） |
| P3 | **R4 import 日志超限警告**：audit 读 imports 日志成功后，文件 >16MB 追加警告提示手动清理（轮转涉及多进程并发 append 的 rename 竞态，Windows 下不安全，不做自动轮转） | core lib.rs | 手工：构造大文件走 audit 冒烟；单测可选 |
| P3 | **R5 文档补注**：README 准确性一节补 atime 折扣语义 + 命令行归因边界说明；verdict.rs 规则 1 注释补「全量历史，无窗口——方向性保守」权衡记录 | README.md + core verdict.rs | 文档通读 |

> **实施状态（2026-11-20 当日）**：R1-R5 全部实施，core 测试 74→79 全绿
> （+2 扫盘折扣 +1 新鲜度阈值 +2 归因边界）。R3 实测语义修正：4 天窗口 +
> 最新事件 2 天前**会**告警（2 > 4/4=1，报告原例的「无」算错了）；
> 实机 `--since 90d` 验证见第四节追加记录。M2 按本报告结论不改动（记录权衡）。

### 执行序与并行性

R2（imports.rs）独立可并行；R1 → R3 → R4 串行（都落 lib.rs / 同一调用段）；
R5 收尾。每点独立跑 core 测试 + 全量门禁（software-improvement 逐点验证纪律）。

## 四、验证记录（本轮取证数据存档）

- 36 包判定分布：unused 20 / indirect 13 / rare 3（active 0）。
- 仅 atime 证据包 14 个：11 个 days_ago=69（同日聚类）+ tokentracker-cli(0) + httpx(1)
  + runta-cli(1)（后三个为真实个体使用，不受聚类影响——折扣只剔同日 ≥5 聚类）。
- 全量 ETW 日志（14MB）中 `site-packages` 命令行 0 条 → H2① 当前潜伏。
- 安装路径前缀碰撞对 0 组 → H2② 当前潜伏。

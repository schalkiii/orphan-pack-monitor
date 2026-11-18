# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-11-18

> 首个正式发布版本（0.1.0 为仓库内记录的 golden 基线，未单独发布）。
> 相对 0.1.0：ETW 采集端与常驻化、新装观察期、漂移信号、GUI 全面中文化与
> 表头排序/窗口档位/会话级忽略、准确性增强（本地化解析/安装期过滤/全宿主历史/
> wrapper 还原）、事件管道治理（窗口过滤 + 64MB 轮转）、降级语义修正。

### Added

- GUI 忽略入口（补齐 CLI `--ignore` 对等能力）：证据面板「忽略此包」按钮 +
  工具条「已忽略」标签（点击 × 恢复并重跑审计）；会话级，变更即重跑完整审计
- 事件管道治理：`core::etw::read_etw_log_since` 窗口过滤——audit 只读 `--since` 窗口内
  （since-1 天）的 ETW 事件，窗口外事件不进判定与计数；采集端启动时日志超 64MB 自动
  轮转为 `events.jsonl.1`（常驻采集下日志不再无限增长）
- `install` 任务设置加固：schtasks 创建的计划任务默认「电池模式停止 + 72 小时执行时限」
  ——两者都会杀死常驻采集（2026-11-18 实测：笔记本切电池即被终止）。install 现用
  PowerShell `Set-ScheduledTask` 改为电池容忍 + 无时限（失败告警不阻断）
- GUI 审计窗口档位（30 天 / 90 天 / 1 年，切换即重跑审计，since 影响判定不能只重排缓存）、
  「重新审计」按钮、审计警告横幅（采集器警告黄条常驻展示，不再只打到终端）
- 新装观察期：`VerdictResult.newly_installed`（安装时间落在 `--since` 窗口内的包）。
  观察期包不进入清理候选——CLI `clean` 脚本与 GUI 清理预览自动跳过并留痕（脚本注释、
  报告标注、GUI 证据面板提示「新装（观察期）」）。采集器未提供安装时间的包由
  `audit` 用二进制文件创建时间兜底填充（`fill_installed_at`）
- 漂移即信号：`SignalSource::Drift` + `Store::unused_streak_months`。连续 ≥3 个月快照
  均判 unused 的包在证据链追加漂移证据（GUI 显示「连续 N 个月快照均判 unused」），
  区分「本月恰好没跑」与「长期没人用」；判定档位不变
- ETW 采集端常驻化：`etw-capture install / uninstall / status` 子命令——复制自身到
  数据目录、注册登录自启计划任务（schtasks ONLOGON /RL HIGHEST）并立即启动；
  `uninstall` 停会话删任务。已知取舍：随任务启动会显示一个控制台窗口（见 DESIGN.md）
- 采集端外部停会话可优雅退出：事件线程返回即触发停机标志（uninstall 停会话后进程
  自行结束，不再挂死在 Ctrl+C 等待上）

### Changed

- **降级判定语义修正**：ETW 可用性按「审计窗口内是否读到事件」判定
  （`tracker_from_log_events`），而非只看当前进程是否提权——常驻采集写入日志后，
  **未提权**的审计也能消费事件，旧逻辑会在此误报「no ETW events available」；
  降级警告文案同步改为可行动指引（管理员运行随仓库的 etw-capture，或
  `etw-capture install` 常驻；旧文案「采集器未随仓库提供」已过时）
- `install` 升级路径：部署目标正在运行时（Windows 锁定运行中的 exe）先停 ETW 会话、
  超时再强制结束任务，然后拷贝新版并重启——运行中升级不再直接失败
- uv / pipx 安装路径解析：`uv tool list` 的括号路径与 pipx `venvs` 目录（PIPX_HOME 优先）
  ——两管理器的包也能用文件创建时间兜底观察期判定，不再只覆盖 cargo/npm/pip
- ETW 判定计数限窗：`etw_hits` / `last_exec` / 证据只统计 `--since` 窗口内事件，
  窗口外命中不再虚增 indirect 证据
- `TrackerStatus` 精简为 `available` / `reason`：删除全仓无消费方的
  `events_captured` / `uptime_secs`（write-only，且迫使 probe 全量解析日志）
- `drift` 子命令校验快照名存在，不存在的名字直接报错并列出可用快照
  （此前按空集 diff，会把所有包误报为 removed）
- 清理脚本 / GUI 预览的观察期跳过说明去重合并（skipped-note 闭包，两条输出路径共用）
- CI 补 `cargo fmt --check` 门禁并覆盖 etw-capture workspace（root / gui / etw-capture
  三处 test + clippy + fmt）
- 历史解析增强：PSReadLine 改为枚举目录下全部 `*_history.txt`（VSCode 等新宿主的
  历史不再漏采）；zsh EXTENDED_HISTORY 行（`: <ts>:<elapsed>;cmd`）优先用行内时间戳
  而非文件 mtime；wrapper 扩充 watch/nice/stdbuf/timeout，且连带其 flag/时长参数
  剥离（`timeout 10 make` 不再被记成名为 "10" 的命令，也修复了 `time -p make` 记成
  "-p" 的潜在问题）
- GUI 全面中文化：界面文案、判定档位/信号来源/置信度名与证据细节（展示层翻译，
  core 输出格式不变）；新增「使用说明」面板（运作方式、五档判定、操作、安全边界，
  默认展开可收起，ETW 未开启时提示运行采集器）
- GUI 排序交互重做：移除工具条排序按钮，改为**点击表头排序**（当前列高亮 + ▲/▼
  方向指示，同列再点反向），新增「输入次数」「大小」两列排序（按数值而非展示文案）
- `probe_atime` 直接以命令名构造事件（签名加 `binary_name` 参数），消除调用方覆写占位
  binary 的脆弱契约（等价重构，审计 A9）
- `CREATE_NO_WINDOW` 提为 `inventory` 公开常量，etw/signals/etw-capture 的探测与
  schtasks 命令统一引用，不再内联裸数字（一致性清理，审计分支步骤 3）
- `model::UsageEvent::command_line` 文档注释与实证对齐（较新 Windows 构建的 ETW 事件
  无命令行字段）

### Removed

- write-only 的 `ignores` 表及 `Store::sync_ignores` / `Store::ignores()`：全仓无任何
  读取方，写入结果不可观测（死代码清理，审计 A10；旧库中的遗留空表无害）

### Fixed

- **A1（P1）**：`fsutil behavior query disablelastaccess` 本地化输出解析失败（中文系统
  输出带括号说明后缀 `= 2  (托管的系统，…)`，旧实现对整段做数值解析 → None），
  导致中文 Windows 上 atime 信号被静默跳过。抽出 `parse_disablelastaccess` 纯函数，
  只取 `=` 后首个空白分隔 token 再做十六进制解析（附中/英文 fixture 测试）
- atime 探测剔除安装期触碰：二进制创建后 24h 内的访问视为安装动作本身而非执行
  （`atime_is_evidence`），新装包不再因安装触碰被误判 indirect
- GUI: 点击行后，排序/筛选状态下证据面板与高亮指向错误的包（选中索引错位，可误导删除决策）
- shell 配置索引与查询大小写不一致，Windows 上 `Set-Alias Cat BAT` 类配置漏报 integration（误判 unused）
- `report_from_store` 离线模式用空事件集重建判定，所有包必判 unused
- npm 字符串形 `bin` 字段映射为脚本文件名，应为包名（scoped 包取末段）
- atime 探测路径仅对 cargo 正确；现按管理器推断（cargo/npm/pip），并对全部 binaries 探测
- `split_commands` 把 `cmd 2>&1` 拆出名为 "1" 的伪命令段
- GUI "Last run" 排序改用结构化天数字段，不再解析显示字符串

### Added

- ETW 采集端 `etw-capture/`（S17，独立 workspace）：管理员订阅 Microsoft-Windows-Kernel-Process
  进程创建事件（ferrisetw UserTrace，keyword 0x8010 含 rundown 预热），按 core 契约追加
  `events.jsonl`；PID→ImageName 缓存解析父进程名；非管理员启动即报错；同名残留会话幂等清理。
  实测（Win11 build 28000）：该事件无 CommandLine 字段，`cmd` 恒为 null，归因不受影响
  （详见 etw-capture/DESIGN.md）
- MIT LICENSE 文件
- GUI 审计移入后台线程：窗口立即出现并显示 "Auditing…" 加载态，冷启动不再阻塞
- `Store::events()` 事件读取；`report_from_store` 以库中真实事件重估判定
- 采集器纯函数解析（`.crates2.json` / `uv tool list` / `pipx list --json` / pip entry_points）及单测，
  `.crates2.json` 测试改用内置 fixture，不再依赖本机文件
- CLI 漂移对比 `diff_names` 纯函数与 `--json` 输出形状测试
- CI（GitHub Actions：test + clippy，root 与 gui 两个 workspace）
- GUI 视图层单测（选中解析、筛选、排序、Summary 计数）

## [0.1.0] - 2026-11-18

### Added

- 五档判定引擎（active / indirect / rare / integration / unused）+ 可反驳证据链
- 采集器：cargo / npm / pip / uv / pipx 全局包清单与包→二进制映射
- 信号：PSReadLine / bash / zsh / Atuin 历史解析、shell 配置与 alias 索引、NTFS atime 探测、
  ETW 事件日志消费（`events.jsonl`，采集端为规划组件）
- SQLite 存储 + 月度快照与漂移对比
- CLI：`audit` / `status` / `clean` / `drift` 子命令，支持 `--json` / `--since` / `--ignore` / `--manager`
- GPUI 桌面界面：仪表盘 + 包表格 + 证据面板 + 清理预览，支持筛选/排序

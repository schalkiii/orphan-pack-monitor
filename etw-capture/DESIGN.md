# etw-capture 设计短文（S17）

> 目标：闭环路线 B 的核心差异化——执行归因。此前仓库只有 `events.jsonl` 的**消费端**，
> 本组件补上**采集端**：管理员权限下订阅 Kernel-Process provider，把进程创建事件落盘。

## 为什么是 ETW + Microsoft-Windows-Kernel-Process

- atime 只能证明「跑过」；shell 历史会被脚本/CI/agent 绕过。
- `Microsoft-Windows-Kernel-Process`（GUID `22FB2CD6-0E7B-422B-A0C7-2FAD1FD0E716`，`logman
  query providers` 实测）在进程创建时给出 **ImageName / ParentProcessID / 时间戳**——归因的核心字段。
- 内核级、不漏报、无需 hook 进程。

## 技术选型

- **ferrisetw 1.2**（KrabsETW 的 Rust 移植）：封装 StartTrace/EnableTraceEx2/ProcessTrace 与
  TDH schema 解析，属性按名取值。比裸 `windows` crate 少约一个数量级的样板代码。
- 经实测（`logman` + ferrisetw 官方示例）：Microsoft-Windows-Kernel-Process 可用**普通
  UserTrace 会话**订阅，无需 `EVENT_TRACE_SYSTEM_LOGGER_MODE`（那是经典 KernelTrace 路径的要求）；
  管理员权限仍然必需。
- 启用关键词 `0x8010`（进程事件 0x10 + rundown 0x8000）：rundown 使会话建立时内核下发
  现存进程快照，父名缓存立即预热。

## 架构

```
etw-capture/            独立 workspace（同 gui/ 模式：重型 windows 依赖不拖累 CLI 构建）
  src/main.rs           单二进制：子命令分派 install/uninstall/status；默认前台采集
                        （权限检查 → 建 trace → 回调解析 → append events.jsonl）
  DESIGN.md             本文
```

- **契约复用**：直接调用 `orphan_audit_core::etw::append_etw_event` 写
  `%APPDATA%\orphan-audit\etw\events.jsonl`（`dirs::data_dir()`，与消费端 `read_etw_log`
  单一来源；字段 `binary/ts/parent/cmd`）。ts 为落盘时间（与内核事件时间差毫秒级，日粒度判定无影响）。
- **事件映射**：Kernel-Process 的进程创建事件（event_id=1）→
  `binary = basename(ImageName) 去 .exe`。
  **CommandLine 实测缺失**（2026-11-18，Win11 build 28000，TDH schema 返回 NotFound——
  较新构建已把命令行从该事件移除），`cmd` 恒为 null；契约本就允许 cmd 可选，
  归因（binary + parent）不受影响。若未来需要命令行，可改读 Security 4688（需审计策略），不在本期。
  `parent`：事件只带 `ParentProcessID`（数字），用会话内维护的 **PID→ImageName 缓存**
  （start/stop/rundown 事件共同刷新）解析成父进程名（如 `pwsh.exe`，供消费端
  `is_interactive_shell` 归因）；缓存未命中（父进程早于采集启动且无 rundown）则留空。
- **不做过滤**：消费端（verdict）自己按 bin 匹配；采集端忠实记录。日志按行追加，崩溃不丢已写事件。

## 运行形态

- **前台手动**（无参数）：跑着就记录，Ctrl+C 停止（ferrisetw 的 stop 机制清理会话）。
- **常驻**（`install` 子命令）：复制自身到数据目录（与 events.jsonl 同根），
  注册计划任务 `orphan-audit-etw-capture`（`schtasks /Create /SC ONLOGON /RL HIGHEST`），
  并立即 `schtasks /Run` 启动一次——不用等下次登录。
  - **升级路径**：部署目标正在运行时（Windows 锁定运行中的 exe），install 先停会话、
    超时再 `schtasks /End` 强制结束任务，然后拷贝新版并重启（2026-11-18 实测通过）。
  - **任务设置加固**（2026-11-18 实测发现）：schtasks 创建的任务默认「电池模式停止 +
    72 小时执行时限」——笔记本切电池会被调度器杀死、3 天后必被时限终止
    （首次常驻部署即因此于 07:50 意外停止）。schtasks 无对应开关，install 随后用
    PowerShell `Set-ScheduledTask` 改为 `-AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
    -ExecutionTimeLimit ([TimeSpan]::Zero)`；失败不阻断但会告警（退化为默认设置）。
  - `uninstall`：先停 ETW 会话再删任务；运行中的采集进程随会话停止自行退出
    （事件线程返回即触发停机标志）。
  - `status`：查询任务是否注册 / 采集进程是否在跑 / 日志路径、大小与修改时间。
  - **窗口形态**（2026-11-20）：release 构建为 **GUI 子系统**（crate 根
    `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`，
    debug 保持控制台子系统，`cargo run` 开发体验不变）——计划任务/登录拉起**不再弹
    控制台窗口**。交互输出经 `attach_console_if_available` 衔接：AttachConsole 挂父
    控制台 + 重定向 std 句柄（终端里跑 install/status/deep 输出照常可见；管道/无控制台
    场景静默跳过，Rust std 对空句柄写入是静默丢弃不 panic）。守护侧诊断（会话启停、
    轮转、4688 标志自愈、追加失败）落 `<数据目录>/etw/capture.log`（每行时间戳）。
- **日志治理**：常驻采集下日志无限增长，两端配合治理——采集端启动时超过 64MB 轮转为
  `events.jsonl.1`（覆盖旧轮转档）；消费端 `core::etw::read_etw_log_since` 只读
  `--since` 窗口内事件（审计主流程传 since-1 天），窗口外事件不进判定。
  真正的环形缓冲/老化不在本期。

## 权限与降级

- 非管理员启动：立即报错退出（明确提示），不静默。
- 会话名固定 `orphan-audit-etw`；异常退出后残留会话由下次启动时先停后建（幂等）。
- deep 模式用独立会话名 `orphan-audit-deep`（与常驻采集互不影响）；强杀进程会遗留
  该会话，下次 `deep` 启动时先停后建（幂等）。

## 4688 命令行并入（2026-11-19）

Kernel-Process 只给进程名，命令行是归因的关键（`pip uninstall numpy` 与
`python -m numpy` 的区分全靠它）。常驻采集另起一线程轮询 Security 日志：

- **启用链**（install 时执行，幂等、失败仅告警）：`auditpol /set` 用子类别 **GUID**
  `{0CCE922B-69AE-11D9-BED3-505054503030}` 引用「进程创建」——中文系统不认英文名
  "Process Creation"（实测）；`reg add` 置 `ProcessCreationIncludeCmdLine_Enabled=1`。
  该注册表值在本机实测会被环境（安全软件/域策略）数分钟内回写删除，事件命令行变空
  时先查它。
- **采集**：每 5 秒 `wevtutil qe Security`（2 分钟重叠窗口 + EventRecordID 断点续传，
  状态存 `events.jsonl.4688.state`，单轮上限 `/c:2000`）。直启 wevtutil 不经 cmd /c
  包装——Rust 参数拼接的 `\"` 转义 cmd 不认，会把含空格的 XPath 拆参（实测 rc=87）；
  `/uni:true` 强制 UTF-16LE 输出，绕开重定向下控制台代码页（中文系统 GBK）的乱码。
  落盘时间戳用**事件自带 SystemTime**（2026-11-20 起，`append_etw_event_at`），不是
  轮询时刻——轮询延迟不得推后事件时间（消费端按 ts 过滤判定窗口）。
- **只写解释器**（node.exe / python.exe / pythonw.exe）：其余进程 Kernel-Process 已
  覆盖，避免双计；命令行进 `cmd` 字段，归因由消费端完成。

## deep 深度模式（opt-in spike，2026-11-19）

`etw-capture.exe deep` 前台运行：订阅 FILE_IO_INIT（Create/Cleanup/Close 带路径），
把 site-packages / node_modules 下的文件打开归因为包使用，写入 import 事件日志。
实测要点（Win11 26100 / ferrisetw 1.2）：

- 路径属性在 Create（opcode 64）事件里叫 **OpenPath** 而非 FileName；Cleanup/Close/
  QueryInfo 的属性表没有路径——只处理 Create，其余丢弃。
- 内核文件事件是洪流：一次 import 触发同包上百次打开（实测 11 秒 6500 行）。每包
  每进程只记一次 + 过滤非包段（`.dist-info` / `.egg-info` / `__pycache__` / `.pth` /
  `.IPGSD` TSD 隔离副本）后压到 24 行/11 秒。
- `.dist-info` 必须过滤：pip 装卸与元数据扫描也会打开它们，不能算使用。
- CPU 代价可感知（每文件打开都进用户态回调），只作前台手动短时验证，不进常驻。

## 验证（对应审计 S17 完成判据，2026-11-18 实测通过）

1. 提权终端启动 `etw-capture`，另一终端跑 `cargo-binstall --help`（一个此前判 unused 的全局工具）。
2. 确认 `events.jsonl` 出现 `binary=cargo-binstall` 行 —— ✅ `{"binary":"cargo-binstall","parent":"bash.exe",...}`
   （父进程名经 PID 缓存正确归因；rundown 预热生效）。
3. 停止采集，跑 `orphan-audit audit`，确认 cargo-binstall 判为 `indirect` —— ✅
   evidence `{source: etw, detail: executed 1x}`，unused 计数 27→26。

## 常驻化验证（2026-11-18 实测通过）

1. 管理员终端 `etw-capture.exe install` —— ✅ 计划任务注册成功，exe 部署到
   `%APPDATA%\orphan-audit\etw-capture.exe`，任务立即启动。
2. `etw-capture.exe status` —— ✅ `Scheduled task: installed`、
   `Capture process: running`、日志持续增长。
3. 采集期间执行 `cargo-binstall --help` —— ✅ `events.jsonl` 出现对应事件行
   （5 秒窗口 383→388 行）。
4. 运行中重新 `install`（升级路径）—— ✅ 自动停止旧进程 → 拷贝新版 → 重建任务 →
   重启，采集恢复且日志继续增长。
5. 任务设置加固验证 —— ✅ `Get-ScheduledTask ... Settings` 确认
   `DisallowStartIfOnBatteries=False`、`StopIfGoingOnBatteries=False`、
   `ExecutionTimeLimit=PT0S`（无时限）。
6. 首次部署的意外停止复盘（2026-11-18 07:50）：任务默认设置「电池模式停止 + 72h 时限」
   所致（笔记本本机），加固后不再复现。

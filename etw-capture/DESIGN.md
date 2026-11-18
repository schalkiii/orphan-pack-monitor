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
  - **已知取舍**：控制台程序随任务启动会显示一个控制台窗口（最小化即可，不影响采集）。
    完全无窗需要 Windows 服务或隐藏宿主包装，不在本期。
- **日志治理**：常驻采集下日志无限增长，两端配合治理——采集端启动时超过 64MB 轮转为
  `events.jsonl.1`（覆盖旧轮转档）；消费端 `core::etw::read_etw_log_since` 只读
  `--since` 窗口内事件（审计主流程传 since-1 天），窗口外事件不进判定。
  真正的环形缓冲/老化不在本期。

## 权限与降级

- 非管理员启动：立即报错退出（明确提示），不静默。
- 会话名固定 `orphan-audit-etw`；异常退出后残留会话由下次启动时先停后建（幂等）。

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

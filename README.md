# orphan-audit

Windows 上的孤儿 CLI 工具审计器：找出你装了却长期不用的全局包，并给出可反驳的证据。
主路线 B（最近调用追踪），辅助路线 A（项目依赖静态审计接口预留）。

## 与竞品的差异

| 维度 | shelfware / dusty / dronagiri | UniGetUI / DevTool Manager | **orphan-audit** |
|---|---|---|---|
| 平台 | macOS / Linux | Windows（但无使用追踪） | **Windows 原生** |
| 核心信号 | shell 历史 + atime | 无 | **ETW 事件日志消费 + 历史 + atime + 配置 + alias** |
| 归因能力 | 只知道"跑过" | — | **事件日志就绪时知道谁跑的（父进程）、跑几次** |
| 删除方式 | 生成脚本 / 交互 | 直接卸载 | **只生成脚本，绝不自动删** |

## 架构

```
crates/core/    核心引擎（无 GUI 依赖）
  model.rs       数据模型：Package / UsageEvent / Verdict / Evidence
  inventory.rs   采集 cargo / npm / pip / uv / pipx 全局包与 bin 映射
  signals.rs     PSReadLine/bash/Atuin 历史解析、alias、shell 配置、atime
  verdict.rs     五档判定引擎（active/indirect/rare/integration/unused）
  store.rs       SQLite 持久化 + 月度快照（漂移对比）
  actions.rs     卸载脚本与报告生成
  etw.rs         ETW 事件日志消费（JSONL 读取 + 追加写入，与采集端共用契约）
crates/cli/     CLI
gui/            GPUI 桌面界面（独立 workspace，避免拖累 CLI 编译）
etw-capture/    ETW 采集端（独立 workspace；install 子命令可常驻登录自启，前台运行亦可）
spikes/         技术验证（GPUI Windows 编译 spike）
```

## 判定规则（顺序即正确性）

1. `active` — 交互命中 ≥ 自适应阈值（历史每千条计 1，钳制 3..50）
2. `indirect` — 近期执行过（含 ETW/atime 归因），即使不是用户敲的
3. `rare` — 历史出现过，但 `--since` 窗口内无执行
4. `integration` — 没跑过，但被 shell 配置引用
5. `unused` — 审计窗口内五路信号全静默，**唯一进入清理候选**

每个判定附带 evidence，可在 GUI 证据面板逐条反驳。

两条安全增强（不改变档位，只影响清理候选与证据链）：

- **新装观察期**：安装时间在 `--since` 窗口内的包不进入清理候选——装了还没来得及用
  很正常，不足一个月的静默不构成弃用证据。CLI 脚本与 GUI 预览都会跳过并留痕。
- **漂移证据**：连续 ≥3 个月快照均判 unused 的包，证据链追加「连续 N 个月快照均判
  unused」——长期没人用比单月恰好没跑更接近可删。

### 判定字段语义

- **Typed Nx**：在 shell 历史中用户亲手敲过的次数（交互信号）。
- **Last run / never**：距最近一次**执行类证据**（ETW 事件 / 文件访问时间）的天数；
  无执行证据时显示 `never`。注意"Typed 25x · Last run never"是正常状态，不是数据错误：
  纯文本 shell 历史（PSReadLine 等）没有逐条时间戳，历史命中计入 Typed 但不更新 Last run
  （时间不可考，展示具体日期会误导）；有真实时间戳的来源（ETW / Atuin）才会更新。

## 构建与运行

需要 Rust 1.85+（实测 1.99.1）。

```bash
# CLI
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p orphan-audit -- audit           # 完整报告
cargo run -p orphan-audit -- status          # 统计摘要
cargo run -p orphan-audit -- clean           # 生成卸载脚本（只输出，不执行）
cargo run -p orphan-audit -- --json audit    # JSON 输出
cargo run -p orphan-audit -- drift           # 列出可用快照

# GUI（仪表盘 + 包表格 + 证据面板 + 清理预览；支持筛选/排序/点击表头排序/
# 查看判定证据/窗口档位切换/会话级忽略包）
cd gui && cargo run
```

## ETW 与降级

- 审计端**只消费** ETW 事件日志（数据目录 `orphan-audit/etw/events.jsonl`，字段：`binary` / `ts` / `parent` / `cmd`），
  采集端是随仓库提供的 `etw-capture/` 独立 crate（设计见 `etw-capture/DESIGN.md`）。
- 采集端两种形态：
  - **常驻（推荐）**：管理员终端 `cd etw-capture && cargo build --release` 后运行
    `target\release\orphan-audit-etw-capture.exe install`——复制自身到数据目录、注册
    登录自启计划任务（ONLOGON / HIGHEST）并立即启动；任务设置已加固（电池模式不停、
    无执行时限——schtasks 默认的两项会杀死常驻采集）。运行中重复 `install` 即升级
    （自动停旧进程 → 拷贝 → 重启）。`uninstall` 停会话删任务，`status` 查看任务/进程/
    日志状态。已知取舍：随任务启动会显示一个控制台窗口（最小化即可，不影响采集）。
  - **日志治理**：常驻采集下日志超 64MB 自动轮转（`events.jsonl.1`，覆盖旧档）；
    审计端只读 `--since` 窗口内的事件（`read_etw_log_since`），窗口外事件不进判定。
  - **前台手动**：直接运行（无参数）即开始采集，Ctrl+C 停止。
  两种形态都订阅 Microsoft-Windows-Kernel-Process 的进程创建事件并落盘（rundown 预热
  父进程名缓存）；非管理员启动立即报错退出，不静默降级。采集期间执行的命令会在下次
  `audit` 中体现为 `indirect` 证据。
- 报告中的 `ETW: available/unavailable` 反映运行环境（是否提权）与日志可读性；`audit` 本身不开启任何追踪，
  在采集器写入 `events.jsonl` 之前，ETW 信号集为空，判定只依赖 shell 历史 + atime。
- 无事件日志时报告明确标注 `degraded`，不会静默给出弱证据结论。
- NTFS atime 自 Win10 1803 默认为 System Managed（启用，60 分钟粒度）；若被禁用会跳过该信号并警告。
  atime 探测会剔除「安装期触碰」：二进制创建后 24h 内的访问视为安装动作本身，不算执行证据。

## 实测数据（2026-11-18，本机）

- 采集：28 个全局包（pip 24 / cargo 2 / uv 2），历史源 PSReadLine + bash 共 7020 条
- 判定：1 active / 11 indirect / 16 unused（A1 修复 + ETW 常驻前为 1/1/26：
  atime 信号恢复后 `cargo-update`、`httpx` 获得访问时间证据；常驻采集的 ETW 事件
  为 9 个包补上 `executed 1x via timeout.exe` 执行证据，全部从 unused 救出）
- 新装观察期实测：`cargo-binstall`、`pip`（二进制创建时间在 30 天内）被标记，
  不进入清理候选
- 漂移信号：当前库只有本月快照，连续月数不足 → 正确不触发（下月起生效）
- 交叉验证：`binstall`、`llvm-cov`、`headroom` 在历史中确实 0 次出现 → unused 判定正确；
  `numpy` 仅出现在 `pip install numpy` 安装命令中，解析器取首个命令词 `pip`，未把安装误算为使用 → 判定正确。
- ETW 采集端闭环：采集器运行期间执行 `cargo-binstall --help`，停止后 `audit` 即依据
  `events.jsonl` 的执行证据（executed 1x）将其从 unused 改判 indirect，父进程正确归因为 `bash.exe`。
- ETW 常驻化实测：`install` 注册计划任务并立即启动，`status` 确认采集进程在跑、
  日志持续增长（5 秒窗口 383→388 行）；运行中重新 `install` 升级路径实测通过
  （停旧 → 拷贝 → 重启）。首次部署曾意外停止，复盘为计划任务默认「电池模式停止 +
  72h 执行时限」（笔记本本机）——install 已内置 PowerShell 加固并实测确认
  `DisallowStartIfOnBatteries=False` / `StopIfGoingOnBatteries=False` / `PT0S`。
- clippy `-D warnings` 与 `cargo fmt --check` 在三个 workspace 全部通过；62 个单元测试通过
  （core 50 + cli 3 + gui 8 + etw-capture 1）。

## 安全

- 工具**从不自动卸载**任何包。`clean` 只生成带证据注释的脚本，用户审阅后自行执行。
- 依赖包（`is_dependency`）永不进入清理候选。
- npm 的 bin 映射读 `package.json` 的 bin 字段（而非按包名猜测），低置信度在报告中标注。

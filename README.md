# orphan-audit

Windows 上的孤儿包审计器：找出你装了却长期不用的全局包，并给出可反驳的证据。
主路线 B（最近调用追踪），辅助路线 A（项目依赖静态审计接口预留）。

## 与竞品的差异

| 维度 | shelfware / dusty / dronagiri | UniGetUI / DevTool Manager | **orphan-audit** |
|---|---|---|---|
| 平台 | macOS / Linux | Windows（但无使用追踪） | **Windows 原生** |
| 核心信号 | shell 历史 + atime | 无 | **ETW 事件日志消费 + import 钩子 + 4688 命令行 + 历史 + atime + 配置 + alias** |
| 归因能力 | 只知道"跑过" | — | **事件日志就绪时知道谁跑的（父进程）、跑几次** |
| 删除方式 | 生成脚本 / 交互 | 直接卸载 | **脚本审阅或两步确认勾选卸载，绝无静默删除** |

## 架构

```
crates/core/    核心引擎（无 GUI 依赖）
  model.rs       数据模型：Package / UsageEvent / Verdict / Evidence
  inventory.rs   采集 cargo / npm（多 prefix）/ pip（多 python 环境）/ uv / pipx
                 全局包、winget 便携包与 bin 映射
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
5. `unused` — 审计窗口内所有信号全静默，**唯一进入清理候选**

每个判定附带 evidence，可在 GUI 证据面板逐条反驳。

两条安全增强（不改变档位，只影响清理候选与证据链）：

- **新装观察期**：安装时间在 `--since` 窗口内的包不进入清理候选——装了还没来得及用
  很正常，不足一个月的静默不构成弃用证据。CLI 脚本与 GUI 预览都会跳过并留痕。
- **长期未用证据**：连续 ≥3 个月快照均判 unused 的包，证据链追加「连续 N 个月快照均判
  unused」——长期没人用比单月恰好没跑更接近可删（月度快照机制的内部名是 drift）。
- **atime 扫盘折扣**：安全软件定期扫盘会把一批包的文件访问时间聚在同一日——「同日
  ≥5 个命令的 atime」或「同一分钟 ≥4 个命令的 atime」（扫盘是秒级连碰，真实使用
  不会一分钟用 4 个全局 CLI）且无其他信号的，被识别为扫盘，相关 atime 证据剔除并
  打警告，不会把真 unused 包误判成近期使用。atime 本就是最弱的执行信号（无法归因
  谁触发），真实使用总有 ETW / import / 历史证据兜底。
- **探测爆发折扣**：agent / 安全工具批量「冒烟测试」CLI 时（实测：某 agent 用
  timeout 在 6 秒内逐个启动 9 个命令），每个命令恰好 1 次的执行会被记成使用证据。
  同一非交互父进程在一分钟内拉起 ≥5 个不同命令、且这些命令无其他证据的，整簇剔除
  并打警告；重复使用（如某工具被 agent 每天 78 次调用）不受影响。
- **命令行归因边界**：ETW/4688 捕获的命令行按「包安装路径」归因时要求路径落在目录
  边界上（`node_modules\lodash` 不会误吃 `node_modules\lodash-cli` 的执行）；pip 包
  的 site-packages 是全部包共享的根，路径归因对 pip 不适用（由 `python -m 模块`
  映射承担）。

### 判定字段语义

- **Typed Nx**：在 shell 历史中用户亲手敲过的次数（交互信号）。
- **Last run / never**：距最近一次**执行类证据**（ETW / import 钩子 / 文件访问时间）的天数；
  无执行证据时显示 `never`。注意"Typed 25x · Last run never"是正常状态，不是数据错误：
  纯文本 shell 历史（PSReadLine 等）没有逐条时间戳，历史命中计入 Typed 但不更新 Last run
  （无时间戳历史按文件创建/修改时间与行位**线性插值**出每行的估计使用时间，观察窗口
  过滤可用；有真实时间戳的来源（ETW / import 钩子 / Atuin）仍优先）。

## import 钩子与 4688（准确性增强）

- **import 钩子**（`orphan-audit hooks install`，无需管理员）：为机器上每个 python
  解释器部署 `.pth` 观察钩子、为 node 写用户级 `NODE_OPTIONS --require` 钩子——进程
  加载顶层包时追加 import 事件（`orphan-audit/imports/events.jsonl`），审计时映射回
  发行包名作为执行类证据。覆盖 IDE / agent 直接 `node x.js`、`python -c` 这类终端历史
  看不到的使用。钩子运行在一切业务进程内部：吞掉一切异常、零依赖、每进程每包只记
  一次；`hooks status` / `hooks uninstall` 对称管理。uv/pipx 工具自带的 venv
  （`uv tools` / `pipx`）也一并直播 .pth（`UV_TOOL_DIR` / `PIPX_HOME` 重定向生效），
  status 显示 `tool venvs hooked` 计数。装完后**重开终端**生效。
  日志被安全软件整文件隔离时（钩子在装而日志只剩 `.IPGSD` 影子）审计会显式警告
  「import evidence unavailable」——证据降级不静默；`OA_IMPORT_LOG` 环境变量可把
  日志整体重定向到不受管控的目录（读写两侧共用同一变量）。
- **4688 命令行**（采集端 install 自动启用，需管理员）：Windows「进程创建」审计 +
  命令行包含开启后，采集端轮询 Security 日志把 node/python 进程的完整命令行并入
  事件日志，命令行经安装路径 / `python -m 模块` 归因到包。注册表标志
  `ProcessCreationIncludeCmdLine_Enabled` 可能被安全软件/域策略回写删除——采集端
  每 10 分钟自动核对两个注册表位置并重写，无需人工盯守。
- **深度模式（opt-in spike）**：`etw-capture.exe deep` 前台短时运行，把 site-packages /
  node_modules 下的文件打开事件归因成包写入 import 日志。CPU 代价可感知、噪音大，
  只作技术验证。

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
cargo run -p orphan-audit -- hooks install   # 部署 python/node import 钩子（重开终端生效）

# GUI（判定分布条仪表盘 + 管理器标签行 + 检测环境行 + 包表格 + 证据面板 + 勾选卸载；
# 支持分布条/图例点击按档位筛选/管理器标签按来源分类（可与档位叠加）/点击表头排序
# /查看判定证据/窗口档位切换/会话级忽略包；勾选卸载 = 行首勾选框 + 两步确认后直接
# 执行卸载命令并自动重审计核验；卸载残骸操作条 = 两步确认逐文件删除 + 重审计核验
# 清零；工具条「导出脚本」= clean 脚本写桌面（CLI clean 同源）；右栏面板可切
# 「长期对比」（月度快照增删，CLI drift 同源）与「钩子」（CLI hooks 同源，安装/
# 卸载/状态）；多环境时「环境」行列出全部 npm prefix 与
# python 解释器（悬停看完整路径），同名包跨多环境时行内标注归属、证据面板显示完整
# 安装路径；说明不设集中面板，「？」悬停就地融入各控件；exe 内嵌程序化生成图标）
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
    日志状态。release 构建为 GUI 子系统：登录拉起不弹控制台窗口，终端里运行子命令的
    输出经 AttachConsole 照常可见；守护侧诊断（会话启停、轮转、4688 标志自愈）落
    数据目录 `orphan-audit/etw/capture.log`。
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
- 新装观察期实测：`cargo-binstall`（二进制创建时间在 30 天内）被标记，
  不进入清理候选
- 长期未用信号：当前库只有本月快照，连续月数不足 → 正确不触发（下月起生效）
- 交叉验证：`binstall`、`llvm-cov`、`headroom` 在历史中确实 0 次出现 → unused 判定正确；
  `numpy` 仅出现在 `pip install numpy` 安装命令中，解析器取首个命令词 `pip`，未把安装误算为使用 → 判定正确。
- npm atime 补盲（2026-11-19）：IDE（Trae CN）以 node 直接运行包内 `bin/tracker.js`
  调用 tokentracker-cli、完全不触碰 prefix 下的 .cmd/.ps1 shim——atime 探测现按包
  枚举 `bin/` 下的 .js/.mjs/.cjs 入口脚本（命令名与脚本名不同的 bin 映射也覆盖），
  此类「一直在用却被判 unused」的直调工具正确改判 indirect。
- 多环境检测（2026-11-19）：npm 枚举全部全局 prefix（默认 + nvm/fnm + 指纹扫描，
  实测捕获 QClaw 的独立 `%APPDATA%\QClaw\npm-global`，+3 个包）；python 枚举全部
  环境（`py -0p` + 安装根目录 + PATH，实测 4 处：Python313 / CodeBuddy py311 /
  uv 3.12 ×2）。检测到的环境在 CLI `audit` 输出与 GUI「环境」行列出（每枚悬停
  显示完整路径）；各环境自带的 pip/setuptools/wheel 引导设施不参与审计。
- 清理脚本 Windows 化（2026-11-19）：`clean` 生成 PowerShell 脚本（此前是 bash）。
  pip 卸载按包所在环境定位（venv/系统布局直接调解释器；用户级 site-packages 经
  `py -3.13` 版本标签）；npm 非 prefix 包显式 `--prefix`——多环境下不会卸错位置。
- ETW 采集端闭环：采集器运行期间执行 `cargo-binstall --help`，停止后 `audit` 即依据
  `events.jsonl` 的执行证据（executed 1x）将其从 unused 改判 indirect，父进程正确归因为 `bash.exe`。
- ETW 常驻化实测：`install` 注册计划任务并立即启动，`status` 确认采集进程在跑、
  日志持续增长（5 秒窗口 383→388 行）；运行中重新 `install` 升级路径实测通过
  （停旧 → 拷贝 → 重启）。首次部署曾意外停止，复盘为计划任务默认「电池模式停止 +
  72h 执行时限」（笔记本本机）——install 已内置 PowerShell 加固并实测确认
  `DisallowStartIfOnBatteries=False` / `StopIfGoingOnBatteries=False` / `PT0S`。
- clippy `-D warnings` 与 `cargo fmt --check` 在三个 workspace 全部通过（etw-capture 另跑
  release 配置 clippy）；130 个单元测试通过（core 101 + cli 3 + gui 20 + etw-capture 6，
  2026-11-20 复核）。push / PR 由 CI 自动跑同一套门禁（`.github/workflows/ci.yml`）。

## 安全

- 工具**从不静默卸载**任何包。两条清理路径都由你显式触发：`clean` 生成带证据
  注释的脚本（Windows 下为 PowerShell），审阅后自行执行；GUI 勾选卸载需先勾选
  再经两步确认（按钮变红点名数量），执行后自动重审计核验包是否真的消失。
- 依赖包（`is_dependency`）永不进入清理候选。
- npm 的 bin 映射读 `package.json` 的 bin 字段（而非按包名猜测），低置信度在报告中标注。
- 多环境下的卸载精确性：pip 按包所在环境定位解释器（venv/系统直调、用户级
  site-packages 经 `py -<版本>`）；npm 非 prefix 的包显式 `--prefix`——不会卸错位置。
- **卸载残骸可见**：npm 卸载半途失败会在 prefix 根留下孤儿启动器
  （`X` / `X.cmd` / `X.ps1` 三件套及安全代理的 `.IPGSD` 副本）——死文件躺在
  PATH 目录上，敲命令即报错，且不属于任何包、不进五档判定。审计单独点名
  这类残骸（报告 ORPHAN SHIMS 节 + 警告），`clean` 脚本附带逐文件删除行；
  GUI 有「卸载残骸」操作条（两步确认逐文件删除 + 重审计核验清零，
  与脚本同一删除命令），也可用工具条「导出脚本」先生成脚本审阅；
  node 自管命令（npm/npx/corepack）与 `.bat` 脚本（用户自建转发脚本）永不触碰，
  有归属证据的启动器一律保留。

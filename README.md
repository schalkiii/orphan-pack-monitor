# orphan-audit

Windows 上的孤儿 CLI 工具审计器：找出你装了却长期不用的全局包，并给出可反驳的证据。
主路线 B（最近调用追踪），辅助路线 A（项目依赖静态审计接口预留）。

## 与竞品的差异

| 维度 | shelfware / dusty / dronagiri | UniGetUI / DevTool Manager | **orphan-audit** |
|---|---|---|---|
| 平台 | macOS / Linux | Windows（但无使用追踪） | **Windows 原生** |
| 核心信号 | shell 历史 + atime | 无 | **ETW 进程事件 + 历史 + atime + 配置 + alias** |
| 归因能力 | 只知道"跑过" | — | **知道谁跑的（父进程）、跑几次** |
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
  etw.rs         Windows ETW 执行事件（JSONL 落盘，可降级）
crates/cli/     CLI
gui/            GPUI 桌面界面（独立 workspace，避免拖累 CLI 编译）
spikes/         技术验证（GPUI Windows 编译 spike）
```

## 判定规则（顺序即正确性）

1. `active` — 交互命中 ≥ 自适应阈值（历史每千条计 1，钳制 3..50）
2. `indirect` — 近期执行过（含 ETW/atime 归因），即使不是用户敲的
3. `rare` — 历史出现过，但 `--since` 窗口内无执行
4. `integration` — 没跑过，但被 shell 配置引用
5. `unused` — 五路信号全静默，**唯一进入清理候选**

每个判定附带 evidence，可在 GUI 证据面板逐条反驳。

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

# GUI（仪表盘 + 包表格 + 证据面板 + 清理预览；支持筛选/排序/点击查看判定证据）
cd gui && cargo run
```

## ETW 与降级

- 有管理员权限时：可启用 Kernel-Process provider 追踪，记录命令行、父进程、时间戳。
- 无权限时：自动降级为 shell 历史 + atime，报告明确标注 `degraded`，不会静默给出弱证据结论。
- NTFS atime 自 Win10 1803 默认为 System Managed（启用，60 分钟粒度）；若被禁用会跳过该信号并警告。

## 实测数据（2026-11-18，本机）

- 采集：9 个全局包（cargo 4 / pip 3 / uv 1 / 其他 1）
- 历史源：PowerShell PSReadLine `ConsoleHost_history.txt`，6664 条
- 判定：1 active / 8 unused
- 交叉验证：`binstall`、`llvm-cov`、`headroom` 在历史中确实 0 次出现 → unused 判定正确；
  `numpy` 仅出现在 `pip install numpy` 安装命令中，解析器取首个命令词 `pip`，未把安装误算为使用 → 判定正确。
- clippy `-D warnings` 通过；27 个单元测试通过。

## 安全

- 工具**从不自动卸载**任何包。`clean` 只生成带证据注释的脚本，用户审阅后自行执行。
- 依赖包（`is_dependency`）永不进入清理候选。
- npm 的 bin 映射读 `package.json` 的 bin 字段（而非按包名猜测），低置信度在报告中标注。

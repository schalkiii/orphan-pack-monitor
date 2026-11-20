# orphan-audit 第五轮审计报告（多环境批次体检）

> 日期：2026-11-19。范围：全仓（root / gui / etw-capture 三 workspace）+ 文档 + 运行态。
> 背景：v0.2.0 发布后工作区累计**四个未提交批次**（GUI 美化、npm/winget 采集修复、
> IDE 直调误判修正 + 管理器标签、多环境检测 + clean PowerShell 化 + tooltip 定宽），
> 本轮是对这批新代码的全面体检。前序报告：根目录 `AUDIT-REPORT-2026-11-18.md`（一轮）、
> `doc/AUDIT-REPORT-2026-11-18.md`（二轮 A1-A14）、`-R3.md`（三轮 C1-C14/R1-R12）、
> `-R4.md`（四轮 D1-D8，发版前；D4-D8 为记录不修的接受项）。

## 一、现状描述

- 规模：约 7,025 行 Rust（core 4,335 + cli 313 + gui 2,009 含 build.rs + etw-capture 377），
  13 个源文件 + `gui/assets/generate_icon.py`（图标生成）+ `gui/build.rs`（winresource 嵌入）。
- 测试：**73 个单元测试全绿**（core 60 + cli 3 + gui 9 + etw-capture 1，2026-11-19 实测）；
  上一批次验证三 workspace clippy `-D warnings` 0 错、`cargo fmt --check` 干净。
- 运行态（2026-11-19 实测）：审计 41 包 = cargo 2 + npm 14 + pip 23 + uv 2；
  检测到 npm prefix ×2（默认 + QClaw npm-global）与 python ×4（Python313 主环境、
  CodeBuddy py311、uv 托管 3.12 ×2）；ETW 常驻采集中（tracker available）。
- 结构：本轮新代码沿用既有分层——多 prefix/多 python 枚举在 inventory 层（纯 fs + py -0p，
  不 spawn node）、卸载命令环境定位在 actions 层、GUI 仅展示（view 纯函数 + main 状态机）。
  分层未见破坏。
- 工作区：11 个文件修改 + 2 个新增（gui/assets/、gui/build.rs），+2,265/-530 行，未提交
  （用户尚未要求 commit）。

## 二、问题清单

| # | 位置 | 问题 | 严重度 | 影响 |
|---|------|------|--------|------|
| F1 | `crates/core/src/store.rs` `migrate`/`replace_inventory`/`save_snapshot` | **多环境重复包打破存储主键（实测复现）**：`packages` 表主键 `(name, manager)`，而多 prefix/多 python 检测后同名包可合法存在多份（实测 `@jackwener/opencli` 同时装在默认 prefix 与 QClaw prefix）。`replace_inventory` 第二条 INSERT 违反 UNIQUE → 整事务回滚 → **每次审计的持久化全部失败**（实测警告 `[store] inventory save failed: UNIQUE constraint failed`，库内停在 38 行旧数据 vs 实际 41 包）。`save_snapshot` 是 INSERT OR REPLACE，两份副本被静默合并成一份（漂移统计丢一份）。另 `unused_streak_months` 按行计数，重复包会使 streak 按行而非按月膨胀 | **高** | 功能错误：持久化回归、离线报告（report_from_store）与漂移快照基于陈旧包集；修复多环境批次引入，属必须修的回归 |
| F2 | `crates/core/src/lib.rs` `atime_candidate_paths` Pip 分支 | **用户级 site-packages 的 atime 候选路径拼错（实测复现）**：pip 入口目录推导固定取 site-packages 的**两层父目录**（正确于 venv/系统布局 `<root>\Lib\site-packages`），但用户级布局 `%APPDATA%\Python\Python313\site-packages` 的入口在**一层父目录** `Python313\Scripts`（实测该目录存在 dotenv.exe/httpx2.exe/f2py.exe 等大量入口；代码拼的 `%APPDATA%\Python\Scripts` 不存在）。后果：用户级 pip 包全部丢失 atime 证据（误判 unused 风险上升）；`fill_installed_at` 同路径推导 → 新装观察期对用户级 pip 包失效（**刚装即入清理候选**） | 中 | 误报风险 + 观察期保护缺口——与「绝不误判可删」的产品信条直接冲突 |
| F3 | `AGENTS.md` | **规则文件滞后四个批次**：测试数写 62（实际 73）、真机验证写 28 包/2026-11-18（实际 41 包/2026-11-19）、「当前状态」止步于 R3 批次（缺多环境/PowerShell/winget/管理器标签批次描述）、定位行缺 winget 便携包 | 中 | 下个会话的 Agent 以旧基线接手，重复排查或误报门禁数字 |
| F4 | `crates/cli/src/main.rs` `--json audit` | JSON 输出只序列化 `report.results`，`detected_envs` / `warnings` 仅文本模式可见，机器消费面缺多环境信息 | 低 | 记录不修（输出形状变更需用户确认消费方） |
| F5 | `CHANGELOG.md` `[未发布]` 头部摘要 | 摘要只覆盖前两批（GUI 美化 + npm 采集修复），缺直调误判、多环境、PowerShell 化、tooltip 批次（条目本体都在，仅头部导语过期） | 低 | 文档卫生 |
| F6 | `crates/core/src/inventory.rs` `exe_on_path` / `first_exe_on_path` | 两个函数近似重复（都查 PATH 上 `<name>.exe` 是否存在，后者返回首个完整路径），语义可复用 | 低 | 代码质量 |
| F7 | `crates/core/src/store.rs` `events()` | 时间戳解析失败静默回退 `Utc::now()`（预存行为，非本轮引入） | 低 | 记录不修（损坏行无从恢复时间，now 比丢弃整行保守） |
| F8 | `crates/core/src/inventory.rs` `is_npm_global_prefix` | shim 对指纹（同名 .cmd+.ps1 且根无 package.json）存在理论误报面：无 package.json 但带 node_modules 与成对 cmd/ps1 的应用目录会被当 npm prefix | 低 | 已文档化的启发式取舍（锁文件或 shim 对，双重标记 + 根 package.json 排除），实测误报为 0；记录接受 |

沿袭 R4 记录不修项（本轮复核仍然成立）：D4 GUI 状态机无单测、D5 etw-capture 父名缓存
无自动化测试、D6 无 release CI job、D7 事件匹配 O(包×事件)、D8 ManagerArg 双维护点。

## 三、改进建议

### E1（对应 F1，P1）存储层支持同名包多实例

- **做法**：
  1. `packages` 表新增 `install_key TEXT NOT NULL DEFAULT ''`（= `install_path` 的字符串
     形式，缺失时空串），主键改 `(name, manager, install_key)`；`verdicts` 表同加
     `install_key`（取 `VerdictResult.package.install_path`），主键改
     `(snapshot_id, name, manager, install_key)`；
  2. `migrate()` 对旧库做表重建迁移：检测 `PRAGMA table_info` 无 `install_key` 列 →
     建 new 表 → `INSERT ... SELECT ... IFNULL(install_path,'')`（verdicts 恒 `''`）→
     drop 旧表 → rename；新库由 CREATE TABLE IF NOT EXISTS 直接落新 schema；
  3. `unused_streak_months` 改 `GROUP BY snapshot_id` + `MIN(verdict)`——按**月**计数
     （不按行），且取字母序最小档（= 使用最强的档），即「该月所有副本均 unused 才计入
     streak」的保守语义；单副本数据行为不变（向后兼容）；
  4. 回归测试：双 prefix 同名包 `replace_inventory` 成功且 `packages()` 双行；快照保留
     双副本；streak 按月不按行（一份 active 副本即断档）；旧 schema 迁移保数据。
- **文件**：`crates/core/src/store.rs`（含测试）。
- **验证**：`cargo test -p orphan-audit-core` 全绿；实测 `audit` 无 `[store]` 警告、
  库内 41 行、opencli 2 行；`drift` 子命令正常列出快照。

### E2（对应 F2，P2）补用户级 site-packages 的入口目录候选

- **做法**：`atime_candidate_paths` Pip 分支在既有两层父目录（venv/系统布局）之外，
  增加**一层父目录**（用户级布局 `...\Python\Python313\Scripts`）的候选；不存在路径由
  `probe_atime` 静默跳过，多拼候选无害。`fill_installed_at` 自动受益。
- **文件**：`crates/core/src/lib.rs`（`atime_candidates_per_manager` 测试补用户级布局断言）。
- **验证**：`cargo test -p orphan-audit-core`；实测用户级 pip 包（PyMuPDF 等）出现
  installed_at / atime 证据（或确认其入口确无近期访问，两态皆可复核）。

### E3（对应 F3+F5，P2）文档同步

- **做法**：`AGENTS.md` 定位行补 winget 便携包、「当前状态」重写为四批次现状
  （73 测试、41 包、多环境检测、clean PowerShell、ETW 常驻）；`CHANGELOG.md`
  `[未发布]` 头部摘要覆盖全部四批。
- **文件**：`AGENTS.md`、`CHANGELOG.md`。
- **验证**：通读比对；测试数与 `cargo test` 输出一致。

### E4（对应 F6，P3）`exe_on_path` 复用 `first_exe_on_path`

- **做法**：`exe_on_path(p)` 改为 `first_exe_on_path(p).is_some()`（语义等价，删重复逻辑）。
- **文件**：`crates/core/src/inventory.rs`。
- **验证**：编译 + clippy + 既有测试（`command_for` / python 枚举路径均覆盖）。

## 四、优先级与并行性

| 优先级 | 条目 | 可并行性 | 依据 |
|---|------|----------|------|
| P1 | E1 存储多实例 | 可与 E2/E4 并行（仅动 store.rs） | 实测功能回归，每跑一次审计都在丢持久化 |
| P2 | E2 atime 候选 | 可与 E1 并行（仅动 lib.rs） | 误报风险 + 观察期缺口 |
| P2 | E3 文档同步 | 可并行（AGENTS/CHANGELOG） | 交接成本 |
| P3 | E4 去重 | 可并行（inventory.rs） | 代码质量 |

**执行顺序**：E1 → E2 → E4 → 全量门禁（三 workspace test/clippy/fmt）→ 实测复核
（audit 无 store 警告 + 库 41 行 + drift 正常）→ E3 文档定稿。F4/F7/F8 与 D4-D8 记录不修。

## 五、七维结论

① **文档一致性**——F3（AGENTS 滞后四批）、F5（CHANGELOG 导语）；README/DESIGN 与代码一致。
② **文档缺漏**——无新增缺失（多环境取舍已在 README/CHANGELOG 记录）。
③ **架构优化点**——无新问题；多环境枚举与卸载定位的分层正确，`detected_envs` 的
   二次枚举开销可忽略（注释已声明）。
④ **代码写法**——F1/F2 为多环境批次引入的缺陷（回归），F6 为重复；其余新代码
   （指纹扫描、三路 pip 定位、scoped prefix 推导）实现与注释质量良好。
⑤ **测试覆盖率**——F1/F2 场景缺回归用例（双实例持久化、用户级布局候选），修复时补齐；
   D4/D5 沿袭为接受缺口；core 新增解析函数（parse_py_list/指纹扫描/infra 过滤）均有针对性测试。
⑥ **测试严谨性**——未发现「跑通即通过」；断言均检查具体值，且保留格式同步哨兵
   （detail_cn 全格式断言）。本轮 F1 恰证明「真机实测」不可被单测替代（73 绿下仍有回归）。
⑦ **工程规范**——CI 三 workspace 含 fmt 全覆盖；版本一致 0.2.0；四个未提交批次属
   用户决策项（非缺陷，收尾阶段单独汇报）。

## 六、执行状态（2026-11-19 当日追加）

| 条目 | 状态 | 备注 |
|---|------|------|
| E1 存储多实例 | ✅ 已执行 | install_key 列（packages/verdicts）+ 表重建迁移 + streak 按月 GROUP BY + MIN(verdict) 保守语义；新增 3 测试（双实例持久化 / 快照保留+streak 按月 / 旧 schema 迁移保数据）。**端到端验证**：真实旧库副本上迁移执行、41 包（原卡 38）、opencli×2、QClaw 包×2、115,942 事件全量重写、零 store 警告 |
| E2 atime 候选 | ✅ 已执行 | Pip 分支补一层父目录候选（Python313\Scripts）；实测生效：websocket-client 获得 installed_at → 「新装（观察期）」标注，python-dotenv / pywin32 出现 atime 证据（68 天前）——修复前用户级 pip 包两者皆无 |
| E4 exe_on_path 去重 | ✅ 已执行 | 改为 `first_exe_on_path(p).is_some()` |
| E5（执行中新增）CLI `--db` 全子命令生效 | ✅ 已执行 | 审计发现 `--db` 原本只对 `drift` 生效（audit/status/clean 无视、写死默认路径），既是文档性误导也堵死了在受控路径上的端到端验证。三个子命令改走 `audit_with_paths(&config, &db)` |
| E3 文档同步 | ✅ 已执行 | AGENTS.md 定位/状态重写为五批次现状（76 测试/41 包/多环境/本批修复）；CHANGELOG 导语覆盖全部批次 |

全量门禁：**76 测试全绿**（core 63 + cli 3 + gui 9 + etw-capture 1）；三 workspace
clippy 0 错、fmt 干净。

### 环境异常记录（TSD 文件防护，非代码问题）

默认路径 `%APPDATA%\orphan-audit\orphan-audit.db` 的**agent 进程树写入会被 TSD
回滚**：修复后的 audit 在进程内全部成功（零警告），文件在运行中一度增长到 16.8MB，
进程退出后被还原回 13:48 状态（15,867,904 字节，与同目录 `orphan-audit.db.IPGSD`
字节数一致——TSD 于 13:48 生成的隔离副本）。取证过程：schema 与 computed_at 在
多次运行后不变、python 直写可持久、库副本路径一切正常。**结论**：代码在任意
可写路径行为正确（单测 + 真实旧库副本端到端双重证明）；默认路径的持久化受
安全代理干扰，属环境问题。用户从自己终端（受信进程树）运行 audit 时不受影响
——若发现库不更新，参照本节与 `tsd-workspace-encrypted-git-workaround` 记忆。
`.IPGSD` 文件按安全规则不读取、不删除、不绕过。

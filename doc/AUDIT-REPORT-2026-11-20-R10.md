# 审计报告 R10（2026-11-20，第十轮）

主题：**清理链路与残骸盲区**。用户连续第五轮要求「继续完善项目」。前四轮准确性
（R6-R9）火力集中在信号与判定层；本轮换焦点——审计**判定之后**的链路（clean 脚本、
卸载执行、残骸文件），并实施 AGENTS「后续」清单中记录最久的用户痛点（孤儿 shim，
⑧ 补充/⑨ 两批都有记录）。审计方式：全量通读 actions/lib/inventory/model/cli +
实机取证 npm prefix 布局。signals/verdict/etw/imports/store/gui 为 R7-R9 三轮
连续深审对象，本轮只做增量复核（无改动则结论沿用）。

## 一、现状描述

- 规模：~9800 行（core 9 模块 + cli 825 + gui 2933 + etw-capture），113 测试全绿
  （core 87 + cli 10 + gui 16 + etw-capture 6），三 workspace clippy/fmt 清零。
- 清理链路现状：判定 unused → `clean` 生成 PowerShell 脚本（BOM / 多环境定位 /
  观察期跳过 / 依赖排除）或 GUI 勾选卸载（两步确认 + 重审计核验）——**只覆盖
  「包」本身的卸载，不覆盖卸载半途失败留下的文件残骸**。
- 实机取证（本轮新增，两个 npm prefix 全量枚举）：
  - 默认 prefix `%APPDATA%\npm`：活包 bins = aiusage/lark-cli/mimo/trellis/tl/
    runta/codebuddy-stats/cbs/firecrawl/pnpm/pn/pnpx/pnx/tokentracker-cli/tracker/
    tokentracker/tokentracker-tracker；**孤儿 shim = `opencli`、`ocusage` 两组**
    （包目录已删，X/X.cmd/X.ps1 全留，另各带 3 个 .IPGSD 伴生副本）；
  - QClaw prefix `%APPDATA%\QClaw\npm-global`：活包 bins = agently-cli/wecom-cli；
    **孤儿 shim = `opencli` 一组**；`pip.bat`/`python.bat` 是用户自建转发脚本
    （非 npm 产物）；
  - 空 @scope 目录残留：默认 prefix `@jackwener`/`@geeeger`、QClaw `@jackwener`
    （包卸载后 scope 目录未清，内部可能有 TSD 点前缀隔离暂存）。

## 二、问题清单

| 编号 | 严重度 | 位置 | 问题 | 影响 |
|------|--------|------|------|------|
| H1 | 高 | core inventory.rs（无检测）+ lib.rs + actions.rs + cli/main.rs | **npm 卸载残骸 shim 完全不可见**：npm uninstall 半途死掉（本机根因：TSD 干扰，AGENTS ⑧ 补充已记录）会删掉 node_modules 里的包目录、留下 prefix 根的启动器三件套 `X`/`X.cmd`/`X.ps1`（及 .IPGSD 伴生副本）。审计只枚举 node_modules（npm_packages_under），残骸不进任何清单、不进任何警告；`clean` 脚本也不覆盖 | 死 shim 留在 PATH 目录：用户敲 `opencli` 撞死启动器报错（用户实测「卸载失败」的直接体感）；清理建议永远不覆盖这类残骸 = 清理链路盲区；实机现存 3 组 9 文件 + 6 个 .IPGSD 副本 |
| M1 | 中 | core actions.rs `generate_script` | clean 脚本无残骸清理段（与 H1 同一修复体，拆开登记便于验证） | 用户只能手动找残骸文件；多 prefix 下手动找既费劲又易漏 |
| — | 低（已排除，记录防复审） | — | 取证排除的候选：① 空 @scope 目录（3 个）——同类事故的另一种残留形态，但 scope 目录内可能藏 TSD 点前缀隔离暂存（`.opencli-XXXX`），自动列出删除候选有误伤风险，且空目录无害（不在 PATH 上、不拦截命令），本轮不做；② pip Scripts 残留 .exe——本机未观察到实例，无数据支撑阈值设计；③ GUI 加 shim 行——shim 不是包，塞进五档表格破坏模型；warnings 横幅（已存在）即可承载 | 无 |

### 七维度结论（增量）

1. **文档一致性**：R9 后 README/AGENTS 与实现同步；本轮落地后需补 README
   「安全」段（清理链路覆盖残骸）。
2. **文档缺漏**：无新缺漏。
3. **架构优化点**：无新问题。shim 检测归 inventory（npm 布局知识同源），
   呈现归 actions/lib，分层不变；检测结果不持久化（瞬态事实，每次审计重扫）。
4. **代码写法**：H1/M1 如上。actions.rs 的卸载命令矩阵（cargo/npm --prefix/
   pip 解释器定位/go 提示注释）与 inventory 的 npm 布局推导复核无新发现。
5. **测试覆盖率**：缺口随 H1——检测需六类反例（活包归属/孤儿/核心命令排除/
   .bat 不碰/半套残缺/加密 fallback 归属），脚本段需断言 Remove-Item 行。
6. **测试严谨性**：既有 113 测试断言充分。
7. **工程规范**：core + cli 改动，无部署红线（capture/hooks 不涉及）。

## 三、改进建议

| 优先级 | 建议 | 涉及文件 | 验证方式 |
|--------|------|----------|----------|
| P0 | **R1 孤儿 shim 检测 + 清理链路闭环**：`inventory::npm_orphan_shims()` 对每个 npm prefix 枚举根上启动器——stem 候选 = `X.cmd` 存在，或 `X.ps1` 且无扩展名伴生 `X` 存在（npm 三件套必含扩展名 sh 脚本；纯 .ps1 无伴生的可能是用户自己的脚本，不列）；排除 node 自管核心命令 {npm, npx, corepack}（nvm/fnm 版本 prefix 根有 npm/npx 启动器，其包目录形态随版本管理器而变，不值得逐布局适配）；.bat 一律不碰（QClaw 的 pip.bat/python.bat 非 npm 产物）。孤儿判定 = 该 prefix 的包集合（npm_packages_under）无人 `owns_binary(stem)` **且** `node_modules/<stem>` 目录不存在（双保险：目录名兜底 TSD 加密 fallback 包的归属——fallback 包名取目录名，owns_binary 含包名匹配，两条任一即归属）。文件清单 = {X, X.cmd, X.ps1, X.IPGSD, X.cmd.IPGSD, X.ps1.IPGSD} 中实际存在者——**只拼路径查存在性，绝不读 .IPGSD 内容**（TSD 红线）。模型 `model::OrphanShim { prefix, bin, files }`；`AuditReport.orphan_shims` 字段 + warning 一条；audit_with_paths 与 report_from_store 两路都扫（纯 fs，开销可忽略）。`generate_report` 加 "ORPHAN SHIMS" 节；`generate_script` 加残骸清理段（每文件 `Remove-Item -LiteralPath`，段头注释说明来源与审查义务）；CLI clean 接线 | model.rs + inventory.rs + lib.rs + actions.rs + cli/main.rs（generate_report/generate_script 签名加 `&[OrphanShim]`，既有调用方与测试同步） | 单测（检测六反例 + 报告节 + 脚本段）；**实机**：`audit` 恰好报 3 组（opencli×2、ocusage×1），活 shim（aiusage/pnpm/tl/tracker/firecrawl…）零误报，pip.bat/python.bat 不出现；`clean` 脚本含 3 组 Remove-Item 行 |
| P2 | **R3 文档同步**：README「安全」段补残骸清理语义；CHANGELOG Added/Fixed | README.md + CHANGELOG.md | 通读 |

### 执行序与并行性

R1 串行（一条文件链 model→inventory→lib→actions→cli），R3 收尾。每点即测。
core+cli 改动，**无需重装 capture/hooks**。

## 四、展望（不实施，记录）

- 空 @scope 目录残骸（@jackwener/@geeeger 等 3 个）：需先弄清 TSD 隔离暂存
  目录的完整生命周期再定安全清理规则。
- pip Scripts 残留 .exe：待本机出现实例后按数据定。
- GUI 对 orphan shims 的专门呈现（现走 warnings 横幅，够用）。

## 五、执行记录（2026-11-20，R1/R3 全部实施 + 实机验证）

- **R1** ✅：`model::OrphanShim { prefix, bin, files }`；`inventory::npm_orphan_shims`
  （薄封装遍历全部 prefix）+ `npm_orphan_shims_in`（单 prefix 检测，可测）；
  `AuditReport.orphan_shims` 字段，`audit_with_paths` 与 `report_from_store`
  两路都扫 + 同一条警告；`actions::generate_report` 加 ORPHAN SHIMS 节、
  `generate_script` 加残骸清理段（每文件一行；无包候选时也输出——残骸与
  包卸载是两条独立通道）；两个函数签名加 `&[OrphanShim]`，CLI 接线。
  PowerShell 删除行用单引号字面量形态（路径含 `$` 不展开）。
- **测试**：+4（inventory ×2：六反例检测 + 归属双保险；actions ×1：脚本/报告
  段与无候选路径；model 结构体随编译验证）。GUI / etw-capture 经 grep 确认
  零引用受影响 API（GUI 经 `report.warnings` 横幅自动呈现新警告）。
- **门禁**：三 workspace test + clippy + fmt 清零（etw-capture 另跑 release
  clippy）。core+cli 改动，capture/hooks 无需重装（部署红线不涉及）。
- **实机验证**：
  - `audit`（--db 临时路径防 TSD 回滚）：ORPHAN SHIMS 节**恰好 3 组**——
    ocusage（默认 prefix）、opencli（默认 prefix）、opencli（QClaw prefix），
    各 6 文件（三件套 + 3 个 .IPGSD 伴生），与第一节取证基线逐一对应；
    活启动器（aiusage/pnpm/tl/tracker/firecrawl/mimo/lark-cli/agently-cli/
    wecom-cli…）**零误报**；pip.bat/python.bat 未出现；警告行触发。
  - `clean`：3 组 18 行 `Remove-Item -LiteralPath '<path>'`（含全部 .IPGSD
    副本），段头注释齐全。
- **测试计数校正**：本批起全量口径 **122 测试（core 90 + cli 10 + gui 16 +
  etw-capture 6）**——此前「113」的口径漏计了 etw-capture 的 6 个
  （113 = core 87 + cli 10 + gui 16），本次一并纠正。
- **R3** ✅：README 安全段补「卸载残骸可见」；CHANGELOG Added 条目 + 概览行。

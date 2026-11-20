# 审计报告 R11（2026-11-20，第十一轮）

主题：**工程规范与文档新鲜度**。用户连续第六轮要求「继续完善项目」。前五轮
（R6-R10）火力覆盖信号/判定/采集/清理链路；本轮换焦到项目工程面——**CI 门禁
（D6，自首轮审计记录至今未做）**、**文档新鲜度**，并对**最久未深审的 store.rs**
（上次深审 R5/R6，五轮前）做全量复核 + 工作区卫生盘点。signals/verdict/etw/
imports/actions/inventory/lib/cli 在 R7-R10 连续四轮深审，本轮增量复核。

## 一、现状描述

- 规模：~9800 行，122 测试全绿（core 90 + cli 10 + gui 16 + etw-capture 6，
  R10 刚核过全量口径），三 workspace clippy/fmt 清零（etw-capture 含 release
  clippy）。
- 工程面现状：GitHub remote 存在（schalkiii/orphan-pack-monitor，远端 main
  与本地同步在 v0.2.0）；**CI 已存在**（`.github/workflows/ci.yml`，v0.2.0 提交，
  在 GitHub 实跑过一次：2026-11-18 push main 成功，6m42s）——但**落后本地门禁
  基线**（详见 M2）；v0.2.0 之后**十五个未提交批次**（纯本地积压），其中含
  **未跟踪的核心源码文件**（crates/core/src/imports.rs、gui/build.rs、
  gui/assets/、crates/cli/assets/）——不只是改动未提交，整段实现只存在于工作区。
- store.rs（698 行）本轮全量深审：多实例主键迁移、streak 按月 MIN 语义、
  事件回读容错、旧 schema 迁移测试全部有锚定——**无新问题**。
- doc/ 现存 9 份审计报告（2026-11-18 起每轮一份），为历史记录非残留垃圾；
  根目录无临时文件残留；.gitignore 覆盖 target/db/WAL。

## 二、问题清单

| 编号 | 严重度 | 位置 | 问题 | 影响 |
|------|--------|------|------|------|
| M1 | 中 | README.md「实测数据」节尾部 | **质量声明停在 2026-11-18**：`clippy -D warnings 与 cargo fmt --check …全部通过；97 个单元测试通过（core 73 + cli 5 + gui 14 + etw-capture 5）`——与现状（122，构成也全变）差 25 个测试；该行紧跟 clap/fmt 声明，读起来是现役质量门禁描述而非历史快照 | 新读者（或未来的自己）对项目质量状态形成过期认知；该节其余数字带「2026-11-18 实测」日期限定，唯独这行像现役声明 |
| M2 | 中 | .github/workflows/ci.yml | **CI 与本地门禁基线漂移 + 缺发布产物 job**（D6）：现有 CI 止于 v0.2.0 的门禁面——R8 之后本地基线新增「etw-capture **release 配置** clippy」（GENERIC_WRITE 事故的直接防线），CI 未同步，**R8 那个 bug 即使当时有 CI 也拦不住**（CI 从不编译 etw-capture 的 release cfg）；另缺 release 构建、tag 发布产物 job（D6 本义）、concurrency 取消；本次审计初稿误判「无 CI」，实为已有且实跑过一次——教训：**本地基线演进时必须路由到 CI 文件**（同步矩阵新增此路由） | 15 批积压 push 后，release-only 路径仍无 CI 防线；发版要手动本地构建搬运三份产物 |
| — | 低（已排除，记录防复审） | cli/main.rs `Commands::Status` | `status` 跑**完整审计**（全部采集器 + 信号 + 落库）只为打印五档计数——语义可辩（要新鲜数据），成本与 `audit` 相同属已知设计，记录不改 | 无 |
| — | 低（已排除，记录防复审） | inventory.rs / actions.rs | lib-only npm 包（无 bin 字段）会判 unused 并进清理候选——其「无信号」是**按构造空洞**（永远无命令可命中）而非「被反驳」。但本机 0 实例（11 个 npm 包全部有 bin），按证据驱动纪律（同 R10 对 pip 残留的处理）不实施保护，待真实实例出现再定 | 无 |
| — | 低（已排除，记录防复审） | store.rs `events()` | 时间戳解析失败回退 `Utc::now()`——坏时间戳变「现在」会骗过新鲜度窗口；仅手改库可触发（写入侧恒为 to_rfc3339），无现实路径 | 无 |
| — | 低（用户决策项） | 工作区 | 15 未提交批次含未跟踪源码文件：一次误操作（`git checkout .` / `git clean`）即**永久丢失**数周实现（含整段 imports.rs） | 需用户拍板提交节奏，本轮只强调 |

### 七维度结论（增量）

1. **文档一致性**：M1（README 质量行过期）；其余 README/AGENTS/CHANGELOG 与
   实现同步（R10 刚收尾）。
2. **文档缺漏**：无新缺漏。
3. **架构优化点**：无新问题（store.rs 深审通过：迁移幂等、streak 多实例
   MIN 语义、事件回读容错均有测试）。
4. **代码写法**：三条低危记录项（status 成本 / lib-only 候选 / 时间戳回退）；
   store.rs 及复核范围内的其余模块无新发现。
5. **测试覆盖率**：测试本身无新缺口；缺的是**执行面**——122 个测试只在
   本机跑（关联 M2）。
6. **测试严谨性**：既有断言充分。
7. **工程规范**：M2（CI 与本地基线漂移）；目录结构/gitignore/doc 组织规范，无残留垃圾。

## 三、改进建议

| 优先级 | 建议 | 涉及文件 | 验证方式 |
|--------|------|----------|----------|
| P1 | **R1 CI 对齐本地基线 + 发布产物 job**：更新既有 `.github/workflows/ci.yml`——保留现有三 workspace 门禁步骤，补齐：① etw-capture `cargo clippy --release -- -D warnings` + `cargo build --release`（R8 教训：release-only 路径要编到）；② `concurrency` 取消同分支旧跑（tag 不取消）；③ tag（`v*`）触发的 release-artifacts job：三 workspace release 构建 + `upload-artifact` 上传三份 exe（**不自动建 GitHub Release**——发布节奏由维护者拍板）；④ ubuntu advisory check job（core/cli 的 cfg(not(windows)) 分支从未验证过编译——本地交叉实验卡在 libsqlite3-sys 的 C 交叉工具链，代码层结论未知；只 check/clippy 不跑测试，`continue-on-error: true` 不阻塞，连续绿后转正） | .github/workflows/ci.yml（更新） | ① YAML 本地解析通过；② 每条命令与本地门禁逐字同构（本地已全绿 122 + etw release 门禁）；③ 端到端首跑 pending——需 push 后在 Actions 页确认，报告如实标注 |
| P2 | **R2 README 质量行刷新**：实测数据节尾部改为带复核日期的现役声明（2026-11-20：122 = core 90 + cli 10 + gui 16 + etw-capture 6），不再让 2026-11-18 的 97 冒充现状 | README.md | 通读 |
| P3 | **R3 AGENTS 后续清单同步**：D6 标记已完成；低危记录项（status 成本 / lib-only 候选）入「后续」清单防重审 | AGENTS.md | 通读 |

### 执行序与并行性

R1 → R2 → R3 串行（R3 记录 R1 结果）。R1 不动源码——测试基线不受影响，
但按纪律仍逐点跑门禁确认无意外。

## 四、展望（不实施，记录）

- `status` 子命令的快路径（改走 store 报告模式）——语义变化需用户拍板。
- lib-only npm 包的清理候选保护——待本机出现真实实例。
- linux target 的 `cargo check` CI job——本地交叉实验结果见执行记录。

## 五、执行记录（2026-11-20，R1/R2/R3 全部实施）

- **审计自纠**：本报告初判「无任何 CI」是**错的**——`.github/workflows/ci.yml`
  已随 v0.2.0 提交，且在 GitHub 实跑过一次（2026-11-18 push main，success，
  6m42s，`gh run list` 核实）。Write 工具的「读前拦截」阻止了对既有文件的
  盲目覆盖，真实问题由「无 CI」修正为「CI 与本地门禁基线漂移」。教训：
  **本地基线演进时必须路由到 CI 文件**（R8 给本地加 etw release clippy 时
  漏了这一步——CI 防不住 R8 那类 release-only bug）。已写进 AGENTS 门禁行。
- **R1** ✅：ci.yml 保留既有三 workspace 门禁步骤，补齐四项——① etw-capture
  `clippy --release` + `build --release`（带 R8 事故注释）；② `concurrency`
  （同分支取消旧跑、tag 不取消）；③ tag（`v*`）触发的 release-artifacts job
  （三 workspace release 构建 + `upload-artifact` 上传三份 exe，
  `if-no-files-found: error`；不自动建 GitHub Release）；④ ubuntu
  advisory check job（`continue-on-error: true`，只 check/clippy 不跑测试，
  转正条件写在注释里）。
- **linux 交叉实验结论**：本地 `cargo check --target x86_64-unknown-linux-gnu`
  失败于 libsqlite3-sys 的 C 交叉工具链（`x86_64-linux-gnu-gcc` 缺失）——
  **代码层结论未知**，这正是 ubuntu advisory job 存在的理由。附带教训：
  `cargo check ... | tail -5` 的管道退出码是 tail 的（exit 0 掩盖了失败），
  取证命令勿接管道看退出码。
- **本地验证**：YAML 解析通过（3 jobs）；tag job 三条构建路径本地全绿——
  root release（5.38s）、gui release（首次本地验证，后台跑完 exit 0）、
  etw-capture release（R8 已验）。**CI 端到端首跑 pending**：需 push 后在
  Actions 页确认（本批不 push——提交节奏由用户拍板）。
- **R2** ✅：README 实测数据节质量行刷新为带日期现役声明（2026-11-20：
  122 = core 90 + cli 10 + gui 16 + etw-capture 6）+ CI 提及一行。
- **R3** ✅：AGENTS 后续清单去掉已完成项（D6、孤儿 shim 清理——后者 R10
  已做但清单漏删，本批顺带修正），补 R11 记录项（status 快路径 / lib-only
  保护 / unix job 转正条件）。
- **门禁**：全量复跑绿——122 测试（root 100 + gui 16 + etw 6）、三 workspace
  fmt/clippy 清零（本轮零源码改动，属确认性复跑）。
- **store.rs 深审结论**：无新问题；三条低危项（status 成本 / lib-only 候选 /
  events() 时间戳回退 now()）记录于问题清单，不实施。

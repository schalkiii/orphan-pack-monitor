# 审计报告 R8（2026-11-20，第八轮）

主题：**etw-capture 去控制台窗口（用户直接诉求：Rust 原生、不额外开 console）** +
准确性增量复核。当日已连做 R6/R7 两轮，代码其余部分同日刚审计过，本轮聚焦
etw-capture 与交叉复核，不再重复全量扫描。

## 一、现状描述

- 规模：9730 行 + 当日热修，110 测试全绿（core 79 + cli 10 + gui 16 + etw 5），
  三 workspace 门禁清零。
- etw-capture（851 行）：控制台子系统二进制，`install` 以 schtasks ONLOGON/HIGHEST
  注册计划任务，**每次登录弹一个控制台窗口**（DESIGN.md 第 61 行记录的已知取舍，
  install 输出第 166 行也明示）。守护侧诊断（启动、轮转、4688 自愈、追加失败）
  全走 println/eprintln——计划任务拉起时无人在看，交互运行时才有意义。
- 依赖：ferrisetw 1.2（transitively windows-sys 0.48 / 0.61.2 均在树内），
  未直接依赖 windows-sys。

## 二、问题清单

| 编号 | 严重度 | 位置 | 问题 | 影响 |
|------|--------|------|------|------|
| H1 | 高（用户直接诉求） | etw-capture/src/main.rs（crate 根）+ DESIGN.md:61 | **每次登录弹控制台窗口**：控制台子系统 exe 被计划任务交互式拉起。守护形态根本不需要控制台；install/status/deep 的交互输出另有途径 | 用户每次登录被闪一个黑窗（外部 agent 也已注意到）；观感 + 对常驻进程的信任成本 |
| H2 | 中 | main.rs 守护路径全部 eprintln/println | **守护侧诊断无处落地**：计划任务拉起时无控制台，启动/轮转/4688 自愈重写（R4 特性）/追加失败全部静默丢失；无窗口化（H1 修复）后连"没人看"的控制台都没有了 | 采集端异常（如标志被抹后自愈了多少次）完全不可观测 |
| M1 | 中 | main.rs `poll_4688_loop` → `etw::append_etw_event`；core etw.rs:94-105 | **4688 事件时间戳失真**：事件自带 SystemTime 已解析进 `Sec4688.ts` 却被丢弃，落盘用 `Utc::now()`——轮询 5s 周期 + 2 分钟重叠窗 + 停机续传场景下，事件时间可偏晚最多数分钟；时间戳直接进判定窗口过滤与 last_seen 展示 | 窗口边界事件的档位判定可被推迟的 ts 推出窗外；last_seen 展示失真 |
| L1 | 低 | main.rs `query_4688_xml` `/c:500` | 单轮查询上限 500 条且 `/rd:true`（最新优先）+ record_id 断点取 max——突发 >500 条解释器进程启动时，旧于断点的溢出事件永久丢失 | 极端突发下漏采（常态负载远低于此） |
| L2 | 低 | core verdict.rs `is_interactive_shell` | 名单缺 `openconsole.exe`（新版 Windows Terminal 的宿主）等 | 极小概率把用户交互归为间接——归因档位不受影响（indirect 与「可能是用户」同档），实际无判定影响 |

### 七维度结论（增量，当日 R7 已全量覆盖）

1. **文档一致性**：DESIGN.md:61 的「控制台取舍」在 H1 修复后过时——随修复同步重写。
2. **文档缺漏**：H2 的诊断日志（capture.log）无文档说明——随修复补。
3. **架构优化点**：未发现新问题（run_hidden/会话管理/断点续传结构清晰）。
4. **代码写法**：H1/H2/M1/L1 如上；`package_from_watched_path` 已确认覆盖 uv/pipx
   工具 venv（`...\uv\tools\<t>\Lib\site-packages\` 含 site-packages 段），R7 遗留
   疑问解除。
5. **测试覆盖率**：缺口随问题——M1 需要 ts 解析与 `append_etw_event_at` 单测；
   H1/H2 的 AttachConsole/诊断路径无法单测（系统集成），以实机验证代替。
6. **测试严谨性**：既有 5 测试断言充分，无「跑通即通过」。
7. **工程规范**：无新违规；注意 **部署红线**——修完必须 `cargo build --release`
   再 install（release 与 debug 行为在本轮刻意分化，见 R1）。

## 三、改进建议

| 优先级 | 建议 | 涉及文件 | 验证方式 |
|--------|------|----------|----------|
| P0 | **R1 release GUI 子系统**：crate 根加 `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`（debug 保持控制台子系统，cargo run 开发体验不变）；`main()` 首行 `attach_console_if_available()`——`AttachConsole(ATTACH_PARENT_PROCESS)` 成功后 CreateFileW CONOUT$ + SetStdHandle(stdout/stderr)，失败（无父控制台，如计划任务拉起）静默跳过：管道场景（cargo test / bash）无需挂接、句柄本就有效；需新增直接依赖 `windows-sys`（0.61，features: Win32_System_Console + Win32_Storage_FileSystem + Win32_Security）；install 输出删去 console 提示行 | etw-capture/src/main.rs + Cargo.toml | `cargo test`（管道下输出正常）；release 构建后 `schtasks /Run` 拉起 → status 显示 running（GUI 子系统无控制台句柄不 panic 即证明 std 空句柄兜底生效）；`cmd /c etw-capture.exe status` 终端可见输出（AttachConsole 生效）；deep 前台运行可见输出。**无窗口的视觉确认由用户下次登录完成** |
| P1 | **R2 守护诊断落盘**：新增 `log_diag(msg)`——追加时间戳行到 `<数据目录>/etw/capture.log`；守护路径（会话启动/停止、轮转、4688 自愈重写、追加失败、事件线程退出）改走 log_diag（追加失败注意罕见性，不额外限流）；交互子命令输出保持 println | etw-capture/src/main.rs | 实机：install 后 capture.log 出现启动行；手工删 4688 标志 → ≤600s 内出现 re-asserted 行 |
| P2 | **R3 4688 真实时间戳**：core 新增 `etw::append_etw_event_at(path, binary, parent, cmd, ts: DateTime<Utc>)`（原函数变薄壳传 now）；capture 侧 `Sec4688.ts` 解析（chrono rfc3339，解析失败回退 now）传入 | core/src/etw.rs + etw-capture/src/main.rs + 双侧测试 | 单测：固定 ts 落盘后 read 回读一致；解析失败回退不 panic。实机：新事件 ts 与 Security 日志原始 SystemTime 对齐 |
| P3 | **R4 查询上限放宽**：`/c:500` → `/c:2000`，注释说明突发溢出语义（rd:true 最新优先 + 断点取 max 的丢失条件） | etw-capture/src/main.rs | 既有 parse 测试不回归；实机正常轮询 |
| P3 | **R5 文档同步**：DESIGN.md 61 行取舍段重写为 GUI 子系统行为说明（交互输出经 AttachConsole、守护诊断在 capture.log）；README 如提及（无，确认即可） | etw-capture/DESIGN.md | 文档通读 |

### 执行序与并行性

R1 → R2 → R4 串行（同文件 main.rs）；R3 独立可并行（core + capture 两侧）；
R5 收尾。每点即测（software-improvement 纪律）；**全部完成后 `cargo build
--release` + install 重部署**（红线），实机验证四项。

## 四、展望（不实施，记录）

- L2 `is_interactive_shell` 扩名单——无判定影响，不修。
- Prefetch / Amcache 兜底（需 LZXPRESS/格式解析，投入产出比待估）与长驻进程
  import 盲区（R7 已记录）继续留档。

## 五、执行记录（2026-11-20，5/5 全部实施 + 部署）

- **R1** ✅：crate 根 GUI 子系统能力项 + `attach_console_if_available()`；新增直接
  依赖 windows-sys 0.61（Foundation/Security/Storage_FileSystem/System_Console）。
  **实施中实测修正两处（审计时未预见）**：
  ① `GENERIC_WRITE` 在 windows-sys 0.61 归属 `Win32::Foundation` 而非
  `Storage::FileSystem`——该函数被 cfg 排除在 debug 构建外，首次 release 编译才
  炸（E0432），「部署红线：release 构建后才装」再次自证价值；
  ② **无条件 SetStdHandle 会抢走父进程布置的管道**——本机 git-bash 自带隐藏
  控制台，首次 install 的全部输出被吞进 CONOUT$（管道端零字节）。修正为
  **只补空缺句柄**（GetStdHandle 返回空/INVALID 才 SetStdHandle），管道与显式
  重定向场景输出照原路走；修正后二次 install 输出完整回流管道。
- **R2** ✅：`log_diag`/`diag` 落 `<数据目录>/etw/capture.log`，守护路径全量改造
  （会话启停 / 轮转 / 4688 自愈 / 追加失败 / 事件线程退出 / install 的 console
  提示行删除）。**补一处审计未列的卫生问题**：`rotate_log_if_huge` 单测经
  log_diag 污染生产 capture.log（实机日志混入 3 条 oa_rot 测试行）——新增
  `OA_CAPTURE_LOG` 环境变量重定向（先例 OA_IMPORT_LOG），测试并断言诊断行落盘。
- **R3** ✅：core `append_etw_event_at`（原函数薄壳化为 now）+ capture
  `parse_4688_ts`（rfc3339 解析，失败回退 now 不丢事件）；core/capture 各 +1
  测试。**实机实证**：python 03:51:19.319 启动 → 事件 ts=19.383（真实
  SystemTime），而写入它的轮询 24.137 才执行（日志中相邻的 wevtutil 自身事件
  可证）——旧代码会落 24.1，4.75 秒偏晚消除。
- **R4** ✅：`/c:2000` + 突发溢出语义注释（DESIGN.md 采集段同步）。
- **R5** ✅：DESIGN.md「已知取舍」重写为「窗口形态」说明（GUI 子系统 + 空缺句柄
  补齐语义 + capture.log）。**审计错判一处**：README 实际也提及控制台取舍
  （第 120 行，审计称「无提及」），ETW 小节同步改写。
- **门禁与部署**：112 测试全绿（core 80 + cli 10 + gui 16 + etw-capture 6，+2）；
  三 workspace clippy 清零（etw-capture 额外跑 **release 配置** clippy——cfg
  分支只有 release 编译）；release 构建（PE subsystem=2 实证 GUI）+ install
  重部署，升级停机路径实机走通。
- **实机验证**：cargo test 管道输出正常；计划任务拉起常驻（pid 36328，GUI
  子系统无控制台句柄不 panic，事件持续落盘）；status/install 输出经管道完整
  可见；capture.log 完整生命周期（started → 外部停止 + stopped → started）；
  R3 时间戳如上实证。re-assert 诊断行**实机强制验证**：手工删除 Lsa 落点的
  ProcessCreationIncludeCmdLine_Enabled（03:57:52）→ 下一 600s 检查周期
  （04:00:15）发现缺失、重写双落点并在 capture.log 落 "4688 cmdline flag
  was missing; re-asserted"，注册表恢复 1。
  **无窗口的视觉确认留给用户下次登录。**

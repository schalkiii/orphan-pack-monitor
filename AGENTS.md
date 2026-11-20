# AGENTS.md — orphan-audit

## 定位
Windows 孤儿包审计器：找出装了却长期不用的全局包（cargo/npm/pip/uv/pipx/winget 便携包），
五档判定（active/indirect/rare/integration/unused）+ 可反驳证据；清理只能由用户显式触发
（CLI clean 生成脚本审阅后自跑，或 GUI 勾选 + 两步确认后执行），**绝无静默删除**。

## 怎么跑
- 测试基线：`cargo test --workspace`；GUI 另建：`cd gui && cargo test`；采集端另建：`cd etw-capture && cargo test`
- 质量门禁：`cargo clippy --workspace --all-targets -- -D warnings`（gui/、etw-capture/ 目录内同样跑
  clippy）；CI（`.github/workflows/ci.yml`）与本地门禁逐字同构——**本地基线演进时必须同步 CI
  文件**（R8 加 etw release clippy 时漏同步，R11 审计发现 CI 防不住 R8 那类 bug）
- 格式：`cargo fmt`（workspace 与 gui/、etw-capture/ 各跑一次；spikes/ 冻结不动）
- CLI：`cargo run -p orphan-audit -- audit`（子命令 audit/status/clean/drift，全局 --json/--since/--ignore/--manager）
- GUI：`cd gui && cargo run`（独立 workspace，避免 CLI 编译拖入 GPUI）
- ETW 采集：`cd etw-capture && cargo run`（**管理员**；订阅 Kernel-Process 写 events.jsonl，Ctrl+C 停；设计见 etw-capture/DESIGN.md）

## 技术栈
Rust 1.85+（实测 1.99.1）；rusqlite(bundled) 存储；clap 4；GUI 用 gpui-pre 0.3.12
（package 重命名 gpui，平台入口 `gpui_platform::application()`）；Windows 优先（ETW/atime/PSReadLine）。

## 目录与约定
- `crates/core` 引擎（model/inventory/signals/verdict/store/actions/etw），无 GUI 依赖；
  `crates/cli` CLI；`gui/`、`etw-capture/` 各为独立 workspace；`spikes/` 是 GPUI 可行性证据（exclude，勿改）。
- 判定规则顺序即正确性：active→indirect→rare→integration→unused，重排必产生误报（见 verdict.rs 模块注释）。
- 两条安全增强：新装观察期（installed_at 在 since 窗口内的包不进清理候选）、
  长期未用证据（内部名 drift：连续 ≥3 个月快照均判 unused → 证据链追加该行，档位不变）。
- 安全红线：绝不静默/自动卸载——GUI 卸载必须用户勾选 + 两步确认（选择变化即复位
  确认态）；依赖包（is_dependency）与 ignore 列表永不进清理候选。
- 改动纪律：最小改动、逐点验证（每改一点跑全测）、注释同步；提交前 lint 清零。
- ETW 现状：`audit` 只消费 `events.jsonl`（`read_etw_log_since` 只取 `--since` 窗口内
  事件）；采集端 `etw-capture/`（ferrisetw UserTrace，订阅 Kernel-Process）支持**常驻**
  （`install` 子命令注册 ONLOGON 计划任务，本机已部署；任务设置已加固为电池容忍 +
  无时限，日志 64MB 轮转）与前台手动两种形态；运行中重复 `install` 即升级；
  `uninstall`/`status` 管理任务。`audit` 本身不开启追踪——文档/文案不得宣称
  「提权即开启追踪」。

## 当前状态与下一步
- **133 测试全绿**（core 101 + cli 3 + gui 23 + etw-capture 6；此前「113」口径
  漏计 etw-capture 的 6 个，R10 起改全量口径）、三 workspace clippy/fmt 清零
  （etw-capture 另跑 release 配置 clippy——GUI 子系统/AttachConsole 代码只在 release 编译）。
- v0.2.0（tag 9f8ec21）之后累计**二十六个批次**（已随 v0.3.0 提交发布）：
  ① GUI 美化（exe 图标、「？」悬停说明体系、仪表盘点击筛选、斑马表格）；
  ② 采集修复（npm Windows 静默为空 → 纯文件系统采集；winget 便携包采集器；
  .IPGSD 兜底；command_for 的 .cmd 回退）；
  ③ IDE 直调误判修正（npm 包 bin/ 脚本 atime 探测）+ 管理器标签行 + 说明去面板化；
  ④ 多环境检测（npm 多 prefix / python 多环境）+ clean 转 PowerShell + tooltip 定宽；
  ⑤ 第五轮审计修复（见 `doc/AUDIT-REPORT-2026-11-19-R5.md`）：store 同名包多实例
  （install_key 主键 + 旧库自动迁移）、user-site pip atime 候选、CLI --db 全子命令生效；
  ⑥ GUI 多环境可视化：「环境」行（npm/python 分组标签，悬停看完整路径，单环境不显示）+
  状态行精简（来源明细移入悬停说明）；`detected_envs` 结构化为 `DetectedEnv`/`EnvKind`
  （CLI 文本输出格式不变）；
  ⑦ GUI 去 AI 味 + 环境归属：仪表盘 6 卡片 → 五档判定分布条 + 计数图例（占比即数据）、
  中性灰阶替换蓝味深底（蓝只留交互态与 npm 语义点）、中点/破折号分隔改逗号、管理器列
  扁平化；`view::package_env_label` + `build_rows` 门控——同名同管理器跨多环境才在
  包名后标注归属短名（唯一实例安静），证据面板新增等宽安装路径行；
  ⑧ GUI 勾选卸载 + 界面去噪：行首勾选框（`Row.removable` = core 给得出卸载命令）+
  表头全选可见行 + 底部操作条两步确认 → 后台逐个跑 `powershell -NoProfile -Command
  <uninstall_command>`（CREATE_NO_WINDOW）→ 自动重审计核验（node 输出/退出码在
  TSD 下不可信，以清单消失为准，未消失点名）；预览清理脚本整体移除（CLI clean 保留）、
  状态行成功态不再常驻、空态只留信号标签行。**GUI 冒烟期间未卸载任何包**
  （CLI audit 复核 41 包完好）。
  ⑧ 补充（同日第二轮）：**卸载结论只按重审计清单消失与否给出**——用户实测删
  opencli 报「失败」，实际 npm 已把两个 prefix 的包目录都删了，只是退出码非零
  （TSD 吞输出 + shim 删除受扰，opencli.cmd/.ps1 及 .IPGSD 隔离副本残留成孤儿）；
  修复 = 退出码降级为过程信息，结果消息分三档（已移除 N 个 / 仍在清单：…请到
  终端手动处理 / 已移除 X 个，仍在清单：…），同名多环境包在提示里去重；
  界面文案不再提「五路」（未用档说明改「窗口内所有信号均无记录」，证据空态改
  「没有查到任何使用记录」，core/README/CHANGELOG 注释同步）。
  ⑨ 加载态转圈 + 已卸载包复活修复：用户追问「npm 退出码非零，重新加载为什么
  还能看到 opencli」——根因是**安全代理隔离被删文件时在 node_modules（含 @scope
  内层）留下点前缀暂存目录**（.opencli-XXXX，内容 .IPGSD、package.json 仍可读），
  采集器误当安装中的包收录；npm_packages_under 现跳过顶层与 @scope 内层的
  点前缀条目（+1 回归测试）。加载态：表格体中央 8 点追逐环（gpui Animation 逐点
  不透明度；本版 gpui 无 Div rotate），「正在审计……」占位文案与状态行提示删除，
  出错才出文字。**实测 36 包 = npm 11 + pip 23 + uv 2（cargo 0）**：除 opencli ×2
  为用户经 GUI 卸载外，@geeeger/ocusage 与 cargo 的 rg/eza 也在工具之外被卸
  （.crates2.json 已消失，磁盘核实非采集 bug）；ocusage 的 .cmd/.ps1 shim 残留
  成孤儿（与 opencli shim 同类，清理 shim 仍是后续改进项）。
  ⑩ 准确性四连（用户追问「包多在项目里 import，shell 历史参考性不大，有没有更准
  的办法」后拍板四项全做）：**① import 钩子采集端**——core 新增 `SignalSource::Import`
  （is_execution、窗口过滤同 ETW）；`orphan-audit hooks install` 为每个 python 解释器
  纯菌写 `.pth` 观察钩子（meta_path find_spec 恒 None、每进程每模块去重、吞一切异常）
  + 用户级 `NODE_OPTIONS --require` node 钩子（registerHooks.resolve 只记裸说明符 +
  Module._load 兜底按路径归名）；事件写 imports/events.jsonl，审计经 pip top_level.txt
  （小写键契约）映射回包名；`hooks status/uninstall` 对称。**② 4688 命令行并入**——
  capture 启用「进程创建」审计（子类别必须用 GUID `{0CCE922B-…3030}`，中文系统不认
  英文名）+ ProcessCreationIncludeCmdLine_Enabled；常驻线程 5s 轮询 Security 日志
  （直启 wevtutil `/uni:true`——cmd /c 包装会被 Rust 的 `\"` 转义坑成 rc=87，
  /uni 输出 UTF-16LE 带 BOM 解码），EventRecordID 断点续传（.4688.state），只写
  node/python 解释器事件（其余 Kernel-Process 已覆盖，避免双计）；命令行经
  install_path / `python -m` 归因。**③ Kernel-File 深度模式**——`etw-capture.exe deep`
  opt-in spike：FILE_IO_INIT 只取 Create（opcode 64，路径属性实测叫 **OpenPath**
  非 FileName）+ 每包每进程去重 + 非包段过滤（.dist-info/.egg-info/__pycache__/.pth/
  .IPGSD），实测 11s 6500 行 → 24 行；deep 用独立会话名（run_deep 曾误清常驻
  TRACE_NAME，已修）。**④ PSReadLine 伪时间戳**——无时间戳历史按 (mtime-created)/
  行数线性插值每行使用时间（EXTENDED_HISTORY 真时间戳仍优先）。
  **部署状态：hooks 已装 4 解释器 + NODE_OPTIONS（重开终端生效）；4688 链路已启用
  且实机验证（node/python 事件带命令行落盘）；capture 新 exe 已 install。**
  ⑪ 第六轮审计修复（`$project-audit` 产出 `doc/AUDIT-REPORT-2026-11-20-R6.md`，
  11 建议，R1-R11 当日全实施 + 部署）：**R1** `owns_binary` 大小写不敏感（Windows
  命令名不区分大小写，`RG`/`rg` 不同判 = 丢全部使用证据误判可删，最危险方向）；
  **R2/R3** GUI 卸载链路三处状态安全——核验按「包名+install_path」配对（同名多环境
  不再误报全残留）、重审计成功清空勾选/确认态（旧行索引失效）、`audit_generation`
  代数丢弃交错回调；**R4** 4688 标志自愈——采集循环每 600s 核对双注册表位置
  （`Policies\System\Audit` + 旧版 `Control\Lsa`）缺即重写（环境实测数分钟内抹掉
  标志）；**R5** uv/pipx 工具 venv 钩子——install 直播 .pth 到
  `%APPDATA%\uv\tools`/`%LOCALAPPDATA%\pipx\venvs`（`UV_TOOL_DIR`/`PIPX_HOME`
  生效），status 增 `tool venvs hooked`，core `module_to_package_map` 对 uv/pipx
  扫 `Lib\site-packages`；**R6** GUI detail_cn 补 `imported Nx` → 「导入 N 次
  （import 钩子）」；**R7** NODE_OPTIONS 成对摘除（`--require`+标记路径一起走，
  保第三方 --require）+ 陈旧路径刷新（标记与当前路径同时满足才跳过）；
  **R8** `parse_since` `checked_mul` 防溢出回绕；**R9** clean 脚本 UTF-8 BOM
  （PowerShell 5.x 中文乱码）；**R10** hooks uninstall 诚实输出 + status 单遍扫描；
  **R11** help/注释补注（drift 只对比两档、default_db 回退隐患、--json 覆盖范围）。
  第四节展望三项（atime 扫描器折扣已在本批 ⑫ 实施 / UserAssist·Prefetch /
  长驻进程 import 留作候选）。部署实测：capture 重装运行中、
  hooks 4/4 解释器 + 2/2 tool venv（harbor/headroom-ai）、`import fitz` 事件
  经 OA_IMPORT_LOG 重定向验证落盘。
  ⑫ 第七轮审计修复（同日第二轮，`doc/AUDIT-REPORT-2026-11-20-R7.md`，聚焦准确性、
  实机取证驱动）：**R1 atime 扫盘折扣**——实机发现 11 个包的 atime 精确聚在同一日
  （69 天前）且无其他信号（安全软件扫盘形态），`--since 90d` 或下次扫盘落入窗口时
  会全部误判 indirect；`signals::discount_swept_atimes` 按「同 UTC 日期 ≥5 个不同
  命令的 atime」判扫盘日、整体剔除并打警告，audit 与 report_from_store 两路都生效；
  实机验证 `--since 90d` 下 11/11 保持 unused（21 条事件剔除、警告触发）。
  **R2 归因边界**——pip 的 install_path 是共享 site-packages 根（含该路径的命令行
  会打中全部 23 个 pip 包，潜伏未爆发）、`node_modules\lodash` 是 `lodash-cli` 的
  无边界子串；修复 = pip 跳过路径归因（`-m 模块` 映射承担）+ `contains_at_boundary`
  边界匹配。**R3** ETW 最新事件超窗口 1/4 且 ≥2 天时警告采集可能中断（「ETW 开启」
  不再掩盖盲区）。**R4** import 日志超 16MiB 提示手动清理（并发 append 下不做自动
  轮转）。**R5** README 补 atime 折扣/归因边界语义、verdict 规则 1 注释记录
  「全量历史不过滤」的设计权衡（M2 不改动）。本轮只动 core（signals/imports/
  lib/verdict）+ README，capture/hooks 无需重装。
  ⑫ 补充（同日热修，外部反馈 P0）：**hooks install 写 NODE_OPTIONS 的反斜杠路径
  让一切新 node 进程启动即崩**——node 解析 NODE_OPTIONS 用类 POSIX 引号规则，`\`
  被当转义符吃掉，--require 找不到文件（实测反斜杠 rc=1 / 正斜杠 rc=0；昨晚装的
  钩子让别的 agent 的 webdav 计划任务崩了 16 轮）。修复 = `merge_node_options`
  一律写正斜杠形态 + 旧反斜杠形态下次 install 自动刷新（自愈）+ strip 两形态
  通吃；本机注册表已修并实测 node 带钩子 rc=0。**纠正外部误判**：etw-capture
  （pid 9068）不写 NODE_OPTIONS、无周期回写——对方「修了 20 分钟被改回」是我们
  10:26 为部署 R6 修复重跑的那次 `hooks install` 落进了他们的修复窗口；capture
  周期重写的只有 4688 的 ProcessCreationIncludeCmdLine_Enabled（R4 自愈，另一个
  注册表值，与本故障无关）。
  ⑬ 第八轮审计修复（同日第三轮，`doc/AUDIT-REPORT-2026-11-20-R8.md`，用户直接诉求
  「Rust 原生、不额外开 console」）：**R1 无窗口常驻**——release 构建切 **GUI 子系统**
  （crate 根 cfg_attr；debug 保持控制台子系统，cargo run 开发体验不变），计划任务/
  登录拉起不再弹控制台窗口；`attach_console_if_available` 挂父控制台但**只补空缺的
  std 句柄**——无条件 SetStdHandle 会抢走父进程布置的管道（本机 git-bash 隐藏控制台
  + 管道，首次 install 输出被全吞进 CONOUT$，实测修正；GetStdHandle 非空不抢）。
  **R2 守护诊断落盘** `<数据目录>/etw/capture.log`（会话启停/轮转/4688 自愈/追加
  失败/事件线程退出，每行时间戳；`OA_CAPTURE_LOG` 可重定向——顺带修掉 rotate 单测
  污染生产日志的卫生问题）。**R3 4688 真实时间戳**——core 新增
  `append_etw_event_at`，落**事件自带 SystemTime** 而非轮询时刻（解析失败回退
  now 不丢事件；实机实证偏晚 4.75s→64ms——python 03:51:19.319 启动、事件 ts
  19.383、写它的轮询 24.137 才跑）。实施中另修：`GENERIC_WRITE` 在 windows-sys
  0.61 属 `Win32::Foundation`——该代码 cfg 排除在 debug 外、首次 release 编译才
  炸，部署红线（release 构建后才装）自证。**R4** wevtutil `/c:500`→`/c:2000`
  （突发溢出语义注释）。**R5** DESIGN.md「已知取舍」重写为「窗口形态」+ README
  ETW 小节同步（README 实有 console 提及，审计误判为无）。**部署：release 重装**
  （PE subsystem=2 实证，pid 36328），升级停机路径实机走通，capture.log 完整
  生命周期可见；无窗口视觉确认留给用户下次登录。
  ⑭ 第九轮审计修复（同日第四轮，`doc/AUDIT-REPORT-2026-11-20-R9.md`，实机取证
  驱动：14.8MB events.jsonl 全量聚类 + atime 分布分析）：**R1 探测爆发折扣**——
  实锤某 agent 用 GNU timeout 6 秒内逐个冒烟 9 个命令（06:09:24-29），8 个包被
  误判 indirect（executed 1x via timeout.exe）；`discount_probe_bursts`：同一
  **非交互**父进程 + 同一日历分钟 ≥5 个不同命令 + 命令无其他证据（保护）→ 整簇
  剔除 + 警告；`is_interactive_shell` 提升 pub(crate) 复用。**实施中发现并修复
  互为保护缺陷**：探测执行触碰的 atime 与爆发 ETW 同源（执行 06:09:24、NTFS
  system-managed atime 惰性刷新 06:16:23.4268221），atime 保护爆发、爆发保护
  atime，首版仅 harbor 翻 unused——修复 = atime 不作爆发保护 + 爆发折扣**先行** +
  扫盘折扣只认存活 ETW（回归测试固化）。**R2 atime 折扣 v2**：分钟层（同一分钟
  ≥4 命令，实机 11-05/11-06 两个同秒 4 命令簇曾漏剔）+ 日期层 ≥5（慢扫盘）+
  保护规则（有历史/import/ETW 证据的命令 atime 保留）。**R3** README 安全增强段
  重写 + CHANGELOG。实机验证（--since 90d）：8 假 indirect 全部回 unused、证据
  清零（pip+uv 非未用 9→1）；防误伤：headroom-ai（ETW 80×）/pnpm（6×）/
  tokentracker（import 9×）全保留——重复使用不成爆发簇。core-only，capture/hooks
  无需重装。**教训**：跨信号折扣的保护规则必须考虑「同一事件的多信号投影」
  （执行=ETW+atime 同源）——否则互为保护；NTFS system-managed atime 是惰性
  批量刷新（执行与 atime 可差数分钟，句柄视图与目录枚举视图读数还可能不一致）。
  ⑮ 第十轮审计修复（同日第五轮，`doc/AUDIT-REPORT-2026-11-20-R10.md`，聚焦
  清理链路盲区——前四轮火力都在信号/判定层）：**npm 卸载残骸（孤儿启动器）
  检测**——实机取证两个 prefix 共 3 组残骸（opencli ×2、ocusage ×1，各 6 文件
  = 三件套 + .IPGSD 伴生），npm uninstall 被 TSD 干扰半途死掉后留在 PATH 目录
  上，此前对审计完全不可见、clean 也不覆盖（⑧ 补充/⑨ 记录的用户痛点闭环）。
  `inventory::npm_orphan_shims(_in)`：stem 候选 = X.cmd 存在或 X.ps1+无扩展名
  伴生；排除 node 自管 {npm,npx,corepack}；.bat 一律不碰（QClaw 的
  pip.bat/python.bat 是用户自建转发脚本）；孤儿 = 本 prefix 无包 owns_binary
  且 node_modules/<stem> 目录不存在（加密 fallback 的归属兜底）；文件清单含
  .IPGSD 副本但**只查存在性绝不读内容**（TSD 红线）。呈现 = 报告 ORPHAN
  SHIMS 节 + `[npm] N orphan shim set(s)` 警告（GUI 走既有 warnings 横幅）+
  clean 脚本逐文件 `Remove-Item -LiteralPath '...'`（单引号字面量，路径含 $
  不展开；无包候选时也输出——残骸与包卸载是两条独立通道）。实机验证：恰好
  3 组零误报（aiusage/pnpm/tl/tracker 等活启动器全保留）。**测试计数纠正**：
  122（core 90 + cli 10 + gui 16 + etw 6）——此前「113」漏计 etw-capture。
  core+cli 改动，capture/hooks 无需重装。取证排除项（勿重审）：空 @scope
  目录残留（@jackwener/@geeeger，内可能藏 TSD 隔离暂存，删除有风险）；
  pip Scripts 残留 .exe（本机无实例）；GUI shim 专行（warnings 横幅够用）。
  ⑯ 第十一轮审计修复（同日第六轮，`doc/AUDIT-REPORT-2026-11-20-R11.md`，
  焦点：工程规范与文档新鲜度 + store.rs 深审〔五轮未审，结论：无新问题〕）：
  **CI 对齐本地门禁基线**——审计初判「无 CI」是错的（v0.2.0 已提交且实跑过
  一次，`gh run list` 核实；Write 工具读前拦截避免了盲目覆盖既有文件），真实
  问题是**基线漂移**：R8 给本地加的 etw release clippy 从未进 CI，R8 那个
  GENERIC_WRITE release-only bug 即使有 CI 也拦不住。ci.yml 补齐：etw
  release clippy+build、concurrency（同分支取消/tag 不取消）、tag（v*）
  release-artifacts job（三 exe 上传 artifact，不自动建 GitHub Release）、
  ubuntu advisory check（`continue-on-error`，Unix 分支从未验证过编译——本地
  交叉实验卡在 libsqlite3-sys 的 C 工具链，代码层结论未知；连续绿后转正）。
  README 质量行刷新（97→122 带复核日期）+ CI 提及；AGENTS 后续清单去重
  （D6/孤儿 shim 已做未删）。**教训**：本地门禁演进必须同步路由到 CI 文件
  （已入「怎么跑」）；`cargo check | tail` 的退出码是 tail 的——取证命令勿
  接管道看退出码。本轮零源码改动（122 复跑确认），CI 端到端首跑 pending
  （push 后 Actions 页核）。
  ⑰ 第十二轮审计修复（同日第七轮，`doc/AUDIT-REPORT-2026-11-20-R12.md`，
  准确性第五轮：实机取证驱动）：**import 日志整文件隔离的静默降级**——本机
  实锤 imports/events.jsonl 被安全代理整体隔离成 .IPGSD，且呈 **churn 态**
  （钩子宿主 append 时明文短暂存在——审计恰在其后跑能读到；随即整文件转
  密文，影子持续增长 1.00→1.18MB——审计恰逢其死则 import 证据为零），而
  `read_import_events_since` 对缺失文件返回 Ok(空) 零警告：靠 import 撑住
  indirect 的包（实测 tokentracker-cli import 9×）会无声滑向 unused——危险
  方向。修复 = `imports::hooks_installed()` + `import_log_warning(log,
  hooks_installed)`（.IPGSD 影子点名隔离成因 + 自愈说明；无影子 = 新装钩子
  早期态；未装钩子不警告），lib.rs 读取前接线；README import 段补隔离场景 +
  OA_IMPORT_LOG 逃生舱。实机验证：警告精确触发；tokentracker 在日志死态下由
  atime 兜底仍 indirect（**未发生误判可删的漂移**）。取证顺带：R9 展望
  「多分钟探测爆发」未复发（无数据不定阈值，继续观察）；全量审计实测 21.4s
  （36 包 / 16.9MB 事件 / 4 python 环境——大头是子进程探测等待，可接受不
  优化）。测试 122→123（core 90→91）。core-only，capture/hooks 无需重装。
  ⑱ 第十三轮审计修复（同日第八轮，`doc/AUDIT-REPORT-2026-11-20-R13.md`，
  焦点轮换：verdict.rs 判定引擎 + etw.rs 消费侧深审〔两文件早期轮后未单独
  深审，结论：判定逻辑本体无新问题〕+ 实机数据新鲜度〔capture 存活，最新
  事件 = 取证时刻〕）：**R1 零 ETW 事件提示的提权盲区**——警告条件原为
  `if degraded`，而 tracker_from_log_events 在提权环境 0 事件也报 available
  （probe_status 只查提权不查采集），提权 + 采集端未装 = 「ETW: available」
  + 0 事件 + 零警告；改 `if etw_events == 0`（NEW ⊇ OLD 超集证明，差集恰为
  盲区）。**R2** etw.rs is_elevated 陈旧注释（fsutil→net session）。**验证
  技法沉淀**：`APPDATA=<tmp>` 重定向对 `dirs::data_dir()` 无效（Windows
  Known Folder API 不读环境变量）——db 用 --db、import 用 OA_IMPORT_LOG、
  ETW 日志无逃生舱；`parse_since` 钳制 ≥1d（`.max(1)`）+ 常驻采集存活 ⇒
  真机造不出零事件窗口，提权分支靠超集证明收尾。记录项（勿重审）：atime
  窗口外仍出证据行（带日期，判定不受影响——设计一致）；read_etw_log_since
  ts 解析失败回退 now()（与 store.events 同类，失败方向安全）；.IPGSD 影子
  增长对 import 16MiB 检查不可见（TSD 的文件，红线不动）；report_from_store
  离线路径 tracker=probe_status + degraded 恒 true（离线语义）。core-only。
  ⑲ 第十四轮审计修复（同日第九轮，`doc/AUDIT-REPORT-2026-11-20-R14.md`，
  收尾轮：CLI 输出层深审〔main.rs 829 行，v0.2.0 后未单独深审〕+ 全仓一致性
  扫尾〔无 TODO 残留 / 版本一致 / CI 基线无漂移〕）：**R1** `--json audit`
  的 warnings 改经 stderr 输出（stdout 纯 results 形状不变——R12/R13 的
  显式降级至此覆盖机器消费路径；实机：stdout `[` 开头 823 行纯 JSON、
  stderr 恰 4 条 warning 含隔离警告）；**R2** hooks status 对被隔离 import
  日志显示 quarantined（此前误报 "not created yet"——排障命令在最需要说
  真话的场景说假话；core 新增 `pub imports::shadow_path` 与警告共用拼法，
  只查存在性绝不读内容，+1 单测）；**R3** --json help 纠偏（drift 实无
  json 输出，R6 补注写错——修文本不实现 feature，无消费方不造无主功能）。
  测试 123→124（core 91→92）。core+cli，capture/hooks 无需重装。
  ⑳ 第十五轮审计修复（同日第十轮 = **终轮**，`doc/AUDIT-REPORT-2026-11-20-R15.md`，
  GUI 专项——R6 后经 ⑦⑧⑨ 三大批次未再深审的最后一个面）：唯一 actionable =
  「间接」档两处悬停说明漏 import 钩子（批⑩ 的第三路执行证据，已补）；
  其余全绿——状态机 R6 加固点（代次守卫/索引失效/确认复位/失败清理）逐一
  复核在位，view.rs 14 测试语义覆盖，悬停文案与 core 逐条一致。记录项：
  警告横幅条目为 core 英文原文（翻译层需用户拍板）；GUI 默认 db 路径 agent
  树回滚（R5 既有记录）。**终轮宣告**：全仓每模块均经 ≥1 轮专项深审
  （inventory/signals R7·R9·R10 / verdict·etw R13 / store R11 / actions R10 /
  imports R12 / lib R12·R13 / CLI R14 / GUI R15 / etw-capture R6·R8 / 文档·CI
  R11），继续轮次 = 重复已审面，循环收束；恢复条件（爆发复发/环境变化/
  新误判反馈/新功能落地）见报告第六节。gui-only 两行改动。
  ㉑ GUI 警告横幅中文化（用户反馈跟进——用户看截图问「orphan shim 是什么意思」
  并要求改进描述，R15 记录的待拍板项就此拍板）：`view::warning_cn` 按已知
  前缀把 core 警告翻译为自解释中文（npm 残骸/etw 探测爆发/atime 扫盘/imports
  隔离与超限/零 ETW 事件/store 失败），路径、计数、错误详情等载荷保留，
  未知措辞整条透传（detail_cn 同款策略）。npm 残骸措辞刻意用「卸载残骸」
  替代 "orphan shim"——用户反馈该术语与「找孤儿包」主航线混淆（残骸是
  已卸载包留下的启动器死文件，独立于五档判定的清理通道）。+2 测试
  （gui 16→18），capture/hooks 无需重装。
  ㉒ GUI/CLI 能力对齐（用户指令「GUI 加残骸删除入口；所有 CLI 的能力 GUI 都要
  有」）：**残骸删除操作条**（警告横幅下方，组数/文件数/命令名一览，两步确认
  逐文件删除 + 重审计核验清零——与勾选卸载同一哲学，删除命令与 clean 脚本
  逐文件行同源）；**导出脚本**（工具条按钮，generate_script + write_script_file
  写桌面）；**长期对比面板**（右栏切换，快照芯片选旧/新、新旧不得同选，
  store::drift_diff 同源）；**钩子管理面板**（hooks 自 CLI 整体迁入 core 成共享
  模块——HooksStatus 结构化状态 + import 日志三分支中文渲染，安装单击、卸载
  两步确认、动作后重新探测）。共享实现全部下沉 core（actions::shim_removal_
  commands/default_script_export_path/write_script_file、store::drift_diff、
  core::hooks、lib::default_db_path），CLI 改薄壳调用、输出逐字不变；测试
  126→130（core 92→101 / cli 10→3 / gui 18→20），assets 随 hooks 迁至
  crates/core/assets。capture/hooks 无需重装。
  ㉓ 用户反馈跟进（证据链梳理 + 路径折行）：① signal_pills 按来源分组——
  Shell 足迹（历史/配置/别名/还原命令）/ 执行观测（ETW/import/访问时间）/
  长期趋势（月度快照），辅助信号文字调暗；signal_help 三段式（原理/判定/局限）
  梳理为每信号一句话（空态总览与证据条目「？」共用）。② view::path_display
  在反斜杠后插零宽空格——gpui 断点跟 UAX #14、`\` 属字母类不断行，长路径曾在
  `:`/`@` 后碎断（"C:" 独占一行）；证据面板安装路径与环境行悬停路径同批修复。
  +1 测试（gui 21）。gui-only。
  ㉔ 包表格「环境」列（用户指令「单环境也显示那一个」）：归属环境从同名多环境
  才后缀标注改独立一列逐行显示（npm prefix 短名 / pip 解释器短名，cargo/go/
  winget 显「—」），build_rows 撤 env_sets 门控、包名后缀标注移除；列宽重排
  （包名 230 / 判定 110 / 最近 90 / 大小 80 / 环境 76）；列不参与排序。
  gui-only，测试数不变。
  ㉕ 审计警告归入信号总览（用户指令「审计警告放到证据链的对应项下面」+
  「合理换行」）：顶部警告横幅取消，warnings_banner 删除；signal_pills 改
  signal_overview(warnings, etw_on)——按 warning_signal([tag]→SignalSource)
  把警告卡片归入对应信号组下方（etw/signal→Etw、imports→Import、atime→Atime、
  npm/store→「其他」），ETW 指引挂执行观测组；总览常驻右栏底部（evidence_
  column 尾部分隔线下，选中与否都显示），右栏 .id+overflow_y_scroll 可滚动；
  分组换行修复 = 组名单独一行、芯片自成一行。+1 测试（gui 22）。gui-only。
  ㉖ 第十六轮审计修复（[$project-audit]，R15「新功能落地」恢复条件触发，见
  `doc/AUDIT-REPORT-2026-11-20-R16.md`，聚焦批㉒~㉕ 新增面）：**hooks 库代码
  4 处 `eprintln!` 在 GUI 子系统会 panic**（window 无 stderr 句柄，写失败 panic →
  hooks_busy 永久卡死；R16-H1）——失败明细收集进 Vec<String>，以 `[hooks]` 行
  追加在 install/uninstall 返回摘要尾部（CLI 诊断 stderr→stdout 尾部，人读场景
  无脚本依赖）；GUI 反馈按行拆 div 渲染 + 成功回调并入 `[hooks]` 行
  （hooks_feedback_text 助手，R16-L2，+1 测试 gui 23）；README「警告横幅下」
  过时措辞刷新（R16-M1）。`grep eprintln crates/core/src` 归零；实机 hooks
  uninstall/install 往返复核输出格式与现场恢复（4/4 解释器 + 2/2 venv + node）。
- 实测（2026-11-19 晚）：36 包 = npm 11 + pip 23 + uv 2（cargo 0——rg/eza 与
  ocusage、opencli ×2 均已卸，见 ⑨）；npm prefix ×2（默认 + QClaw npm-global）、
  python ×4；ETW 常驻采集中（events.jsonl 持续增长）。
- 环境注意：TSD 会吞 agent 进程树下 node 的 stdout（采集永不 spawn node，纯 fs 读）；
  `%APPDATA%\orphan-audit\orphan-audit.db` 的 **agent 写入可能被 TSD 回滚**（隔离为
  .IPGSD），验证持久化时用 `--db` 指到临时路径（取证见 R5 报告第六节）；
  **imports/events.jsonl 同理**——agent 树进程的 import 事件会被回滚；R12 实锤更重
  的形态：**整文件被隔离成 .IPGSD 且 churn**（宿主 append 时明文短暂存在、随即整体
  转密文）——审计已显式警告（import_log_warning），钩子与 deep 模式都支持
  `OA_IMPORT_LOG` 重定向（%TEMP% 验证）；用户自启的终端里钩子写入不受影响。
  **ProcessCreationIncludeCmdLine_Enabled 注册表标志会被环境在数分钟内回写删除**
  （审计策略本身不被动）——4688 事件命令行变空时先查这个值（re-add 即恢复）；
  **python/sed/git 写的工作区源码文件会被 TSD 落盘加密**（rustc 读到密文编译失败，
  python 读回的"正常"是透明解密假象）——改源码一律用 ZCode Edit/Write 工具；
  万一已加密：`git add <file>` 快照 → `git show :<file> > <file>` 重写 → `git reset`；
  绝不能 `cat 密文文件 > tmp && mv`（毁掉解密元数据，明文永久丢失，2026-11-19 事故）。
  该行为的 GUI 症状：审计警告「[store] inventory save failed: UNIQUE constraint
  failed: packages.name, packages.manager, packages.install_key」——TSD 在两轮审计
  之间还原库文件（旧 schema 还魂）所致，**非代码 bug**；agent 启动的实例必现或偶现，
  用户自启进程是否复现待验证（若复现请反馈）。
- 后续：GUI 状态机 / etw-capture 交互测试（R4/R5 记录不修项）、deep 模式若长期实用
  再考虑去 spike 化（降噪 / 开销评估）、status 子命令快路径（现跑完整审计只为打印
  计数，改 store 报告模式是语义变化需拍板，R11 记录）、lib-only npm 包的清理候选
  保护（无 bin 字段的包按构造永远无信号，本机 0 实例待真实案例，R11 记录）、
  unix advisory CI job 连续绿后去掉 continue-on-error 转正。

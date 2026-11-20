# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [未发布]

## [0.3.0] - 2026-11-20

> GUI 美化批次：审美 / 条理 / 易用性整体提升 + exe 图标 + 「？」悬停说明体系；
> 清单采集修复（npm Windows 静默为空）+ winget 便携包采集器；
> IDE 直调误判修正 + 管理器标签行（UniGetUI 式分类）；
> 多环境检测（npm 多 prefix / python 多环境）+ 清理脚本 PowerShell 化；
> 第五轮审计修复（存储多实例 / atime 候选 / --db 一致性，
> 见 `doc/AUDIT-REPORT-2026-11-19-R5.md`）；
> GUI 检测环境行 + 状态行精简（detected_envs 结构化）；
> GUI 去 AI 味重构（五档判定分布条 + 中性灰阶）+ pip/npm 环境归属标注；
> GUI 勾选卸载（两步确认直接执行）+ 界面去噪（预览条 / 状态行 / 空态说明移除）；
> 已卸载包「死而复生」修复（安全代理隔离暂存目录被误采为包）+ 加载态转圈动画；
> 准确性四连：import 钩子采集端 + 4688 命令行并入 + Kernel-File 深度模式 + 历史伪时间戳；
> 第六轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R6.md`）：命令名大小写、卸载链路
> 三处状态安全、4688 标志自愈、uv/pipx 工具 venv 钩子覆盖、NODE_OPTIONS 成对摘除等；
> 第七轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R7.md`）：atime 扫盘同日聚类折扣、
> 命令行归因边界、ETW 采集新鲜度警告、import 日志超限提示；
> 同日热修：NODE_OPTIONS 钩子路径改正斜杠（反斜杠转义致一切新 node 进程崩溃）；
> 第八轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R8.md`）：etw-capture 无窗口化
> （GUI 子系统 + 守护诊断落盘 + 4688 真实时间戳）；
> 第九轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R9.md`）：探测爆发折扣 +
> atime 扫盘折扣两层化（分钟级聚类 + 保护规则）；
> 第十轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R10.md`）：npm 卸载残骸
> （孤儿启动器）检测与清理链路闭环；
> 第十一轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R11.md`）：CI 对齐本地
> 门禁基线 + tag 发布产物 job；
> 第十二轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R12.md`）：import 日志
> 整文件隔离的显式降级警告；
> 第十三轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R13.md`）：零 ETW 事件
> 提示的提权盲区修正；
> 第十四轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R14.md`）：CLI 输出层
> 三处诚实性修复（json 警告分流 stderr / hooks status 隔离感知 / help 纠偏）；
> 第十五轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R15.md`，终轮）：GUI
> 「间接」档说明补 import 钩子——全仓至此每个模块均经专项深审，审计循环收束；
> 用户反馈跟进：GUI 警告横幅中文化（R15 记录的待拍板项落地）；
> 批㉒ GUI/CLI 能力对齐：残骸删除入口 + 导出脚本 + 长期对比 + 钩子管理
> （共享实现全部下沉 core，GUI 与 CLI 同源）；
> 批㉓ 用户反馈跟进：证据链信号按来源分组 + 每信号一句话说明、
> 安装路径按目录边界折行；
> 批㉔ 用户反馈跟进：包表格新增「环境」列（单环境也显示归属）；
> 批㉕ 用户反馈跟进：审计警告按 tag 归入信号总览对应组下方（顶部横幅取消）、
> 信号总览常驻右栏底部且分组换行不串组；
> 批㉖ 第十六轮审计修复（见 `doc/AUDIT-REPORT-2026-11-20-R16.md`，R15 收束后
> 「新功能落地」恢复条件触发，聚焦批㉒~㉕ 新增面）：hooks 失败诊断随返回值走
> （GUI 子系统 eprintln panic 隐患）+ GUI 反馈多行渲染 + README 过时措辞。

### Added

- **hooks 失败诊断随返回值走（批 ㉖，R16 审计）**：批㉒ 把 hooks 迁入 core 时
  4 处 `eprintln!` 原样带入——CLI 上下文无害，但 GUI（release 为 window 子系统，
  无 stderr 句柄）里点「安装/卸载钩子」且任一解释器 .pth 写失败（Program Files
  场景）时，写 stderr 失败 panic → 后台线程死、`hooks_busy` 永久卡「处理中…」。
  现失败明细收集进 `Vec<String>`，以 `[hooks]` 行**追加在返回摘要文本尾部**
  （CLI 诊断从 stderr 移到 stdout 尾部，人读场景无脚本依赖；库代码不再碰进程流）。
  GUI 侧：反馈按 `\n` 拆行渲染（gpui 纯文本不渲染换行），安装/卸载成功回调把
  返回文本中的 `[hooks]` 行并入反馈（「钩子已安装（部分失败）」+ 逐行明细，
  `hooks_feedback_text` 助手）。+1 测试（gui 22→23）。附带（R16 审计 M1）：
  README「GUI 警告横幅下有卸载残骸操作条」的过时定位描述已随批㉕横幅取消刷新。

- **审计警告归入信号总览（批 ㉕，用户反馈跟进）**：顶部全宽警告横幅取消，
  警告按 core `[tag]` 前缀归入右栏「信号与诊断」总览的**对应信号组下方**
  （`etw`/`signal` → 执行观测，`imports` → 执行观测 import 项，`atime` →
  执行观测，`npm` 残骸与 `store` 库故障归「其他」——残骸另有操作条）；
  ETW 未开启的行动指引挂执行观测组。总览**常驻右栏底部**（选中包与否都
  可见，原空态限定），右栏可纵向滚动。**分组换行修复**：组名单独一行、
  组内芯片自成一行——窄栏下不再把「别名」和「执行观测」混到同一行。
  `warning_signal` + `warning_card` 助手，+1 测试（gui 21→22）。

- **包表格「环境」列（批 ㉔，用户反馈跟进）**：归属环境（npm prefix 短名 /
  pip 解释器环境短名）从「同名多环境才在包名后缀标注」改为**独立一列、逐行
  显示**——单环境也显示那一个（用户指令），无环境语义的管理器（cargo / go /
  winget）显示「—」；`build_rows` 撤销 env_sets 门控，包名列的后缀标注随之
  移除（有专列不再重复）。列宽重排：包名 270→230、判定 120→110、最近运行
  100→90、大小 90→80。列不参与排序（短名只是展示标注，完整路径仍是证据面板
  的归属铁证）。

- **GUI 证据链梳理 + 路径折行修复（批 ㉓，用户反馈跟进）**：① 信号总览按来源
  分组——**shell 足迹**（历史 / 配置 / 别名 / 还原命令，shell 派生信号不再散落
  「主 / 辅」两处）、**执行观测**（ETW / import / 访问时间）、**长期趋势**
  （月度快照）；辅助信号（不直接定档）文字调暗；每路信号说明从「原理 / 判定 /
  局限」三段梳理为**一句话**（空态总览与每条证据的「？」共用同一数据）。
  ② 安装路径折行修复：gpui 断点跟 UAX #14 走、`\` 属字母类不产生断点，长路径
  在 `:` / `@` 后碎断（实测 "C:" 独占一行）——`view::path_display` 在反斜杠后
  插零宽空格（U+200B），折行落在目录边界、行尾保留 `\`；环境行悬停路径同批
  修复。+1 测试（gui 20→21）。

- **GUI/CLI 能力对齐（批 ㉒）**：用户指令「GUI 加残骸删除入口；所有 CLI 的能力
  GUI 都要有」。缺口盘点后补齐四项，共享实现全部下沉 core（GUI 与 CLI 调同一
  函数，不各写一份）：
  - **残骸删除入口**：警告横幅下方新增「卸载残骸」操作条（组数 / 文件数 /
    命令名一览），两步确认后逐个运行 core `shim_removal_commands`（与 clean
    脚本逐文件删除行同源）；完成自动重审计，残骸清零才报成功（与勾选卸载
    同一「以重审计为准」哲学，退出码在安全代理环境下不可信）。
  - **导出脚本**：工具条按钮，`generate_script` + `write_script_file`（UTF-8
    BOM）写桌面，与 CLI `clean --output` 同一生成器与写盘函数。
  - **长期对比**：右栏新面板，月度快照芯片选旧 / 新（新旧不得同选、同侧再点
    取消）→ `store::drift_diff`（CLI drift 子命令同一实现）渲染两档增删。
  - **钩子管理**：右栏新面板，hooks 自 CLI 迁入 core 成共享模块（`HooksStatus`
    结构化状态 + import 日志三分支中文渲染）；安装单次点击、卸载两步确认，
    动作后重新探测、以最新状态为准。
  CLI 侧同步重构走 core API（hooks / drift_diff / write_script_file /
  default_db_path，输出逐字不变）；测试 126→130（core 92→101：hooks 迁入 6 +
  actions 2 + drift_diff 1；cli 10→3；gui 18→20）。

- **GUI 警告横幅中文化**（用户反馈跟进，R15 待拍板项落地）：审计警告在 GUI
  横幅按已知前缀翻译为自解释中文——如「[npm] 4 orphan shim set(s)」→
  「检测到 4 组卸载残骸——包已卸载，但启动器文件还留在 PATH 目录，敲命令会
  报错；clean 脚本附带逐文件删除行」（措辞用「卸载残骸」替代 "orphan shim"
  术语，避免与「找孤儿包」主航线混淆）；ETW 探测爆发 / atime 扫盘折扣 /
  import 隔离 / 零 ETW 事件 / store 失败等全部已知警告同批覆盖，路径、计数、
  错误详情等载荷原样保留；未知措辞整条透传（core 演进不阻塞显示，detail_cn
  同款策略）。`view::warning_cn` + 2 测试。

- **GUI 档位说明补 import 钩子**（R15 审计 P3，终轮）：「间接」档的两处悬停
  说明（五档总览 + 图例单项）原写「被执行过（ETW / 访问时间）」，漏了批⑩
  加入的第三路执行证据 import 钩子——GUI 自述原则「文案与 core 判定实现一一
  对应」就此补齐。终轮宣告见报告第六节：全仓每个模块均经至少一轮专项深审，
  继续轮次边际收益为零；恢复条件（爆发复发 / 环境变化 / 新误判反馈 / 新功能
  落地）已记录。

- **CLI 输出层诚实性三连**（R14 审计 R1-R3，收尾轮）：① `--json audit` 的
  warnings 改经 **stderr** 输出——stdout 保持纯 results（文档化形状，jq 可
  解析），机器消费方此前完全看不到 R12/R13 刚建立的显式降级警告（隔离 /
  零 ETW 事件），显式降级原则至此覆盖人与机器两条消费路径；② `hooks status`
  对被安全代理隔离的 import 日志显示 `quarantined by security software:
  <影子路径>`——此前一律报 "not created yet"，在用户排查 import 证据死亡的
  场景里恰好说假话（core 新增 `imports::shadow_path`，与审计警告共用拼法，
  仍只查存在性绝不读内容）；③ `--json` help 纠偏：drift 实际无 json 输出
  （R6 补注写错），改为「audit/status 生效」并说明 warnings 的 stderr 通道。

- **零 ETW 事件提示的提权盲区修正**（R13 审计 R1）：窗口内零 ETW 事件的
  提示原以 `degraded`（环境不可用）为条件——提权环境下 tracker 会报
  available（探测只查提权不查采集），提权跑 audit 且采集端未装的用户会拿到
  「ETW: available」+ 0 事件 + 零警告，核心差异化信号静默缺失且无安装指引。
  现改为窗口内 0 事件即提示（不看环境），与 R12 的 import 显式降级对称：
  证据源死亡必须显式。附带记录（见报告）：atime 窗口外证据行为、ETW 日志
  ts 解析回退、.IPGSD 影子增长对体积检查不可见——均判定为设计一致或环境
  约束，不改。

- **import 日志隔离显式降级警告**（R12 审计 R1，实机取证驱动）：钩子在装而
  `imports/events.jsonl` 缺失时，读取侧静默返回空集——import 证据归零而无任何
  感知，靠它撑住 indirect 的包会无声滑向 unused（危险方向的静默失真）。本机
  实锤成因：安全代理把**整个日志**隔离成 `.IPGSD`（且呈 churn 态——钩子宿主
  append 时明文短暂存在、随即整体转密文，审计恰逢其死则证据为零）。现在
  `audit` 在读取前显式检查并警告：`.IPGSD` 影子存在 → 点名「被安全软件隔离，
  import 证据不可用，下次你自己的 import 时日志自愈」；无影子 → 「钩子在装但
  尚无 import 事件（或日志被移走）」；未装钩子不警告（正常态）。README import
  段补隔离场景与 `OA_IMPORT_LOG` 重定向逃生舱。取证顺带：R9 展望的「多分钟
  探测爆发」未复发（无数据不定阈值）；全量审计实测 21.4s（36 包 / 16.9MB
  事件 / 4 python 环境，如实记录不优化）。

- **CI 对齐本地门禁基线 + 发布产物 job**（R11 审计 R1）：CI 早在 v0.2.0 已
  存在并实跑过，但 R8 之后与本地基线漂移——etw-capture 的 **release 配置**
  clippy/build（GENERIC_WRITE 事故的直接防线）从未进 CI。现补齐：etw
  release clippy + release build、concurrency（同分支取消旧跑、tag 不取消）、
  tag（`v*`）触发的 release-artifacts job（三 workspace release 构建 +
  artifact 上传三份 exe，不自动建 GitHub Release）、ubuntu advisory check
  （core/cli 的 Unix 分支从未验证过编译，`continue-on-error` 不阻塞，连续绿
  后转正）。教训入 AGENTS：本地门禁基线演进时必须同步 CI 文件

- **npm 卸载残骸检测（孤儿启动器）**（R10 审计 R1）：npm uninstall 半途失败
  （本机实测为安全代理干扰）会删掉 node_modules 里的包目录、留下 prefix 根的
  启动器三件套 `X`/`X.cmd`/`X.ps1`（及 .IPGSD 隔离副本）——死文件躺在 PATH
  目录上敲命令即报错，且此前对审计完全不可见。现在 `audit` 单列
  **ORPHAN SHIMS** 报告节 + `[npm] N orphan shim set(s)` 警告，`clean` 脚本
  附带逐文件删除行（PowerShell 单引号字面量形态）；保守规则：node 自管核心
  命令（npm/npx/corepack）与 `.bat` 脚本（用户自建转发脚本）永不触碰，
  有归属证据（bin 字段 / 加密 fallback / 包目录名）的启动器一律保留。
  实机验证：恰好命中 3 组残骸（opencli ×2、ocusage ×1，共 18 文件），
  活启动器零误报

- **etw-capture 无窗口常驻**（R8 审计 R1/R2，用户直接诉求）：release 构建切换为
  **GUI 子系统**（debug 保持控制台子系统，`cargo run` 开发体验不变）——计划任务/
  登录拉起不再弹控制台窗口；交互输出经 `AttachConsole` 衔接，**只补空缺的 std
  句柄**（父进程布置的管道/重定向不抢，cargo test / bash 捕获照常拿到输出）；
  守护侧诊断（会话启停 / 日志轮转 / 4688 标志自愈 / 追加失败）落
  `<数据目录>/etw/capture.log`（每行时间戳；`OA_CAPTURE_LOG` 可重定向，单测隔离用）

- **采集健康度警告**（R7 审计 R3/R4）：① ETW 最新事件距本次审计超过窗口 1/4 且
  ≥2 天时提示 `[etw] newest event is Xd old — capture may be down`——capture
  中途挂掉后窗口内仍有旧事件，「ETW 开启」会静默掩盖近几天的盲区；② import
  日志超 16MiB 时提示手动清理（多进程并发 append 下 rename 有竞态，不做自动
  轮转）
- **import 钩子采集端**：`orphan-audit hooks install` 为机器上每个 python 解释器部署
  `.pth` 观察钩子、为 node 写入用户级 `NODE_OPTIONS --require` 钩子——进程加载顶层包时
  追加一条 import 事件到 imports/events.jsonl，审计时经 pip top_level.txt 映射回发行包名，
  作为新的 `import` 使用信号（覆盖 IDE / agent 直接 `node x.js`、`python -c` 调包这类
  终端历史完全看不到的使用）。钩子运行在一切业务进程内部：吞掉一切异常、零依赖、
  每进程每包只记一次；`hooks status` / `hooks uninstall` 对称管理；`OA_IMPORT_LOG`
  可整体重定向日志路径（读写两侧同函数，测试隔离用）
- **4688 命令行并入采集端**：etw-capture 启用「进程创建」审计 + 命令行包含
  （子类别用 GUID 引用，中文系统不认英文名），常驻轮询 Security 日志把 node/python
  进程事件（含完整命令行）并入 events.jsonl——命令行经 install_path / `python -m 模块`
  归因到包；非解释器进程不写（Kernel-Process 已覆盖，避免双计）
- **Kernel-File 深度模式（opt-in spike）**：`etw-capture.exe deep` 订阅 FILE_IO_INIT
  把 site-packages / node_modules 下的文件打开事件归因成包写入 import 事件日志；
  只取 Create（opcode 64，路径属性实测叫 **OpenPath** 而非 FileName）+ 每包每进程去重
  + 过滤 `.dist-info` / `__pycache__` / `.pth` / TSD 隔离副本等非包段（实测把 11 秒
  6500 行压到 24 行）。CPU 代价可感知，只在前台手动短时运行
- **GUI 勾选卸载**：表格行首勾选框（表头勾选框全选当前筛选可见行）→ 底部操作条
  显示勾选数与合计大小 →「移除所选」**两步确认**（第一次点击只进入确认态，按钮变红
  点名数量，再点同一按钮才执行）→ 逐个运行 core `uninstall_command` 生成的卸载命令
  （与 CLI clean 同源：pip 定位到所在解释器、npm 非 prefix 显式 `--prefix`，经 PowerShell
  执行、不弹控制台窗）→ 完成后自动重新审计并核验：安全代理可能吞掉卸载进程的输出、
  退出码失真，「卸没卸掉」以重审计后的清单为准，未消失的包在操作条点名。选择变化
  即复位确认态；勾选含非「未用」档包时操作条琥珀色提示先核对判定依据
- **GUI 判定分布条仪表盘**：五档占比堆叠色条 + 计数图例取代六张同款统计卡——
  一段色就是一档的包数比例（占比即数据，不是装饰）；色块与图例点击按档位筛选
  （再点一次恢复），图例首项「全部 N」为显式恢复入口；悬停图例查看档位含义
- **pip/npm 环境归属标注**：同名同管理器的包分布在多个环境（npm 默认与独立
  prefix、多个 python 解释器）时，表格包名后标注归属环境短名（如 opencli 的
  「默认 / npm-global」两行各自标注）以示区分；唯一实例保持安静——完整安装
  路径在证据面板以等宽字体呈现，清理时按路径分清卸载对象

- **多环境检测**：npm 枚举本机全部全局 prefix（默认 prefix + nvm/fnm 版本目录 +
  指纹扫描——`node_modules\.package-lock.json` 或同名成对 .cmd/.ps1 shim 且根上
  无 package.json，实测捕获 QClaw 的独立 `%APPDATA%\QClaw\npm-global`，+3 包）；
  python 枚举全部环境（`py -0p` 注册表项 + Program Files/用户目录/conda/uv 安装根
  + PATH，实测 4 处）。检测到的环境列入 CLI `audit` 输出与 GUI「环境」行；
  项目内 .venv 不扫（项目依赖非全局 CLI）；各环境自带的 pip/setuptools/wheel
  引导设施不参与审计
- **GUI「环境」行**（多环境检测的可视化）：npm 全局 prefix 与 python 解释器各一组
  短名标签（彩点区分种类：npm 蓝 / python 绿；uv 托管目录截出版本号、默认 prefix
  标「默认」），悬停每枚查看完整路径；每类只有 0/1 处时不显示（默认单环境无信息量）
- 管理器标签行（UniGetUI 式分类浏览）：「全部」+ 各出现过的管理器（cargo / npm /
  pip / …）标签各带包数，点击只看该管理器、可与仪表盘档位筛选叠加；本机未安装的
  管理器不出现，winget 仅便携包的取舍在行内「？」说明
- 「？」悬停说明体系（gpui 原生 tooltip）：各路信号（shell 历史 / ETW / 访问时间 /
  配置 / 别名）与两路辅助信号（还原命令 / 漂移）各配「原理 → 判定 → 局限」三段说明；
  五档判定、观察窗口、证据链、清理脚本语义同样悬停可查。文案与 core 判定实现
  （verdict.rs 规则顺序与自适应阈值、signals.rs 扫描、lib.rs 漂移富化）一一对应
- exe 图标：`gui/assets/generate_icon.py` 纯标准库程序化生成（SDF 光栅化 → 面积平均
  降采样 → PNG-in-ICO，256/64/48/32/16 五档），`build.rs` 经 winresource 嵌入
  （Explorer / 任务栏 / 标题栏）
- winget 便携包采集器：解析 `WinGet\Links` 符号链接 → `Packages\<包ID>` 归组，
  bin 名 = 链接名（用户输入的命令），Confidence::Exact；install_path 供 atime 探测
  与新装观察期。MSI/EXE 应用不纳入：`winget list` 无法区分「winget 安装」与
  「安装器自装恰好匹配目录」，且无 bin 清单可用（纳入会产生无证据的「未用」误报）

### Fixed

- **自动化冒烟探测被当成使用证据**（R9 审计 H1，实机取证）：agent / 安全工具批量
  验证 CLI 可用性时（实测某 agent 用 GNU timeout 6 秒内逐个启动 9 个命令），8 个
  包被误判 indirect（「executed 1x via timeout.exe」）。新增 `discount_probe_bursts`：
  同一非交互父进程一分钟内拉起 ≥5 个不同命令、且命令无其他证据（历史/import/
  簇外 ETW）→ 整簇剔除并警告。**实施中发现并修复互为保护缺陷**：探测执行触碰的
  atime 与爆发 ETW 事件同源（实测执行 06:09、atime 惰性刷新 06:16），atime 不得
  作为爆发保护，否则爆发与扫盘两个折扣互相救活——爆发折扣先行，atime 折扣只认
  存活的 ETW。实机验证：8 个假 indirect 全部回到 unused；重复使用（pnpm 6×、
  headroom-ai 80×）不受影响
- **atime 扫盘折扣漏小聚类**（R9 审计 M1）：日期级阈值 ≥5 漏掉 4 命令的秒级扫盘
  聚类（实机 11-05/11-06 两个同秒 4 命令簇，仅因恰逢新装被安装触碰窗兜住）。
  折扣两层化：分钟级「同一分钟 ≥4 个不同命令」+ 日期级 ≥5（慢扫盘兜底）；
  并新增保护规则——窗口内有历史/import/ETW 证据的命令 atime 保留
- **4688 事件时间戳失真**（R8 审计 R3/M1）：事件自带的 SystemTime 此前被丢弃、
  落盘用轮询时刻——轮询 5s 周期 + 2 分钟重叠窗 + 停机续传场景下事件时间最多
  偏晚数分钟，直接影响判定窗口过滤与 last_seen 展示（窗口边界事件可被推迟的
  ts 推出窗外）。采集端现经 `append_etw_event_at` 落**事件真实 SystemTime**
  （解析失败回退 now 不丢事件）；实机实证偏晚 4.75s → 64ms。附带：单轮查询
  上限 `/c:500` → `/c:2000`（突发溢出语义见注释）
- **`hooks install` 写入的 NODE_OPTIONS 反斜杠路径让一切新 node 进程启动即崩**
  （外部反馈实锤，P0）：旧实现把钩子路径按 `Path::display()` 原样写入
  `--require "C:\Users\...\oa_node_hook.cjs"`——node 解析 NODE_OPTIONS 用类 POSIX
  引号规则，`\` 被当转义符吃掉（`C:\Users\...` → `C:Users...`），--require 找不到
  文件 → 编辑器、计划任务、agent 的一切新 node 进程 rc=1（实测反斜杠 rc=1、
  正斜杠 rc=0）。修复：写入一律**正斜杠**形态；旧反斜杠形态在下次 install 时
  识别为待刷新值自动纠正（自愈）；strip 按文件名标记匹配，两种形态都能摘除。
  本机注册表已修复并实测 node 带钩子正常启动
- **atime 扫盘误判**（R7 审计 R1，实机取证）：安全软件定期扫盘把一批包的文件访问
  时间聚在同一日（本机实测 11 个包的 atime 精确落在同一天且无任何其他信号）——
  窗口拉宽（`--since 90d/1y`）或下次扫盘落入窗口时，这批真 unused 包会全部误判
  indirect。现识别「同一 UTC 日期 ≥5 个不同命令的 atime」为扫盘日，该日 atime
  证据整体剔除并打警告（atime 是最弱执行信号，真实使用总有 ETW/import/历史兜底）。
  实机验证：`--since 90d` 下 11/11 扫盘包保持 unused（修前必翻 indirect）
- **命令行归因两处失真**（R7 审计 R2，潜伏缺陷）：① pip 包的 install_path 是同解释器
  全部 pip 包共享的 site-packages 根，任何含该路径的命令行会同时归因到所有 pip 包
  ——现 pip 跳过路径归因（由 `python -m 模块` → top_level 映射承担）；② 子串匹配
  无边界，`node_modules\lodash` 会误吃 `node_modules\lodash-cli` 的执行——现要求
  命中段落在目录边界上（串尾或紧跟 `\`）
- 命令名大小写不一致漏判在用（R6 审计 R1）：`owns_binary` 原按字节比较，Windows
  命令名不区分大小写，ETW 上报与历史敲法的大小写不可控——`RG`/`rg`、`Code`/`code`
  不一致即丢全部使用证据，把在用包误判 unused（最危险方向）。现统一小写比较
- GUI 勾选卸载链路三处状态安全（R6 审计 R2/R3）：
  ① 卸载完成核验按「包名 + 安装路径」配对，同名多环境包卸掉一份不再误报全部残留
  （此前仅按包名去重计数）；
  ② 重审计返回后行选择与确认态按旧结果索引，行序变化即勾错行——重审计成功即
  清空勾选与两步确认态；
  ③ 连续两次审计的回调可能交错返回，旧结果覆盖新结果——引入审计代数，
  过期回调直接丢弃
- 4688 命令行注册标志被环境清掉后事件退化为无命令行（R6 审计 R4；实测标志写入
  数分钟内即被抹）：采集循环每 600 秒核对两个注册表位置
  （`Policies\System\Audit` 与旧版 `Control\Lsa`），缺一处即重写并 stderr 提示，
  无需人工盯守
- import 钩子漏盖 uv/pipx 工具 venv（R6 审计 R5）：`.pth` 只部署到解释器
  site-packages，而 uv/pipx 的工具 venv（`%APPDATA%\uv\tools\<tool>`、
  `%LOCALAPPDATA%\pipx\venvs\<tool>`）不继承——工具自带的库在自身 venv 里的
  import 完全无证据。现 install 时向每个工具 venv 直播 .pth（`UV_TOOL_DIR` /
  `PIPX_HOME` 重定向同步生效），`hooks status` 显示 `tool venvs hooked`；core
  `module_to_package_map` 对 uv/pipx 包改扫其 venv 的 `Lib\site-packages`
- `NODE_OPTIONS` 钩子段摘除破坏第三方 `--require`（R6 审计 R7）：uninstall 原先
  摘掉所有裸 `--require` token；现按「`--require` + 含本钩子标记的路径」成对摘除。
  merge 侧同步修复陈旧路径：原先只认标记存在，钩子文件移动后旧路径永久残留——
  现标记**与**当前路径同时满足才跳过，否则摘旧段再追加新路径
- `--since` 数值溢出静默回绕（R6 审计 R8）：`n × 单位` 溢出 u32 在 release 下
  回绕成小观察窗口（如 `99999999y`），在用包被误判长期未用；现 `checked_mul`
  溢出报错
- `clean` 生成的 PowerShell 脚本在记事本/传统终端里中文乱码（R6 审计 R9）：
  写出时补 UTF-8 BOM，Windows PowerShell 5.x 才按 UTF-8 解码
- GUI 证据面板「导入 N 次」格式未翻译（R6 审计 R6）：`imported 3x` 原样透出
  英文，现译「导入 3 次（import 钩子）」，格式测试同步覆盖
- 已卸载的 npm 包在重新审计后「死而复生」：终端安全代理隔离被删文件时会在
  `node_modules`（含 `@scope` 内层）留下**点前缀暂存目录**（实测
  `.opencli-XXXXXX`，内容为 `.IPGSD` 隔离副本，package.json 仍可读），包采集器
  曾把它们当成安装中的包照常收录——用户卸载 opencli 后清单里依旧出现同名条目
  （install_path 指向暂存目录）。npm 从不安装点前缀目录，现顶层与 `@scope`
  内层的点前缀条目一律跳过（回归测试覆盖两种层级）
- 勾选卸载误报「失败」：卸载结果此前按子进程退出码判定，而终端安全代理会吞掉
  卸载进程的输出、扭曲退出码（实测 npm 已把包目录删除却报非零退出）——同一台
  机器上「失败」与「已从清单消失」同时出现，自相矛盾。现退出码仅作过程信息，
  结论只按重审计后的清单消失与否给出：全部消失「已移除 N 个包」、部分残留
  「仍在清单：…（请到终端手动处理）」；同名多环境包在提示中去重
- 同名包多实例打破存储主键（2026-11-19 实测：@jackwener/opencli 同时装在默认
  prefix 与 QClaw prefix，多环境检测上线后**每次审计的持久化都失败回滚**，
  库内清单停留在旧数据）。`packages` / `verdicts` 表新增 `install_key` 列
  （install_path 的字符串形式）并纳入主键，旧库打开时自动表重建迁移；
  漂移 streak 改按月 GROUP BY 计数且每月取 MIN(verdict)——任一副本在用即
  断档，保守语义防误标可删
- 用户级 site-packages 的 atime 探测路径拼错：pip 入口目录固定取 site-packages
  的两层父目录（正确于 venv/系统布局），但用户级布局
  `%APPDATA%\Python\Python313\site-packages` 的入口在**一层**父目录
  `Python313\Scripts`——用户级 pip 包此前全部丢失 atime 证据与新装观察期
  保护（刚装即入清理候选）。现两种布局的候选都探测
- GUI 窗口原生标题栏无标题（任务栏 / Alt-Tab 同样无名）：`WindowOptions` 缺省
  未设 `titlebar.title`，补「orphan-audit」
- 清理脚本预览没有实际内容：此前取脚本前几行——全是样板注释（Generated by /
  $ErrorActionPreference），真正的卸载命令根本不出现（修复后该功能即被勾选卸载
  取代并整体移除，见 Removed）
- CLI `--db` 参数原先只对 `drift` 生效，audit / status / clean 无视它写死默认
  路径；现四个子命令一致生效（可在受控路径验证持久化）
- 清理脚本平台错位：`clean` 在 Windows 上生成的曾是 bash 脚本（`#!/usr/bin/env bash`
  + `set -euo pipefail`），现生成 PowerShell（`$ErrorActionPreference = 'Stop'`，
  go 清理用 Remove-Item）。多环境下卸载精确性同步修复：pip 按包所在环境定位
  解释器（venv/系统布局直调 `& "python.exe"`；用户级 site-packages 经 `py -3.13`
  版本标签），npm 非 prefix 的包显式 `--prefix`（scoped 包比 plain 多一层
  @scope 目录，prefix 推导沿目录向上找 node_modules 父目录，此前曾推出
  node_modules 本身）
- GUI「？」悬停窗尺寸不足、文字重叠：gpui 以 AvailableSpace::Min 测量 tooltip，
  max_w 约束下文本拿不到确定宽度 → 不换行、行高塌陷叠行。改为定宽 360px 后
  子文本以 Definite 宽度测量，正常折行堆叠
- IDE/agent 以 node 直调的 npm 工具被误判 unused（2026-11-19 实测 tokentracker-cli：
  Trae CN 直接运行包内 `bin/tracker.js`，prefix 下的 .cmd/.ps1 shim 完全不被触碰，
  而旧 atime 探测只看 shim）。修复：atime 探测按包枚举 `bin/` 下 .js/.mjs/.cjs 入口
  脚本（bin 映射里命令名与脚本名不同也覆盖，如 `{"foo-cli": "bin/foo.js"}`），
  并补 .ps1 shim 候选；此类工具现正确判 indirect
- npm 采集在 Windows 上静默为空（本机实测 0 → 11 个）。根因两层：
  ① nodejs 官方安装的 npm 是 `npm.cmd`（无 npm.exe），CreateProcess 只能直接启动
  .exe，探测直接失败；② 即便经 `cmd /d /c` 转发成功，终端安全代理（TSD/深信服）
  会吞掉非信任进程树下 node 的全部 stdout——`node -e "console.log(1)"` 都返回
  exit 0 + 空输出（python/cmd 不受影响）。修复：collect_npm 改为**纯文件系统采集**
  ——直接枚举 `<prefix>/node_modules`（含 @scope）并读各包 package.json（与
  `npm ls -g --json` 同一数据源），prefix 按 npm 优先级解析
  （`npm_config_prefix` > 用户 `.npmrc` > Windows 默认 `%APPDATA%\npm`），
  不再运行任何 node 子进程，免疫安全代理且更快
- 附带修复：终端安全软件把 agent 安装的包加密重命名为 `*.IPGSD`（目录名/文件名
  保留、内容不可读，2026-11-19 实测 @runta/runta-cli）导致 package.json 读不到时，
  兜底用包目录名 + `bin/` 脚本文件名恢复包与命令映射，置信度降为 Inferred
  （GUI/报告标注「推断」提示复核）
- 通用修复：新增 `command_for()`——PATH 上找不到 `<name>.exe` 时回退
  `cmd /d /c`（cmd 按 PATHEXT 解析 .cmd），`tool_available` 等探测不再因 .cmd
  形态的工具（npm/pnpm 等）误判未安装

### Changed

- 信号「漂移」更名「长期未用」（GUI 证据标签与悬停说明、README）：直白描述证据
  含义（连续数月快照均判未用），同时避免与 CLI `drift` 子命令（对比两次快照的
  新增/移除）重名混淆；代码内部标识与月度快照机制名不变
- shell 历史时间戳按行位插值：PSReadLine 无时间戳的历史文件不再整体落 mtime——
  以文件创建/修改时间与行数线性估计每行的使用时间（zsh EXTENDED_HISTORY 真实
  时间戳仍优先），观察窗口过滤不再把整份历史压到文件最后修改的那一刻
- 审计加载态改为转圈动画：表格体中央 8 点追逐环（gpui `Animation` 驱动逐点
  不透明度），取代「正在审计……读取 shell 历史、包管理器清单与配置（约数秒）」
  占位文案；状态行的「正在审计…」同步移除——加载与否看动画，出错才出文字
- hooks uninstall 输出诚实化（R6 审计 R10）：node 侧此前在什么都没动时也打
  「NODE_OPTIONS cleaned」，现按是否真移除分段表述；status 的解释器钩子计数
  只扫一遍解释器列表（原两遍）
- 界面去噪（勾选卸载上线的配套裁剪）：移除「预览清理脚本」入口与底部预览条
  （勾选卸载直接取代这条迂回路；CLI `clean` 生成脚本的能力保留）；状态行不再
  常驻「N 个包，历史 M 条，ETW 开启」——包数看图例「全部 N」、ETW 降级看警告
  横幅，状态行只在审计进行中/失败时出现；证据面板空态删去操作引导段落，
  只留信号标签行（不再提「五路」，有什么展示什么）
- 状态行精简为「N 个包，历史 M 条，ETW 开启」一眼可读的短句：原先把全部历史
  来源文件名整串拼进右上角（长到看不懂），现来源明细与各字段含义移入状态行悬停
  说明逐条展开；多环境计数后缀（npm prefix ×N）移除——由「环境」行完整呈现。
  `detected_envs` 从字符串改为结构化 `DetectedEnv { kind, path }`（CLI 文本
  输出格式不变）
- 使用说明去面板化：移除默认展开的使用说明面板与工具条开关——信号总览移入
  证据面板空态、ETW 开启指引移入审计警告横幅、扫描范围说明移入管理器行「？」，
  其余说明由各控件自带的「？」悬停承载（界面少一整块、首次打开更聚焦数据）
- 「？」图标居中修正：全角 ？ 的墨迹偏字面左下（圆圈内视觉不居中），改半角 ?
  并做行高 / 1px 光学对齐
- GUI 中性灰阶配色：替换蓝味深底（70 处色值），蓝色只用于交互态（悬停 / 激活 /
  主按钮）与 npm 环境语义点；五档判定色、七路信号色、琥珀警告色不变
- 去 AI 味细节：状态行与证据副标题的分隔中点改为逗号、标题副标题的破折号改为
  逗号、表格管理器列由小标签改为扁平文字、tooltip 标题去蓝色
- 仪表盘档位筛选：五档分布条色块与图例点击按档位筛选表格（再点一次或点「全部」
  图例恢复），图例悬停显示档位含义与点击行为；取代原「仅看可清理（未用）」按钮
  （筛选状态以工具条标签呈现）
- 视觉条理化：斑马纹表格 + 选中行档位色左条、判定列色点、管理器列扁平文字、
  证据卡源色 badge（与帮助面板信号色一致）、观察窗口分段控件、清理脚本等宽字体
  预览、标题栏徽标 + 版本标识（v{CARGO_PKG_VERSION}）
- 帮助面板重排：各路信号以色点标签行呈现（每枚带 ？），操作说明同步仪表盘筛选

### Removed

- GUI「预览清理脚本」按钮与底部预览条：勾选卸载（直接勾选、两步确认、执行后
  自动核验）取代「生成脚本 → 审阅 → 手动执行」的预览迂回路；CLI `clean` 仍可
  生成完整 PowerShell 脚本（审阅式清理路径保留）

## [0.2.0] - 2026-11-18

> 首个正式发布版本（0.1.0 为仓库内记录的 golden 基线，未单独发布）。
> 相对 0.1.0：ETW 采集端与常驻化、新装观察期、漂移信号、GUI 全面中文化与
> 表头排序/窗口档位/会话级忽略、准确性增强（本地化解析/安装期过滤/全宿主历史/
> wrapper 还原）、事件管道治理（窗口过滤 + 64MB 轮转）、降级语义修正。

### Added

- GUI 忽略入口（补齐 CLI `--ignore` 对等能力）：证据面板「忽略此包」按钮 +
  工具条「已忽略」标签（点击 × 恢复并重跑审计）；会话级，变更即重跑完整审计
- 事件管道治理：`core::etw::read_etw_log_since` 窗口过滤——audit 只读 `--since` 窗口内
  （since-1 天）的 ETW 事件，窗口外事件不进判定与计数；采集端启动时日志超 64MB 自动
  轮转为 `events.jsonl.1`（常驻采集下日志不再无限增长）
- `install` 任务设置加固：schtasks 创建的计划任务默认「电池模式停止 + 72 小时执行时限」
  ——两者都会杀死常驻采集（2026-11-18 实测：笔记本切电池即被终止）。install 现用
  PowerShell `Set-ScheduledTask` 改为电池容忍 + 无时限（失败告警不阻断）
- GUI 审计窗口档位（30 天 / 90 天 / 1 年，切换即重跑审计，since 影响判定不能只重排缓存）、
  「重新审计」按钮、审计警告横幅（采集器警告黄条常驻展示，不再只打到终端）
- 新装观察期：`VerdictResult.newly_installed`（安装时间落在 `--since` 窗口内的包）。
  观察期包不进入清理候选——CLI `clean` 脚本与 GUI 清理预览自动跳过并留痕（脚本注释、
  报告标注、GUI 证据面板提示「新装（观察期）」）。采集器未提供安装时间的包由
  `audit` 用二进制文件创建时间兜底填充（`fill_installed_at`）
- 漂移即信号：`SignalSource::Drift` + `Store::unused_streak_months`。连续 ≥3 个月快照
  均判 unused 的包在证据链追加漂移证据（GUI 显示「连续 N 个月快照均判 unused」），
  区分「本月恰好没跑」与「长期没人用」；判定档位不变
- ETW 采集端常驻化：`etw-capture install / uninstall / status` 子命令——复制自身到
  数据目录、注册登录自启计划任务（schtasks ONLOGON /RL HIGHEST）并立即启动；
  `uninstall` 停会话删任务。已知取舍：随任务启动会显示一个控制台窗口（见 DESIGN.md）
- 采集端外部停会话可优雅退出：事件线程返回即触发停机标志（uninstall 停会话后进程
  自行结束，不再挂死在 Ctrl+C 等待上）

### Changed

- **降级判定语义修正**：ETW 可用性按「审计窗口内是否读到事件」判定
  （`tracker_from_log_events`），而非只看当前进程是否提权——常驻采集写入日志后，
  **未提权**的审计也能消费事件，旧逻辑会在此误报「no ETW events available」；
  降级警告文案同步改为可行动指引（管理员运行随仓库的 etw-capture，或
  `etw-capture install` 常驻；旧文案「采集器未随仓库提供」已过时）
- `install` 升级路径：部署目标正在运行时（Windows 锁定运行中的 exe）先停 ETW 会话、
  超时再强制结束任务，然后拷贝新版并重启——运行中升级不再直接失败
- uv / pipx 安装路径解析：`uv tool list` 的括号路径与 pipx `venvs` 目录（PIPX_HOME 优先）
  ——两管理器的包也能用文件创建时间兜底观察期判定，不再只覆盖 cargo/npm/pip
- ETW 判定计数限窗：`etw_hits` / `last_exec` / 证据只统计 `--since` 窗口内事件，
  窗口外命中不再虚增 indirect 证据
- `TrackerStatus` 精简为 `available` / `reason`：删除全仓无消费方的
  `events_captured` / `uptime_secs`（write-only，且迫使 probe 全量解析日志）
- `drift` 子命令校验快照名存在，不存在的名字直接报错并列出可用快照
  （此前按空集 diff，会把所有包误报为 removed）
- 清理脚本 / GUI 预览的观察期跳过说明去重合并（skipped-note 闭包，两条输出路径共用）
- CI 补 `cargo fmt --check` 门禁并覆盖 etw-capture workspace（root / gui / etw-capture
  三处 test + clippy + fmt）
- 历史解析增强：PSReadLine 改为枚举目录下全部 `*_history.txt`（VSCode 等新宿主的
  历史不再漏采）；zsh EXTENDED_HISTORY 行（`: <ts>:<elapsed>;cmd`）优先用行内时间戳
  而非文件 mtime；wrapper 扩充 watch/nice/stdbuf/timeout，且连带其 flag/时长参数
  剥离（`timeout 10 make` 不再被记成名为 "10" 的命令，也修复了 `time -p make` 记成
  "-p" 的潜在问题）
- GUI 全面中文化：界面文案、判定档位/信号来源/置信度名与证据细节（展示层翻译，
  core 输出格式不变）；新增「使用说明」面板（运作方式、五档判定、操作、安全边界，
  默认展开可收起，ETW 未开启时提示运行采集器）
- GUI 排序交互重做：移除工具条排序按钮，改为**点击表头排序**（当前列高亮 + ▲/▼
  方向指示，同列再点反向），新增「输入次数」「大小」两列排序（按数值而非展示文案）
- `probe_atime` 直接以命令名构造事件（签名加 `binary_name` 参数），消除调用方覆写占位
  binary 的脆弱契约（等价重构，审计 A9）
- `CREATE_NO_WINDOW` 提为 `inventory` 公开常量，etw/signals/etw-capture 的探测与
  schtasks 命令统一引用，不再内联裸数字（一致性清理，审计分支步骤 3）
- `model::UsageEvent::command_line` 文档注释与实证对齐（较新 Windows 构建的 ETW 事件
  无命令行字段）

### Removed

- write-only 的 `ignores` 表及 `Store::sync_ignores` / `Store::ignores()`：全仓无任何
  读取方，写入结果不可观测（死代码清理，审计 A10；旧库中的遗留空表无害）

### Fixed

- **A1（P1）**：`fsutil behavior query disablelastaccess` 本地化输出解析失败（中文系统
  输出带括号说明后缀 `= 2  (托管的系统，…)`，旧实现对整段做数值解析 → None），
  导致中文 Windows 上 atime 信号被静默跳过。抽出 `parse_disablelastaccess` 纯函数，
  只取 `=` 后首个空白分隔 token 再做十六进制解析（附中/英文 fixture 测试）
- atime 探测剔除安装期触碰：二进制创建后 24h 内的访问视为安装动作本身而非执行
  （`atime_is_evidence`），新装包不再因安装触碰被误判 indirect
- GUI: 点击行后，排序/筛选状态下证据面板与高亮指向错误的包（选中索引错位，可误导删除决策）
- shell 配置索引与查询大小写不一致，Windows 上 `Set-Alias Cat BAT` 类配置漏报 integration（误判 unused）
- `report_from_store` 离线模式用空事件集重建判定，所有包必判 unused
- npm 字符串形 `bin` 字段映射为脚本文件名，应为包名（scoped 包取末段）
- atime 探测路径仅对 cargo 正确；现按管理器推断（cargo/npm/pip），并对全部 binaries 探测
- `split_commands` 把 `cmd 2>&1` 拆出名为 "1" 的伪命令段
- GUI "Last run" 排序改用结构化天数字段，不再解析显示字符串

### Added

- ETW 采集端 `etw-capture/`（S17，独立 workspace）：管理员订阅 Microsoft-Windows-Kernel-Process
  进程创建事件（ferrisetw UserTrace，keyword 0x8010 含 rundown 预热），按 core 契约追加
  `events.jsonl`；PID→ImageName 缓存解析父进程名；非管理员启动即报错；同名残留会话幂等清理。
  实测（Win11 build 28000）：该事件无 CommandLine 字段，`cmd` 恒为 null，归因不受影响
  （详见 etw-capture/DESIGN.md）
- MIT LICENSE 文件
- GUI 审计移入后台线程：窗口立即出现并显示 "Auditing…" 加载态，冷启动不再阻塞
- `Store::events()` 事件读取；`report_from_store` 以库中真实事件重估判定
- 采集器纯函数解析（`.crates2.json` / `uv tool list` / `pipx list --json` / pip entry_points）及单测，
  `.crates2.json` 测试改用内置 fixture，不再依赖本机文件
- CLI 漂移对比 `diff_names` 纯函数与 `--json` 输出形状测试
- CI（GitHub Actions：test + clippy，root 与 gui 两个 workspace）
- GUI 视图层单测（选中解析、筛选、排序、Summary 计数）

## [0.1.0] - 2026-11-18

### Added

- 五档判定引擎（active / indirect / rare / integration / unused）+ 可反驳证据链
- 采集器：cargo / npm / pip / uv / pipx 全局包清单与包→二进制映射
- 信号：PSReadLine / bash / zsh / Atuin 历史解析、shell 配置与 alias 索引、NTFS atime 探测、
  ETW 事件日志消费（`events.jsonl`，采集端为规划组件）
- SQLite 存储 + 月度快照与漂移对比
- CLI：`audit` / `status` / `clean` / `drift` 子命令，支持 `--json` / `--since` / `--ignore` / `--manager`
- GPUI 桌面界面：仪表盘 + 包表格 + 证据面板 + 清理预览，支持筛选/排序

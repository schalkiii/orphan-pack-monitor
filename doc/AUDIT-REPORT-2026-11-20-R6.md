# 审计报告 R6（2026-11-20）

> 审计范围：全部代码（core 9 文件 5523 行 / cli 647 行 / gui 2820 行 / etw-capture 804 行，
> 共 9383 行）与文档（README / AGENTS / CHANGELOG / etw-capture/DESIGN.md / 既有审计报告）。
> 背景：批次⑩（import 钩子 / 4688 / deep / 伪时间戳）落地后的第一轮全面体检。
> 高严重度条目均已由第二读者对照源码逐行复核（标注「已核实」）。

## 一、现状描述

- **规模**：4 个 crate（core / cli / gui / etw-capture），97 个单元测试全绿，
  三 workspace clippy `-D warnings` / fmt 清零。
- **结构**：core（清单采集 → 信号消费 → 五档判定 → 卸载命令）为纯库；
  cli 是薄入口；gui（GPUI）消费 core 的 `AuditReport`；etw-capture 独立 workspace
  常驻采集（Kernel-Process ETW + 4688 轮询 + deep spike）。
- **质量水平**：纯函数普遍有测试且断言具体（非「跑通即过」）；文档与代码总体同步
  （批次⑩后 README/AGENTS/DESIGN/CHANGELOG 均已更新）；薄弱面集中在 **GUI main.rs
  （2009 行，零测试）** 与新上线的钩子/4688 链路的边界。
- **准确性主线现状**：归因引擎（owns_binary / attribute_cmdline / module map）的
  子串与 scoped 包处理正确；发现一处**大小写敏感漏配**（见 H1）与若干覆盖缺口
  （uv/pipx venv 无钩子、4688 标志被环境回写后静默失灵）。

## 二、问题清单

### 高严重度

| # | 位置 | 问题 | 影响 |
|---|------|------|------|
| H1 | core/model.rs:111-114 `owns_binary`（已核实） | 归因用 `==` **大小写敏感**：`basename(b) == command`。Windows 命令名不区分大小写，ETW 侧 strip_exe 保留内核原始大小写、历史保留用户敲法；`Code` vs `code`、`RG` vs `rg` 全部漏配 | 包明明在用却判「未用」进入清理候选——本工具最危险的失配方向（误删）。shell config 索引在 verdict.rs 已小写化，证明此处是遗漏而非约定 |
| H2 | gui/main.rs:133-165 `apply_report`（已核实） | 卸载核验的 `gone = attempted - still.len()` 在 **同名去重之后**计算：双 prefix 同名包两份都卸载失败时 attempted=2、still 去重后剩 1 → 报「已移除 1 个，仍在清单 opencli」 | 核验功能给出与事实相反的结论，用户被误导以为部分成功 |
| H3 | gui/main.rs:114-180（已核实） | `apply_report` 成功路径不清理 `removal_selected` / `removal_confirm`；四处入口（忽略包/窗口切换/重新审计）都会触发重审计，`removal_selected` 是索引，清单增减后勾选框**指向另一个包**；确认按钮只显示数量不显示包名 | 用户可能卸载完全错误的包；红色确认态跨审计存活，一次误点直接执行 |
| H4 | gui/main.rs:203-225 `start_audit`（已核实） | 后台审计无代次守卫：快速连点「重新审计」/切窗口时两个审计并发，旧请求后完成会无条件覆盖新报告并清 loading | 界面数据与档位高亮/筛选属于不同窗口——用户基于错误窗口的数据做清理决策 |
| H5 | cli/main.rs:517 `Clean`（已核实） | `clean -o` 生成的 .ps1 无 UTF-8 BOM；内容含 `& "C:\…python.exe"` 与中文路径时 Windows PowerShell 5.1 按 ANSI 解析 → 路径乱码 | 中文环境用户的审阅式卸载脚本直接失效（GUI 直执行不受影响） |

### 中严重度

| # | 位置 | 问题 | 影响 |
|---|------|------|------|
| M1 | etw-capture/main.rs（本审计新增观察） | `ProcessCreationIncludeCmdLine_Enabled` 注册表值被环境（安全软件/域策略）**数分钟内回写删除**（2026-11-19 实测），4688 事件命令行随之静默变空，采集端只在 install 时设置一次 | 4688 归因（install_path / `python -m`）在用户不知情时全量失效，只剩 binary 级证据 |
| M2 | cli/main.rs hooks（覆盖缺口） | import 钩子只部署到 `python_interpreters()` 枚举的解释器；**uv 工具 venv（%APPDATA%\uv\tools\<tool>）与 pipx venv 有独立 site-packages，无 .pth** | uv 管理的包（本机 2 个）import 使用不可见 → 误判可删；pipx 同理 |
| M3 | gui/view.rs:136-162 `detail_cn`（已核实） | 缺 `imported {n}x` 前缀映射；verdict.rs:126 的 Import 证据在中文界面显示英文原文；且 view.rs:568 测试注释声称「覆盖全部固定格式」与事实不符 | 中文界面证据可读性缺口；守护测试名不副实 |
| M4 | cli/main.rs:242-248 `strip_node_options`（已核实） | 过滤器摘掉**所有**裸 `--require` token——用户自己的第三方 node 钩子会被卸载动作破坏（留下悬空路径） | hooks uninstall 损坏用户既有配置 |
| M5 | cli/main.rs:232-239 `merge_node_options`（已核实） | 只查 marker 不查路径：数据目录迁移后注册表里的旧路径永不被修正，install 报「already configured」 | node import 证据静默丢失 |
| M6 | cli/main.rs:128-129 `parse_since`（已核实） | `n * multiplier` 溢出无检查：`--since 99999999y` debug panic、release 回绕成小值 `.max(1)` | 观察窗口静默失真，判定整体错误且无报错 |
| M7 | gui/main.rs:122-129（已核实） | `apply_report` 硬编码 `SortKey::Verdict/Asc`，而 `rebuild()` 用 `self.sort_key/sort_dir`；筛选跨审计保留、排序不保留 | 用户排序被静默重置，行为不一致（疑似笔误） |
| M8 | gui/main.rs:172-178（已核实） | `Err` 分支不清 `removal_status`，也不清过期 rows/results | 卸载后重审计失败时操作条永远显示「正在重新审计核验…」，表格渲染过期数据 |
| M9 | 测试覆盖缺口 | gui/main.rs 零测试（apply_report/核验/start_audit 全无）；`parse_since` 无溢出用例；`pth_line`/`candidate_sites` 无测试；owns_binary 无大小写用例 | H2/H3/H4/M6/H1 正因此漏网 |

### 低严重度

| # | 位置 | 问题 |
|---|------|------|
| L1 | gui/main.rs:227-235 | `toggle_removal` 注释称「反馈一并清掉」，但 `removal_status = None` 只在勾选分支执行，取消勾选不清 |
| L2 | cli/main.rs:59-65 `Drift` | 只对比 Active/Unused 两档，doc 未说明；Rare/Indirect/Integration 的新增移除不可见 |
| L3 | cli/main.rs:132-137 `default_db` | data_dir 失败静默回退 `"."`，DB 写进任意当前目录且无提示 |
| L4 | gui/main.rs:1162-1166 | `signal_pills`「辅助」分隔位置（i==5 在 Alias 前）与注释「后两枚是辅助」矛盾，实际辅助段三枚 |
| L5 | gui/view.rs:77 | `Summary.observing` 计算并测试但渲染从不读取（死字段） |
| L6 | cli/main.rs:42-44 | `--json` 标注 global 但 Clean/Drift/Hooks 忽略；Audit 的 JSON 只输出 results，warnings/ETW 降级状态丢失，JSON 消费者无从得知证据覆盖不完整 |
| L7 | cli/main.rs:196-202 `pth_line` | 路径直接嵌入 `r"{}`"：含 `"` 或尾随 `\` 的路径生成非法 .pth；依赖 python 以 UTF-8 读 .pth |
| L8 | cli/main.rs:371-373 `hooks status` | `python_interpreters()` 连调两次（每次 spawn `py -0p` 等），开销翻倍 |
| L9 | cli/main.rs:331 | `let _ = py_hook;` 死绑定 |
| L10 | cli/main.rs:365 | uninstall 无条件打印 "NODE_OPTIONS cleaned."，注册表无该值时输出失实 |

### 附：已排查无问题的点

- 子串误配不存在（owns_binary 全等；attribute_cmdline 用 install_path 子串但带
  `len>4` 守卫，短包名如 `mcp` 不会命中 `mcp-cli` 的不同 install_path）；
- scoped npm 归因正确（npm_prefix_of / package_env_label 沿目录上溯，有回归测试）；
- basename 同时处理 `/` 与 `\`；快照 `%Y-%m` 月度语义与文案一致；
- 仪表盘 `n/total` 有 `n>0` 守卫；4688/deep 的会话名互不干扰（DEEP_TRACE_NAME 已修）。

## 三、改进建议（按优先级；标注可并行）

| 优先级 | 建议 | 涉及文件 | 验证方式 | 并行性 |
|--------|------|----------|----------|--------|
| P0 | R1 owns_binary 大小写不敏感：两侧 basename 结果 `to_ascii_lowercase` 后比较 | crates/core/src/model.rs + 测试 | 新测试：`"RG"`/`"Code"`/`"FIRECRAWL-CLI"` 命中；既有 97 测试不回归 | 独立可并行 |
| P0 | R2 GUI 卸载链路修复（H2+H3+M7+M8+L1）：gone 按 (name,path) 对去重前计算、提示名单去重；apply_report 清 removal_selected/confirm 并用 self.sort_key/sort_dir；Err 分支清 removal_status；toggle_removal 两分支都清反馈 | gui/src/main.rs | 新测试（抽纯函数 `verify_removal(pending, results) -> (gone, still_pairs)`）；手工冒烟：双 prefix 同名包勾选卸载失败场景 | 组内串行（同文件） |
| P0 | R3 start_audit 代次守卫：`audit_generation` 自增 + spawn 捕获快照，apply 时代次不符丢弃 | gui/src/main.rs | 新测试：两次 start_audit 乱序完成只应用最新代次（抽纯函数判断） | 依赖 R2（同文件） |
| P1 | R4 4688 标志自愈：poll 循环每 ~10 分钟核对 `ProcessCreationIncludeCmdLine_Enabled`，缺失则重写（warn-only） | etw-capture/src/main.rs | 单测抽取「标志检查间隔」纯逻辑；实机：删值→等一个周期→值恢复且事件带 cmd | 独立可并行 |
| P1 | R5 uv/pipx venv 钩子：hooks install 增加枚举 `%APPDATA%\uv\tools\*\Lib\site-packages` 与 pipx venvs（存在才扫），逐个写 .pth；uninstall 对称清理 | crates/cli/src/main.rs（+core 枚举辅助如需） | 新测试：临时目录伪造 uv tool 布局 → install 后 .pth 存在、uninstall 后消失；实机本机 uv 2 包 | 独立可并行 |
| P1 | R6 detail_cn 补 `imported ` 前缀（中文「导入 N 次」），同步修正守护测试注释 | gui/src/view.rs | 扩展 detail_cn 测试用例 | 独立可并行 |
| P2 | R7 NODE_OPTIONS 修复（M4+M5）：strip 按「--require + 紧随其后的含 marker 路径」成对摘除；merge 发现 marker 但路径不同则替换为新路径 | crates/cli/src/main.rs + 测试 | 扩展既有 merge/strip 测试：第三方钩子存活、旧路径被刷新 | 独立可并行 |
| P2 | R8 parse_since checked_mul：溢出返回错误「窗口过大」 | crates/cli/src/main.rs + 测试 | `--since 99999999y` 报错不 panic | 独立可并行 |
| P2 | R9 clean .ps1 加 UTF-8 BOM | crates/cli/src/main.rs | 新测试：写临时文件断言前三字节 EF BB BF | 独立可并行 |
| P2 | R10 cli 卫生（L8+L9+L10）：status 复用一次枚举；删死绑定；uninstall 按实际动作输出 | crates/cli/src/main.rs | 既有测试 + 手工 status/uninstall 输出核对 | 独立可并行 |
| P3 | R11 文档补注（L2/L3/L6/L7）：drift 两档限制、default_db 回退、--json 覆盖范围写进 help/README | cli/main.rs doc + README | 文档通读 | 独立可并行 |

> **实施状态（2026-11-20 当日）**：R1-R11 全部实施并通过四 workspace 门禁
> （104 测试：core 74 + cli 9 + gui 16 + etw-capture 5；clippy `-D warnings` +
> fmt 清零）。R4/R5 已部署实机（capture 重装运行、hooks 4/4 解释器 + 2/2
> uv/pipx 工具 venv，`import fitz` 事件经 OA_IMPORT_LOG 重定向验证落盘）。
> 第四节展望三项仍未实施，留作后续候选。

### 执行序建议

R1 → R2 → R3（GUI 组串行）与 R4、R5、R6 并行推进；R7-R10 批量小修一次过；
每个改动点独立跑该 workspace 测试（software-improvement 逐点验证纪律）。

## 四、准确性展望（本轮不实施，记录候选）

- **atime 扫描器误报折扣**：安全软件定期扫盘会把全部包的 atime 同日触碰——可检测
  「同日大面积 atime 且无其他信号」的形态并降置信度（需真实数据验证，避免误伤）。
- **UserAssist / Prefetch 兜底信号**：无管理员环境下的执行证据补充（需 ROT13 解码 /
  Prefetch 目录权限，投入产出比待评估）。
- **长驻进程的 import 事件**：钩子在进程启动时挂载，长驻 agent 若先于钩子安装启动，
  其后续 import 不可见（4688/ETW 仍覆盖进程级证据）。

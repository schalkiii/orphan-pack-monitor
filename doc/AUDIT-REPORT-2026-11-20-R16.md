# 审计报告 R16（2026-11-20，第十六轮）

> 触发条件：R15 终轮宣告的恢复条件「新功能落地后」——批㉒~㉕ 新增了 hooks 模块迁移
> （CLI→core）、`store::drift_diff`、`actions` 三个脚本助手、GUI 四个面板（残骸删除 /
> 导出脚本 / 长期对比 / 钩子管理）、警告归组重渲染。本轮聚焦**这批新增面**；
> R5-R15 已覆盖的旧模块不重复深审，仅复核与新代码的交界处。

## 一、现状描述

- 规模：三 workspace（root = core+cli，gui，etw-capture），R15 后新增/改动：
  `core/src/hooks.rs`（自 CLI 迁入，约 530 行）、`store.rs` +drift_diff、
  `actions.rs` +3 助手、`lib.rs` +default_db_path/hooks 接线、`cli/main.rs`
  重写为薄壳（374 行）、`gui/main.rs` +约 700 行（四面板 + 信号总览重写 +
  环境列）、`gui/view.rs` +path_display/env 列门控撤销。
- 门禁基线：132 测试（core 101 + cli 3 + gui 22 + etw 6）全绿；三 workspace
  clippy/fmt 清零。
- 本轮核查过的新增交界：core 警告 `[tag]` 拼法与 GUI `warning_signal` 映射
  逐一比对（etw/signal/imports/atime/npm/store 六种 tag 全部对上，无漏配）；
  `drift_diff` 只比 active/unused 两档有单测；CLI hooks/drift/clean 输出与
  迁移前逐字一致。

## 二、问题清单

### R16-H1 hooks::install/uninstall 在库代码里 eprintln!——GUI 子系统下会 panic（高）

- **位置**：`crates/core/src/hooks.rs` 4 处（install 2 处：.pth 直写+用户级回退
  均失败、版本探测失败；uninstall 2 处：.pth 删除失败）。
- **描述**：批㉒ 把 hooks 从 CLI 迁入 core 时，`eprintln!` 原样带入。CLI 上下文
  无害；但 GUI（release 为 window 子系统）双击启动时 **stderr 句柄无效**，
  Rust 的 `eprintln!` 在写失败时 `panic!("failed printing to stderr")`。
- **后果**：用户在 GUI 点「安装钩子」且任一解释器 .pth 写失败（如 Program
  Files 场景）→ 后台线程 panic → `hooks_busy` 永远卡在「处理中…」，按钮
  永久不可用；线程 panic 的传播行为不可控。
- **严重程度**：高（新功能主路径，触发条件常见）。

### R16-M1 README 仍描述已删除的「警告横幅」（中）

- **位置**：`README.md` 193 行附近。
- **描述**：批㉕ 已把警告横幅改为右栏「信号与诊断」归组渲染，README 的
  「GUI 警告横幅下有『卸载残骸』操作条」中「警告横幅下」已过时。

### R16-L1 [npm] 残骸警告与残骸操作条信息重复（低，接受）

- **位置**：`gui/src/main.rs` signal_overview「其他」组 vs `shims_bar`。
- **描述**：`[npm]` 警告在「其他」组显示（含 clean 脚本提示），残骸操作条
  同时显示组数/文件数与删除入口。两条信息通道各有侧重（提示 vs 行动），
  重复度可接受，不修。

### R16-L2 GUI 安装/卸载反馈不含失败明细（低，随 R16-H1 一并修）

- **位置**：`gui/src/main.rs` hooks_panel 反馈行。
- **描述**：安装/卸载失败明细（R16-H1 修复后将随返回文本返回）当前不会
  显示在 GUI；且 gpui 纯文本不渲染 `\n`，多行文本需按行拆 div。

## 三、改进建议（按优先级）

### P1（R16-H1）：hooks 诊断改为随返回值走

- **做法**：`install()`/`uninstall()` 内部收集 `Vec<String>` 诊断（原 4 处
  `eprintln!` 改 `diagnostics.push(...)`），追加到返回摘要文本尾部（每行
  `[hooks] ...` 前缀不变）；更新两函数 doc 注释。CLI 行为微变：诊断从
  stderr 改为 stdout 摘要尾部（人读场景，无脚本依赖）。
- **文件**：`crates/core/src/hooks.rs`。
- **验证**：`cargo test -p orphan-audit-core`（hooks 6 测试不涉 eprintln 路径，
  应全绿）；`grep -rn eprintln crates/core/src` 应零命中；实机 CLI
  `hooks uninstall` 输出格式复核。

### P2（R16-L2）：GUI 钩子反馈多行渲染

- **做法**：`hooks_panel` 的 feedback 由单 div 改为按 `\n` 拆行渲染；
  install/uninstall 成功回调把返回文本中的 `[hooks]` 诊断行并入反馈。
- **文件**：`gui/src/main.rs`（hooks_panel、install_hooks/uninstall_hooks）。
- **验证**：`cargo test -p orphan-audit-gui`；GUI 冒烟（用户实测安装按钮）。

### P3（R16-M1）：README 措辞刷新

- **做法**：「GUI 警告横幅下有…」→「GUI 有『卸载残骸』操作条…」；顺带把
  审计警告的呈现位置说明改为右栏「信号与诊断」。
- **文件**：`README.md`。
- **验证**：通读该节与代码行为一致。

## 四、并行性

- P1（core）与 P3（README）可并行；P2 依赖 P1 的返回值格式，需串行在后。

## 五、复核记录（不修项）

- core 六种警告 tag 与 GUI 映射全对上，无漏配。
- `drift_diff`、`shim_removal_commands`（PS 单引号字面量，Windows 路径不含
  `'`，安全）、`write_script_file`（BOM）单测覆盖。
- GUI remove_shims/export_script/run_drift/refresh_hooks 全部后台执行 +
  busy 门控；确认态/代次守卫与 R6 加固点一致。
- `default_script_export_path` 桌面回退链（desktop→data→"."）合理。

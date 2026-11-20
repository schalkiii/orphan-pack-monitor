"""orphan-audit import 钩子（python 侧宿主）。

由 `orphan-audit hooks install` 部署的 .pth 引用加载：每当解释器首次 import
一个顶层模块时，向 %APPDATA%\\orphan-audit\\imports\\events.jsonl 追加一行
JSON 事件（模块名 + 时间戳）。core 的审计把模块名经 site-packages 的
top_level.txt 映射回发行包名，作为 Import 使用证据。

设计红线：本文件运行在**一切 python 进程内部**（含用户业务脚本），因此：
- 任何异常都必须吞掉（裸 try/except），绝不能影响解释器启动或业务 import；
- 零第三方依赖、单次 find_spec 开销一次 set 查询；
- 每个进程对同一顶层模块只记一次（sys.modules 缓存本来就保证 find_spec
  只在真实首次加载时被调）。
"""

import sys

try:
    import json
    import os

    _LOG_DIR = os.path.join(
        os.environ.get("APPDATA", os.path.expanduser("~")), "orphan-audit", "imports"
    )
    # OA_IMPORT_LOG 可整体重定向日志路径（测试隔离 / 避开被管控的目录）
    _LOG = os.environ.get("OA_IMPORT_LOG") or os.path.join(_LOG_DIR, "events.jsonl")
    _SELF = "oa_import_hook"  # 不记录钩子自身

    def _record(top):
        try:
            os.makedirs(_LOG_DIR, exist_ok=True)
            line = json.dumps({"kind": "py", "module": top, "ts": _utcnow()})
            with open(_LOG, "a", encoding="utf-8") as f:
                f.write(line + "\n")
        except Exception:
            pass

    def _utcnow():
        import datetime

        return datetime.datetime.now(datetime.timezone.utc).strftime(
            "%Y-%m-%dT%H:%M:%S.%f"
        )[:-3] + "Z"

    _seen = set()  # 每进程每顶层模块只记一次（importlib.util.find_spec 等查找
    # 会重复触发 find_spec，不去重会把「查一下」也计成使用）

    class _OaImportFinder:
        """只观察、不干预：find_spec 恒返回 None，import 流程照常走后续 finder。"""

        def find_spec(self, fullname, path=None, target=None):
            try:
                top = fullname.split(".")[0]
                if top not in _seen:
                    _seen.add(top)
                    if top != _SELF and "." not in fullname:
                        _record(top)
            except Exception:
                pass
            return None

    if "oa_import_hook_loaded" not in sys.modules:
        sys.modules["oa_import_hook_loaded"] = True  # 进程级防重复安装
        sys.meta_path.insert(0, _OaImportFinder())
except Exception:
    pass

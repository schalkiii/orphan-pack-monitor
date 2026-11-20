/*!
 * orphan-audit import 钩子（node 侧宿主）。
 *
 * 由 `orphan-audit hooks install` 写入用户级 NODE_OPTIONS（--require 本文件）：
 * 每当 node 进程加载 node_modules 下的顶层包时，向
 * %APPDATA%\orphan-audit\imports\events.jsonl 追加一行 JSON 事件。
 * core 审计把包目录名与已审计的全局包对上，作为 Import 使用证据——
 * 覆盖 IDE / agent 直接 `node xxx.js` 调用包内脚本时的 require/import 链。
 *
 * 设计红线（与 python 侧一致）：运行在一切 node 进程内部，任何异常必须吞掉；
 * 同一进程同一包只记一次；零依赖。
 */
(function () {
  try {
    var fs = require('node:fs');
    var path = require('node:path');
    var LOG =
      process.env.OA_IMPORT_LOG ||
      path.join(
        process.env.APPDATA || require('node:os').homedir(),
        'orphan-audit',
        'imports',
        'events.jsonl'
      );
    var seen = new Set();

    function record(name) {
      if (!name || seen.has(name)) return;
      seen.add(name);
      try {
        fs.mkdirSync(path.dirname(LOG), { recursive: true });
        fs.appendFileSync(
          LOG,
          JSON.stringify({ kind: 'js', module: name, ts: new Date().toISOString() }) + '\n'
        );
      } catch (e) {
        /* 采集绝不影响业务 */
      }
    }

    // 从解析结果路径提取包名：第一段 node_modules 的下一段（scoped 取两段）
    function pkgFromPath(p) {
      if (!p) return null;
      var norm = String(p).replace(/\\/g, '/');
      var i = norm.lastIndexOf('node_modules/');
      if (i < 0) return null;
      var rest = norm.slice(i + 'node_modules/'.length);
      if (rest.charAt(0) === '@') {
        var parts = rest.split('/');
        return parts.length >= 2 ? parts[0] + '/' + parts[1] : null;
      }
      return rest.split('/')[0] || null;
    }

    var mod = require('node:module');

    // ESM + CJS 统一入口（node >= 22.15 / 23.5 的同步钩子；仅裸说明符需要记录，
    // 相对路径的解析结果由下面的 _load 补丁兜底）
    if (typeof mod.registerHooks === 'function') {
      mod.registerHooks({
        resolve: function (specifier, context, nextResolve) {
          try {
            var isBare =
              specifier &&
              !specifier.startsWith('.') &&
              !specifier.startsWith('/') &&
              !specifier.startsWith('node:') &&
              !/^[a-zA-Z]:[\\/]/.test(specifier); // 盘符路径不是包名
            if (isBare) {
              var name =
                specifier.charAt(0) === '@'
                  ? specifier.split('/').slice(0, 2).join('/')
                  : specifier.split('/')[0];
              record(name);
            }
          } catch (e) {
            /* 忽略 */
          }
          return nextResolve(specifier, context);
        },
      });
    }

    // CJS 兜底：按已解析路径归名（覆盖 require('pkg/sub') 与相对路径进包的情况）
    var ModuleCtor = mod.Module || mod;
    if (ModuleCtor && typeof ModuleCtor._load === 'function') {
      var origLoad = ModuleCtor._load;
      ModuleCtor._load = function (request, parent, isMain) {
        try {
          var resolved = ModuleCtor._resolveFilename(request, parent);
          record(pkgFromPath(resolved));
        } catch (e) {
          /* 忽略 */
        }
        return origLoad.apply(this, arguments);
      };
    }
  } catch (e) {
    /* 钩子永不抛错 */
  }
})();

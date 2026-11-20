//! 存储层：SQLite 持久化包清单、事件、判定与历史快照。
//! 保留快照是为了支持「漂移对比」——diff 本月与上月的 active 集合。

use crate::model::{Confidence, Manager, Package, SignalSource, Verdict, VerdictResult};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

pub struct Store {
    conn: Connection,
}

/// 一档的快照对比结果（drift_diff 返回）。
#[derive(Debug, Clone, PartialEq)]
pub struct DriftDiff {
    pub verdict: Verdict,
    /// 新快照有、旧快照没有（新增）。
    pub added: Vec<String>,
    /// 旧快照有、新快照没有（移除）。
    pub removed: Vec<String>,
}

impl Store {
    /// 打开（或创建）数据库。使用 bundled SQLite，无需系统库。
    pub fn open(path: &std::path::Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// 内存数据库（用于测试）。
    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS packages (
                name TEXT NOT NULL,
                manager TEXT NOT NULL,
                version TEXT,
                binaries TEXT NOT NULL,
                install_path TEXT,
                installed_at TEXT,
                size_bytes INTEGER,
                confidence TEXT NOT NULL,
                is_dependency INTEGER NOT NULL DEFAULT 0,
                install_key TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (name, manager, install_key)
            );
            CREATE TABLE IF NOT EXISTS usage_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                binary TEXT NOT NULL,
                occurred_at TEXT NOT NULL,
                source TEXT NOT NULL,
                command_line TEXT,
                parent TEXT,
                count INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_events_binary ON usage_events(binary);
            CREATE INDEX IF NOT EXISTS idx_events_time ON usage_events(occurred_at);
            CREATE TABLE IF NOT EXISTS verdicts (
                snapshot_id TEXT NOT NULL,
                name TEXT NOT NULL,
                manager TEXT NOT NULL,
                install_key TEXT NOT NULL DEFAULT '',
                verdict TEXT NOT NULL,
                interactive_hits INTEGER NOT NULL,
                last_exec_days_ago INTEGER,
                in_shell_config INTEGER NOT NULL,
                evidence TEXT NOT NULL,
                computed_at TEXT NOT NULL,
                PRIMARY KEY (snapshot_id, name, manager, install_key)
            );
            "#,
        )?;
        self.migrate_legacy_multi_install()?;
        Ok(())
    }

    /// 0.2.0 及更早的库以 (name, manager) / (snapshot_id, name, manager) 为主键——
    /// 多 prefix / 多 python 环境检测上线后，同名包可合法存在多份（实测
    /// @jackwener/opencli 同时装在默认 prefix 与 QClaw prefix），旧主键下第二条
    /// INSERT 即违反 UNIQUE、整个 replace_inventory 事务回滚。检测到旧 schema
    /// （packages 表无 install_key 列）时重建两表；历史 verdicts 无路径信息，
    /// install_key 统一置 ''（streak 查询不按 install_key 匹配，混用无害）。
    fn migrate_legacy_multi_install(&self) -> Result<()> {
        if self.has_column("packages", "install_key")? {
            return Ok(()); // 已是新 schema（packages 与 verdicts 同批迁移）
        }
        self.conn.execute_batch(
            r#"
            BEGIN;
            CREATE TABLE packages_new (
                name TEXT NOT NULL,
                manager TEXT NOT NULL,
                version TEXT,
                binaries TEXT NOT NULL,
                install_path TEXT,
                installed_at TEXT,
                size_bytes INTEGER,
                confidence TEXT NOT NULL,
                is_dependency INTEGER NOT NULL DEFAULT 0,
                install_key TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (name, manager, install_key)
            );
            INSERT INTO packages_new
                SELECT name, manager, version, binaries, install_path, installed_at,
                       size_bytes, confidence, is_dependency, IFNULL(install_path, '')
                FROM packages;
            DROP TABLE packages;
            ALTER TABLE packages_new RENAME TO packages;

            CREATE TABLE verdicts_new (
                snapshot_id TEXT NOT NULL,
                name TEXT NOT NULL,
                manager TEXT NOT NULL,
                install_key TEXT NOT NULL DEFAULT '',
                verdict TEXT NOT NULL,
                interactive_hits INTEGER NOT NULL,
                last_exec_days_ago INTEGER,
                in_shell_config INTEGER NOT NULL,
                evidence TEXT NOT NULL,
                computed_at TEXT NOT NULL,
                PRIMARY KEY (snapshot_id, name, manager, install_key)
            );
            INSERT INTO verdicts_new
                SELECT snapshot_id, name, manager, '', verdict, interactive_hits,
                       last_exec_days_ago, in_shell_config, evidence, computed_at
                FROM verdicts;
            DROP TABLE verdicts;
            ALTER TABLE verdicts_new RENAME TO verdicts;
            COMMIT;
            "#,
        )?;
        Ok(())
    }

    /// `table` 是否已有 `column` 列（schema 版本探测）。
    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .flatten()
            .collect();
        Ok(names.iter().any(|c| c.as_str() == column))
    }

    /// 用一次审计结果替换 packages / usage_events 表（全量刷新）。
    pub fn replace_inventory(&self, packages: &[Package], events: &[UsageEventRow]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM packages", [])?;
        tx.execute("DELETE FROM usage_events", [])?;
        {
            let mut stmt = tx.prepare(
                r#"INSERT INTO packages
                   (name, manager, version, binaries, install_path, installed_at,
                    size_bytes, confidence, is_dependency, install_key)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"#,
            )?;
            for p in packages {
                stmt.execute(params![
                    p.name,
                    p.manager.as_str(),
                    p.version,
                    serde_json::to_string(&p.binaries)?,
                    p.install_path
                        .as_ref()
                        .map(|x| x.to_string_lossy().into_owned()),
                    p.installed_at.map(|t| t.to_rfc3339()),
                    p.size_bytes.map(|x| x as i64),
                    conf_str(p.confidence),
                    p.is_dependency as i32,
                    install_key(p),
                ])?;
            }
        }
        {
            let mut stmt = tx.prepare(
                r#"INSERT INTO usage_events
                   (binary, occurred_at, source, command_line, parent, count)
                   VALUES (?1,?2,?3,?4,?5,?6)"#,
            )?;
            for e in events {
                stmt.execute(params![
                    e.binary,
                    e.occurred_at.to_rfc3339(),
                    e.source.as_str(),
                    e.command_line,
                    e.parent,
                    e.count,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 保存一次判定快照。snapshot_id 建议为 YYYY-MM。
    pub fn save_snapshot(&self, snapshot_id: &str, results: &[VerdictResult]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let now = Utc::now().to_rfc3339();
        {
            let mut stmt = tx.prepare(
                r#"INSERT OR REPLACE INTO verdicts
                   (snapshot_id, name, manager, install_key, verdict, interactive_hits,
                    last_exec_days_ago, in_shell_config, evidence, computed_at)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"#,
            )?;
            for r in results {
                stmt.execute(params![
                    snapshot_id,
                    r.package.name,
                    r.package.manager.as_str(),
                    install_key(&r.package),
                    r.verdict.as_str(),
                    r.interactive_hits as i64,
                    r.last_exec_days_ago.map(|x| x as i64),
                    r.in_shell_config as i32,
                    serde_json::to_string(&r.evidence)?,
                    now,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 读取某快照中处于指定判定档位的包名集合（用于漂移对比）。
    pub fn verdict_names(&self, snapshot_id: &str, verdict: Verdict) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM verdicts WHERE snapshot_id=?1 AND verdict=?2 ORDER BY name",
        )?;
        let rows = stmt.query_map(params![snapshot_id, verdict.as_str()], |r| {
            r.get::<_, String>(0)
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// 列出全部快照 ID。
    pub fn snapshot_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT snapshot_id FROM verdicts ORDER BY snapshot_id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// 快照漂移对比（CLI drift 子命令与 GUI「长期对比」面板共用同一实现，
    /// 保证两侧结果一字不差）。只对比 active/unused 两档——中间档随窗口参数
    /// 波动，列入会淹没真实增删（与 CLI 帮助文案同一理由）。
    pub fn drift_diff(&self, old: &str, new: &str) -> Result<Vec<DriftDiff>> {
        let mut out = Vec::new();
        for verdict in [Verdict::Active, Verdict::Unused] {
            let a: std::collections::BTreeSet<String> =
                self.verdict_names(old, verdict)?.into_iter().collect();
            let b: std::collections::BTreeSet<String> =
                self.verdict_names(new, verdict)?.into_iter().collect();
            out.push(DriftDiff {
                verdict,
                added: b.difference(&a).cloned().collect(),
                removed: a.difference(&b).cloned().collect(),
            });
        }
        Ok(out)
    }

    /// 某包在 exclude_snapshot 之前**连续**判 unused 的**月**数（漂移即信号：
    /// 连续多月 unused 比单月 unused 更接近可删；快照 ID 为 YYYY-MM 字符串，
    /// 字典序即时间序，只统计严格更早的快照，从最新往回数到首个非 unused 为止）。
    /// GROUP BY snapshot_id：同名包多实例（多 prefix / 多环境）时按月计数不按行，
    /// 且每月取 MIN(verdict)（字母序最小 = 使用最强的档）——任一副本在用即断档，
    /// 保守语义防止把「一处还在用」的包误标为长期没人用。单副本数据行为不变。
    pub fn unused_streak_months(
        &self,
        name: &str,
        manager: &str,
        exclude_snapshot: &str,
    ) -> Result<u32> {
        let mut stmt = self.conn.prepare(
            "SELECT MIN(verdict) FROM verdicts
             WHERE name=?1 AND manager=?2 AND snapshot_id<?3
             GROUP BY snapshot_id
             ORDER BY snapshot_id DESC",
        )?;
        let rows = stmt.query_map(params![name, manager, exclude_snapshot], |r| {
            r.get::<_, String>(0)
        })?;
        let mut streak = 0u32;
        for verdict in rows.flatten() {
            if verdict == "unused" {
                streak += 1;
            } else {
                break;
            }
        }
        Ok(streak)
    }

    /// 读取全部使用事件（离线模式重建判定用；对应 replace_inventory 写入的行）。
    pub fn events(&self) -> Result<Vec<crate::model::UsageEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT binary, occurred_at, source, command_line, parent, count FROM usage_events",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        let mut events = Vec::new();
        for (binary, occurred_at, source, command_line, parent, count) in rows.flatten() {
            // 未知来源（老库/手改数据）跳过该行，不影响其余事件
            let Some(source) = parse_source(&source) else {
                continue;
            };
            let timestamp = DateTime::parse_from_rfc3339(&occurred_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let mut ev = crate::model::UsageEvent::new(binary, timestamp, source);
            ev.command_line = command_line;
            ev.parent = parent;
            ev.count = count.max(1) as u32;
            events.push(ev);
        }
        Ok(events)
    }

    /// 读取包清单（用于 GUI/报告展示）。
    pub fn packages(&self) -> Result<Vec<Package>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, manager, version, binaries, install_path, installed_at,
                    size_bytes, confidence, is_dependency FROM packages ORDER BY manager, name",
        )?;
        let rows = stmt.query_map([], |r| {
            let manager = parse_manager(&r.get::<_, String>(1)?)?;
            let binaries: Vec<String> =
                serde_json::from_str(&r.get::<_, String>(3)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
            let mut p = Package::new(r.get::<_, String>(0)?, manager);
            p.version = r.get(2)?;
            p.install_path = r.get::<_, Option<String>>(4)?.map(std::path::PathBuf::from);
            p.installed_at = r.get::<_, Option<String>>(5)?.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            });
            p.size_bytes = r.get::<_, Option<i64>>(6)?.map(|x| x as u64);
            p.confidence = parse_confidence(&r.get::<_, String>(7)?)?;
            p.is_dependency = r.get::<_, i32>(8)? != 0;
            p.binaries = binaries;
            Ok(p)
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

/// 同名包多实例的判别键：多 prefix / 多 python 环境下 (name, manager) 不再唯一
/// （同一包可装在默认 prefix 与 QClaw prefix 两处），install_path 的字符串形式
/// 作为主键组成部分；采集器未提供路径的包统一空串。
fn install_key(p: &Package) -> String {
    p.install_path
        .as_ref()
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 存储层的事件行（与 model::UsageEvent 分开，避免持久化结构污染领域模型）。
#[derive(Debug, Clone)]
pub struct UsageEventRow {
    pub binary: String,
    pub occurred_at: DateTime<Utc>,
    pub source: SignalSource,
    pub command_line: Option<String>,
    pub parent: Option<String>,
    pub count: u32,
}

impl From<&crate::model::UsageEvent> for UsageEventRow {
    fn from(e: &crate::model::UsageEvent) -> Self {
        Self {
            binary: e.binary.clone(),
            occurred_at: e.timestamp,
            source: e.source,
            command_line: e.command_line.clone(),
            parent: e.parent.clone(),
            count: e.count,
        }
    }
}

fn conf_str(c: Confidence) -> &'static str {
    match c {
        Confidence::Exact => "exact",
        Confidence::Inferred => "inferred",
        Confidence::None => "none",
    }
}

fn parse_confidence(s: &str) -> rusqlite::Result<Confidence> {
    match s {
        "exact" => Ok(Confidence::Exact),
        "inferred" => Ok(Confidence::Inferred),
        "none" => Ok(Confidence::None),
        other => Err(rusqlite::Error::InvalidColumnName(format!(
            "confidence={other}"
        ))),
    }
}

fn parse_manager(s: &str) -> rusqlite::Result<Manager> {
    match s {
        "cargo" => Ok(Manager::Cargo),
        "npm" => Ok(Manager::Npm),
        "pip" => Ok(Manager::Pip),
        "uv" => Ok(Manager::Uv),
        "pipx" => Ok(Manager::Pipx),
        "go" => Ok(Manager::Go),
        "brew" => Ok(Manager::Brew),
        "scoop" => Ok(Manager::Scoop),
        "winget" => Ok(Manager::Winget),
        other => Err(rusqlite::Error::InvalidColumnName(format!(
            "manager={other}"
        ))),
    }
}

/// 事件来源字符串还原（写入侧见 SignalSource::as_str）。
fn parse_source(s: &str) -> Option<SignalSource> {
    match s {
        "history" => Some(SignalSource::History),
        "etw" => Some(SignalSource::Etw),
        "import" => Some(SignalSource::Import),
        "atime" => Some(SignalSource::Atime),
        "shell-config" => Some(SignalSource::ShellConfig),
        "alias" => Some(SignalSource::Alias),
        "wrapped" => Some(SignalSource::Wrapped),
        "drift" => Some(SignalSource::Drift),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrip_snapshot_and_drift() {
        let store = Store::in_memory().unwrap();
        let mut p = Package::new("dusty", Manager::Cargo);
        p.binaries = vec!["dusty".into()];
        store.replace_inventory(&[p.clone()], &[]).unwrap();

        let mut r = VerdictResult {
            package: p,
            verdict: Verdict::Unused,
            interactive_hits: 0,
            last_exec_days_ago: None,
            in_shell_config: false,
            newly_installed: false,
            evidence: vec![],
        };
        store
            .save_snapshot("2026-11", std::slice::from_ref(&r))
            .unwrap();
        assert_eq!(
            store.verdict_names("2026-11", Verdict::Unused).unwrap(),
            vec!["dusty"]
        );

        r.verdict = Verdict::Active;
        store
            .save_snapshot("2026-12", std::slice::from_ref(&r))
            .unwrap();
        assert!(store
            .verdict_names("2026-12", Verdict::Unused)
            .unwrap()
            .is_empty());
        assert_eq!(store.snapshot_ids().unwrap(), vec!["2026-11", "2026-12"]);
        assert_eq!(store.packages().unwrap().len(), 1);
    }

    #[test]
    fn drift_diff_reports_added_removed_and_skips_middle_tiers() {
        let store = Store::in_memory().unwrap();
        let mk = |name: &str, v: Verdict| {
            let mut p = Package::new(name, Manager::Cargo);
            p.binaries = vec![name.to_string()];
            VerdictResult {
                package: p,
                verdict: v,
                interactive_hits: 0,
                last_exec_days_ago: None,
                in_shell_config: false,
                newly_installed: false,
                evidence: vec![],
            }
        };
        // 旧快照：dusty active、ghost unused、middle rare
        store
            .save_snapshot(
                "2026-11",
                &[
                    mk("dusty", Verdict::Active),
                    mk("ghost", Verdict::Unused),
                    mk("middle", Verdict::Rare),
                ],
            )
            .unwrap();
        // 新快照：dusty 降档（removed@active + added@unused）、ghost 卸载、
        // fresh 新装；middle 换成另一中间档——两档都不得出现在对比里
        store
            .save_snapshot(
                "2026-12",
                &[
                    mk("dusty", Verdict::Unused),
                    mk("fresh", Verdict::Unused),
                    mk("middle", Verdict::Integration),
                ],
            )
            .unwrap();
        let diffs = store.drift_diff("2026-11", "2026-12").unwrap();
        assert_eq!(diffs.len(), 2, "只对比 active/unused 两档");
        assert_eq!(diffs[0].verdict, Verdict::Active);
        assert!(diffs[0].added.is_empty());
        assert_eq!(diffs[0].removed, vec!["dusty"]);
        assert_eq!(diffs[1].verdict, Verdict::Unused);
        assert_eq!(diffs[1].added, vec!["dusty", "fresh"]);
        assert_eq!(diffs[1].removed, vec!["ghost"]);
    }

    #[test]
    fn unused_streak_counts_consecutive_snapshots_only() {
        let store = Store::in_memory().unwrap();
        let p = Package::new("dusty", Manager::Cargo);
        let mk = |v: Verdict| VerdictResult {
            package: p.clone(),
            verdict: v,
            interactive_hits: 0,
            last_exec_days_ago: None,
            in_shell_config: false,
            newly_installed: false,
            evidence: vec![],
        };
        // 2026-06 active，2026-07/08/09 连续 unused
        store
            .save_snapshot("2026-06", &[mk(Verdict::Active)])
            .unwrap();
        store
            .save_snapshot("2026-07", &[mk(Verdict::Unused)])
            .unwrap();
        store
            .save_snapshot("2026-08", &[mk(Verdict::Unused)])
            .unwrap();
        store
            .save_snapshot("2026-09", &[mk(Verdict::Unused)])
            .unwrap();
        // 截至当前快照 2026-10：连续 3 个 unused
        assert_eq!(
            store
                .unused_streak_months("dusty", "cargo", "2026-10")
                .unwrap(),
            3
        );
        // 当前快照被排除：查 2026-09 时只数 07/08 两个月
        assert_eq!(
            store
                .unused_streak_months("dusty", "cargo", "2026-09")
                .unwrap(),
            2
        );
        // 首个非 unused（06 active）截断计数
        assert_eq!(
            store
                .unused_streak_months("dusty", "cargo", "2026-07")
                .unwrap(),
            0
        );
        // 无历史记录
        assert_eq!(
            store
                .unused_streak_months("ghost", "cargo", "2026-10")
                .unwrap(),
            0
        );
    }

    #[test]
    fn events_roundtrip_after_replace() {
        let store = Store::in_memory().unwrap();
        let mut p = Package::new("dusty", Manager::Cargo);
        p.binaries = vec!["dusty".into()];

        let now = Utc::now();
        let mut ev = crate::model::UsageEvent::new("dusty", now, SignalSource::History);
        ev.command_line = Some("dusty run".into());
        ev.count = 5;
        let rows = vec![UsageEventRow::from(&ev)];
        store.replace_inventory(&[p], &rows).unwrap();

        let events = store.events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].binary, "dusty");
        assert_eq!(events[0].source, SignalSource::History);
        assert_eq!(events[0].count, 5);
        assert_eq!(events[0].command_line.as_deref(), Some("dusty run"));
    }

    #[test]
    fn duplicate_packages_across_prefixes_persist() {
        // 多 prefix 下同名包两份（不同 install_path，实测 @jackwener/opencli 布局）：
        // 旧主键 (name, manager) 下第二条 INSERT 违反 UNIQUE、整个事务回滚
        let store = Store::in_memory().unwrap();
        let mk = |path: &str| {
            let mut p = Package::new("@scope/tool", Manager::Npm);
            p.binaries = vec!["tool".into()];
            p.install_path = Some(std::path::PathBuf::from(path));
            p
        };
        store
            .replace_inventory(
                &[
                    mk(r"C:\npm\node_modules\@scope\tool"),
                    mk(r"C:\qclaw\npm-global\node_modules\@scope\tool"),
                ],
                &[],
            )
            .unwrap();
        assert_eq!(store.packages().unwrap().len(), 2);
        // 路径均缺失的同名包两份（install_key 同为 ''）仍冲突：采集器保证
        // 同包目录唯一，此形态不应出现；断言固化该边界语义
        let mut no_path = Package::new("x", Manager::Pip);
        no_path.binaries = vec!["x".into()];
        assert!(store
            .replace_inventory(&[no_path.clone(), no_path], &[])
            .is_err());
    }

    #[test]
    fn snapshot_keeps_instances_and_streak_counts_months() {
        let store = Store::in_memory().unwrap();
        let mk = |path: &str, v: Verdict| {
            let mut p = Package::new("tool", Manager::Npm);
            p.install_path = Some(std::path::PathBuf::from(path));
            VerdictResult {
                package: p,
                verdict: v,
                interactive_hits: 0,
                last_exec_days_ago: None,
                in_shell_config: false,
                newly_installed: false,
                evidence: vec![],
            }
        };
        // 2026-06：两份副本均 unused
        store
            .save_snapshot(
                "2026-06",
                &[mk(r"C:\a", Verdict::Unused), mk(r"C:\b", Verdict::Unused)],
            )
            .unwrap();
        // 两份副本都保留（旧主键 INSERT OR REPLACE 会静默合并成一份）
        assert_eq!(
            store
                .verdict_names("2026-06", Verdict::Unused)
                .unwrap()
                .len(),
            2
        );
        // 2026-07：一份 active 一份 unused → 该月 MIN=active，streak 断档
        store
            .save_snapshot(
                "2026-07",
                &[mk(r"C:\a", Verdict::Active), mk(r"C:\b", Verdict::Unused)],
            )
            .unwrap();
        assert_eq!(
            store
                .unused_streak_months("tool", "npm", "2026-08")
                .unwrap(),
            0
        );
        // 07 重写为全 unused → 两个月均计入（按月不按行：2 而非 4）
        store
            .save_snapshot(
                "2026-07",
                &[mk(r"C:\a", Verdict::Unused), mk(r"C:\b", Verdict::Unused)],
            )
            .unwrap();
        assert_eq!(
            store
                .unused_streak_months("tool", "npm", "2026-08")
                .unwrap(),
            2
        );
    }

    #[test]
    fn legacy_schema_migrates_with_data() {
        let dir = std::env::temp_dir().join(format!("oa_mig_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("legacy.db");
        {
            // 手工构造 0.2.0 及更早的 schema（无 install_key 列）+ 一行真实数据
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE packages (
                    name TEXT NOT NULL, manager TEXT NOT NULL, version TEXT,
                    binaries TEXT NOT NULL, install_path TEXT, installed_at TEXT,
                    size_bytes INTEGER, confidence TEXT NOT NULL,
                    is_dependency INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (name, manager)
                );
                CREATE TABLE verdicts (
                    snapshot_id TEXT NOT NULL, name TEXT NOT NULL, manager TEXT NOT NULL,
                    verdict TEXT NOT NULL, interactive_hits INTEGER NOT NULL,
                    last_exec_days_ago INTEGER, in_shell_config INTEGER NOT NULL,
                    evidence TEXT NOT NULL, computed_at TEXT NOT NULL,
                    PRIMARY KEY (snapshot_id, name, manager)
                );
                INSERT INTO packages VALUES
                    ('dusty','cargo','0.1','["dusty"]','C:\b','2026-01-01T00:00:00+00:00',NULL,'exact',0);
                INSERT INTO verdicts VALUES
                    ('2026-01','dusty','cargo','unused',0,NULL,0,'[]','2026-01-01T00:00:00+00:00');
                "#,
            )
            .unwrap();
        }
        // 打开即迁移：数据保留、快照可读
        let store = Store::open(&db).unwrap();
        let pkgs = store.packages().unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].name, "dusty");
        assert_eq!(
            pkgs[0].install_path.as_deref(),
            Some(std::path::Path::new(r"C:\b"))
        );
        assert_eq!(
            store.verdict_names("2026-01", Verdict::Unused).unwrap(),
            vec!["dusty"]
        );
        // 迁移后可写入旧主键下会冲突的多实例
        let mut a = Package::new("tool", Manager::Npm);
        a.install_path = Some(std::path::PathBuf::from(r"C:\a"));
        let mut b = Package::new("tool", Manager::Npm);
        b.install_path = Some(std::path::PathBuf::from(r"C:\b"));
        store.replace_inventory(&[a, b], &[]).unwrap();
        assert_eq!(store.packages().unwrap().len(), 2);
        drop(store); // 释放 SQLite 连接后才能删目录（Windows WAL 占用）
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

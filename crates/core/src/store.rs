//! 存储层：SQLite 持久化包清单、事件、判定与历史快照。
//! 保留快照是为了支持「漂移对比」——diff 本月与上月的 active 集合。

use crate::model::{Confidence, Manager, Package, SignalSource, Verdict, VerdictResult};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

pub struct Store {
    conn: Connection,
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
                PRIMARY KEY (name, manager)
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
                verdict TEXT NOT NULL,
                interactive_hits INTEGER NOT NULL,
                last_exec_days_ago INTEGER,
                in_shell_config INTEGER NOT NULL,
                evidence TEXT NOT NULL,
                computed_at TEXT NOT NULL,
                PRIMARY KEY (snapshot_id, name, manager)
            );
            "#,
        )?;
        Ok(())
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
                    size_bytes, confidence, is_dependency)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)"#,
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
                   (snapshot_id, name, manager, verdict, interactive_hits,
                    last_exec_days_ago, in_shell_config, evidence, computed_at)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)"#,
            )?;
            for r in results {
                stmt.execute(params![
                    snapshot_id,
                    r.package.name,
                    r.package.manager.as_str(),
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

    /// 某包在 exclude_snapshot 之前**连续**判 unused 的快照数（漂移即信号：
    /// 连续多月 unused 比单月 unused 更接近可删；快照 ID 为 YYYY-MM 字符串，
    /// 字典序即时间序，只统计严格更早的快照，从最新往回数到首个非 unused 为止）。
    pub fn unused_streak_months(
        &self,
        name: &str,
        manager: &str,
        exclude_snapshot: &str,
    ) -> Result<u32> {
        let mut stmt = self.conn.prepare(
            "SELECT verdict FROM verdicts
             WHERE name=?1 AND manager=?2 AND snapshot_id<?3
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
}

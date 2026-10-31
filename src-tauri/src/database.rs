use crate::model::{Audit, Server};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::{fs, path::Path, time::Duration};

pub const DATABASE_FILE: &str = "remote-desk.sqlite3";
const SERVER_COLUMNS: &str = "id,name,host,port,username,auth,key_path,fingerprint,group_name,color,enabled,allow_exec,allow_write,allow_terminal,root,mcp_enabled,mcp_permissions_json";
const AUDIT_COLUMNS: &str =
    "id,time,server_id,server_name,source,kind,detail,success,duration_ms,summary,response_json";

pub struct Database {
    connection: Connection,
}
impl Database {
    pub fn settings(&self) -> Result<crate::model::RuntimeSettings> {
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM metadata WHERE key='runtime_settings'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let settings: crate::model::RuntimeSettings = json
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        settings.validate()?;
        Ok(settings)
    }
    pub fn save_settings(&self, settings: &crate::model::RuntimeSettings) -> Result<()> {
        settings.validate()?;
        self.connection.execute("INSERT INTO metadata(key,value) VALUES('runtime_settings',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(settings)?])?;
        Ok(())
    }
    pub fn open(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let database_path = dir.join(DATABASE_FILE);
        let has_json_files = ["servers.json", "audit.previous.jsonl", "audit.jsonl"]
            .iter()
            .any(|name| dir.join(name).exists());
        if !database_path.exists() && has_json_files {
            bail!("不再支持 JSON/JSONL 工作区，请先使用旧版应用迁移为 SQLite")
        }
        let mut connection = Connection::open(database_path).context("无法打开 SQLite 数据库")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 2 {
            bail!("数据库版本较新，请使用新版应用打开")
        }
        if version != 0 && version != 2 {
            bail!("不再支持此数据库版本，请先使用旧版应用升级至 SQLite 版本 2")
        }
        if version == 0 {
            if has_json_files {
                bail!("不再支持 JSON/JSONL 工作区，请先使用旧版应用迁移为 SQLite")
            }
            let tables: i64 = connection.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            if tables != 0 {
                bail!("数据库缺少版本信息，无法作为新工作区初始化")
            }
        }
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        let transaction = connection.transaction()?;
        transaction.execute_batch(include_str!("schema.sql"))?;
        transaction.pragma_update(None, "user_version", 2)?;
        transaction.commit().context("SQLite 初始化提交失败")?;
        Ok(Self { connection })
    }
    pub fn servers(&self) -> Result<Vec<Server>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {SERVER_COLUMNS} FROM servers ORDER BY position"
        ))?;
        let rows = statement.query_map([], server_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn save_server(&mut self, server: &Server) -> Result<()> {
        let transaction = self.connection.transaction()?;
        save_server(&transaction, server)?;
        transaction.commit()?;
        Ok(())
    }
    pub fn remove_server(&mut self, id: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM servers WHERE id=?1", [id])?;
        Ok(())
    }
    pub fn insert_audit(&mut self, audit: &Audit) -> Result<()> {
        insert_audit(&self.connection, audit)?;
        Ok(())
    }
    pub fn recent_audits(&self, limit: u32) -> Result<Vec<Audit>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {AUDIT_COLUMNS} FROM audit_entries ORDER BY sequence DESC LIMIT ?1"
        ))?;
        let mut audits = statement
            .query_map([limit], audit_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        audits.reverse();
        Ok(audits)
    }
    pub fn audit(&self, id: &str) -> Result<Audit> {
        self.connection
            .query_row(
                &format!("SELECT {AUDIT_COLUMNS} FROM audit_entries WHERE id=?1"),
                [id],
                audit_row,
            )
            .optional()?
            .context("操作记录不存在")
    }
}
fn server_row(row: &Row<'_>) -> rusqlite::Result<Server> {
    let permissions: Option<String> = row.get(16)?;
    let mcp_permissions = permissions
        .map(|json| {
            serde_json::from_str::<Vec<String>>(&json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    16,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    Ok(Server {
        id: row.get(0)?,
        name: row.get(1)?,
        host: row.get(2)?,
        port: row.get(3)?,
        username: row.get(4)?,
        auth: row.get(5)?,
        key_path: row.get(6)?,
        fingerprint: row.get(7)?,
        group: row.get(8)?,
        color: row.get(9)?,
        enabled: row.get(10)?,
        allow_exec: row.get(11)?,
        allow_write: row.get(12)?,
        allow_terminal: row.get(13)?,
        root: row.get(14)?,
        mcp_enabled: row.get(15)?,
        mcp_permissions,
    })
}
fn audit_row(row: &Row<'_>) -> rusqlite::Result<Audit> {
    let response: Option<String> = row.get(10)?;
    let response = response
        .map(|json| {
            serde_json::from_str(&json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    Ok(Audit {
        id: row.get(0)?,
        time: row.get(1)?,
        server_id: row.get(2)?,
        server_name: row.get(3)?,
        source: row.get(4)?,
        kind: row.get(5)?,
        detail: row.get(6)?,
        success: row.get(7)?,
        duration_ms: row.get::<_, i64>(8)? as u64,
        summary: row.get(9)?,
        response,
    })
}
fn save_server(connection: &Connection, s: &Server) -> Result<()> {
    let permissions = s
        .mcp_permissions
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    connection.execute("INSERT INTO servers(id,name,host,port,username,auth,key_path,fingerprint,group_name,color,enabled,allow_exec,allow_write,allow_terminal,root,mcp_enabled,mcp_permissions_json)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
        ON CONFLICT(id) DO UPDATE SET name=excluded.name,host=excluded.host,port=excluded.port,username=excluded.username,auth=excluded.auth,key_path=excluded.key_path,fingerprint=excluded.fingerprint,group_name=excluded.group_name,color=excluded.color,enabled=excluded.enabled,allow_exec=excluded.allow_exec,allow_write=excluded.allow_write,allow_terminal=excluded.allow_terminal,root=excluded.root,mcp_enabled=excluded.mcp_enabled,mcp_permissions_json=excluded.mcp_permissions_json",
        params![s.id,s.name,s.host,s.port,s.username,s.auth,s.key_path,s.fingerprint,s.group,s.color,s.enabled,s.allow_exec,s.allow_write,s.allow_terminal,s.root,s.mcp_enabled,permissions])?;
    Ok(())
}
fn insert_audit(connection: &Connection, a: &Audit) -> Result<()> {
    let duration_ms = i64::try_from(a.duration_ms).context("操作耗时超出数据库整数范围")?;
    let response = a.response.as_ref().map(serde_json::to_string).transpose()?;
    connection.execute("INSERT INTO audit_entries(id,time,server_id,server_name,source,kind,detail,success,duration_ms,summary,response_json)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
        ON CONFLICT(id) DO UPDATE SET time=excluded.time,server_id=excluded.server_id,server_name=excluded.server_name,source=excluded.source,kind=excluded.kind,detail=excluded.detail,success=excluded.success,duration_ms=excluded.duration_ms,summary=excluded.summary,response_json=excluded.response_json",
        params![a.id,a.time,a.server_id,a.server_name,a.source,a.kind,a.detail,a.success,duration_ms,a.summary,response])?;
    Ok(())
}

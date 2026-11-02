CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS servers (
    position INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE, name TEXT NOT NULL, host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
    username TEXT NOT NULL, auth TEXT NOT NULL CHECK(auth IN ('password','key','agent')),
    key_path TEXT NOT NULL, fingerprint TEXT NOT NULL, group_name TEXT NOT NULL,
    color TEXT NOT NULL, enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    allow_exec INTEGER NOT NULL CHECK(allow_exec IN (0,1)),
    allow_write INTEGER NOT NULL CHECK(allow_write IN (0,1)),
    allow_terminal INTEGER NOT NULL CHECK(allow_terminal IN (0,1)), root TEXT NOT NULL,
    mcp_enabled INTEGER NOT NULL DEFAULT 1 CHECK(mcp_enabled IN (0,1)),
    mcp_permissions_json TEXT,
    login_path TEXT, login_path_synced_at TEXT
);
CREATE TABLE IF NOT EXISTS audit_entries (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
    time TEXT NOT NULL, server_id TEXT NOT NULL, server_name TEXT NOT NULL,
    source TEXT NOT NULL, kind TEXT NOT NULL, detail TEXT NOT NULL,
    success INTEGER NOT NULL CHECK(success IN (0,1)),
    duration_ms INTEGER NOT NULL CHECK(duration_ms >= 0), summary TEXT NOT NULL,
    response_json TEXT
);
CREATE INDEX IF NOT EXISTS audit_server_sequence ON audit_entries(server_id,sequence DESC);
CREATE INDEX IF NOT EXISTS audit_kind_sequence ON audit_entries(kind,sequence DESC);

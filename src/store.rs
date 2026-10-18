//! Persistencia en SQLite: conexiones guardadas, layout de pestañas y secretos cifrados.

use std::path::Path;

use anyhow::{bail, Context};
use rusqlite::{params, Connection as Db, OptionalExtension};

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE connections (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        host TEXT NOT NULL,
        port INTEGER NOT NULL DEFAULT 22,
        user TEXT NOT NULL,
        auth TEXT NOT NULL,
        key_path TEXT,
        init_command TEXT
    );
    CREATE TABLE layout_tabs (
        position INTEGER PRIMARY KEY,
        connection_id INTEGER REFERENCES connections(id) ON DELETE CASCADE
    );
    CREATE TABLE meta (key TEXT PRIMARY KEY, value BLOB NOT NULL);
    CREATE TABLE secrets (
        account TEXT PRIMARY KEY,
        nonce BLOB NOT NULL,
        ciphertext BLOB NOT NULL
    );",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    Password,
    Key,
    Agent,
}

impl Auth {
    fn as_str(self) -> &'static str {
        match self {
            Auth::Password => "password",
            Auth::Key => "key",
            Auth::Agent => "agent",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "password" => Ok(Auth::Password),
            "key" => Ok(Auth::Key),
            "agent" => Ok(Auth::Agent),
            other => bail!("método de autenticación desconocido: {other}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub id: Option<i64>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    pub key_path: Option<String>,
    pub init_command: Option<String>,
}

/// Qué abre cada pestaña al restaurar la sesión.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    Local,
    Connection(i64),
}

pub struct Store {
    db: Db,
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creando {}", parent.display()))?;
        }
        Self::init(Db::open(path).with_context(|| format!("abriendo {}", path.display()))?)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Db::open_in_memory()?)
    }

    pub fn default_path() -> anyhow::Result<std::path::PathBuf> {
        // El nombre del directorio y del archivo se mantienen para no perder datos existentes.
        let dirs = directories::ProjectDirs::from("dev", "omniterm", "omniterm")
            .context("no se pudo determinar el directorio de datos")?;
        Ok(dirs.data_dir().join("omniterm.db"))
    }

    fn init(db: Db) -> anyhow::Result<Self> {
        db.pragma_update(None, "foreign_keys", true)?;
        let mut store = Self { db };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> anyhow::Result<()> {
        let current = self.db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))? as usize;
        if current > MIGRATIONS.len() {
            bail!("la base de datos es más nueva que esta versión de omni-term");
        }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let tx = self.db.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (index + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn list_connections(&self) -> anyhow::Result<Vec<Connection>> {
        let mut statement = self.db.prepare(
            "SELECT id, name, host, port, user, auth, key_path, init_command
             FROM connections ORDER BY name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u16>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;
        rows.map(|row| {
            let (id, name, host, port, user, auth, key_path, init_command) = row?;
            Ok(Connection { id: Some(id), name, host, port, user, auth: Auth::parse(&auth)?, key_path, init_command })
        })
        .collect()
    }

    /// Inserta o actualiza y devuelve el id.
    pub fn save_connection(&self, connection: &Connection) -> anyhow::Result<i64> {
        match connection.id {
            Some(id) => {
                self.db.execute(
                    "UPDATE connections SET name=?1, host=?2, port=?3, user=?4, auth=?5, key_path=?6, init_command=?7
                     WHERE id=?8",
                    params![
                        connection.name,
                        connection.host,
                        connection.port,
                        connection.user,
                        connection.auth.as_str(),
                        connection.key_path,
                        connection.init_command,
                        id
                    ],
                )?;
                Ok(id)
            }
            None => {
                self.db.execute(
                    "INSERT INTO connections (name, host, port, user, auth, key_path, init_command)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        connection.name,
                        connection.host,
                        connection.port,
                        connection.user,
                        connection.auth.as_str(),
                        connection.key_path,
                        connection.init_command
                    ],
                )?;
                Ok(self.db.last_insert_rowid())
            }
        }
    }

    pub fn delete_connection(&self, id: i64) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM connections WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn save_layout(&mut self, tabs: &[TabKind], active: usize) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM layout_tabs", [])?;
        for (position, kind) in tabs.iter().enumerate() {
            let connection_id = match kind {
                TabKind::Local => None,
                TabKind::Connection(id) => Some(*id),
            };
            tx.execute(
                "INSERT INTO layout_tabs (position, connection_id) VALUES (?1, ?2)",
                params![position as i64, connection_id],
            )?;
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('active_tab', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [active.to_string().into_bytes()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Pestañas guardadas y la activa. Las conexiones que ya no existen se descartan.
    pub fn load_layout(&self) -> anyhow::Result<(Vec<TabKind>, usize)> {
        let mut statement = self.db.prepare(
            "SELECT t.connection_id, c.id FROM layout_tabs t
             LEFT JOIN connections c ON c.id = t.connection_id
             ORDER BY t.position",
        )?;
        let tabs = statement
            .query_map([], |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)))?
            .filter_map(|row| match row {
                Ok((None, _)) => Some(Ok(TabKind::Local)),
                Ok((Some(_), Some(id))) => Some(Ok(TabKind::Connection(id))),
                Ok((Some(_), None)) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let active: Option<Vec<u8>> =
            self.db.query_row("SELECT value FROM meta WHERE key='active_tab'", [], |row| row.get(0)).optional()?;
        let active = active
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| text.parse::<usize>().ok())
            .unwrap_or(0);
        Ok((tabs, active))
    }

    pub fn meta(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self.db.query_row("SELECT value FROM meta WHERE key=?1", [key], |row| row.get(0)).optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &[u8]) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn put_secret(&self, account: &str, nonce: &[u8], ciphertext: &[u8]) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT INTO secrets (account, nonce, ciphertext) VALUES (?1, ?2, ?3)
             ON CONFLICT(account) DO UPDATE SET nonce=excluded.nonce, ciphertext=excluded.ciphertext",
            params![account, nonce, ciphertext],
        )?;
        Ok(())
    }

    pub fn get_secret(&self, account: &str) -> anyhow::Result<Option<(Vec<u8>, Vec<u8>)>> {
        Ok(self
            .db
            .query_row("SELECT nonce, ciphertext FROM secrets WHERE account=?1", [account], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?)
    }

    pub fn delete_secret(&self, account: &str) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM secrets WHERE account=?1", [account])?;
        Ok(())
    }

    pub fn clear_secrets(&self) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM secrets", [])?;
        self.db.execute("DELETE FROM meta WHERE key IN ('kdf_salt', 'kdf_check')", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> Connection {
        Connection {
            id: None,
            name: name.into(),
            host: "example.com".into(),
            port: 22,
            user: "root".into(),
            auth: Auth::Key,
            key_path: Some("/home/u/.ssh/id_ed25519".into()),
            init_command: None,
        }
    }

    #[test]
    fn saves_updates_and_lists_connections_sorted() {
        let store = Store::open_in_memory().unwrap();
        let b = store.save_connection(&sample("beta")).unwrap();
        store.save_connection(&sample("Alpha")).unwrap();

        let mut updated = store.list_connections().unwrap().into_iter().find(|c| c.id == Some(b)).unwrap();
        updated.host = "other.test".into();
        updated.port = 2222;
        store.save_connection(&updated).unwrap();

        let all = store.list_connections().unwrap();
        assert_eq!(all.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Alpha", "beta"]);
        assert_eq!(all[1].host, "other.test");
        assert_eq!(all[1].port, 2222);
    }

    #[test]
    fn layout_round_trips_and_drops_deleted_connections() {
        let mut store = Store::open_in_memory().unwrap();
        let a = store.save_connection(&sample("a")).unwrap();
        let b = store.save_connection(&sample("b")).unwrap();
        store.save_layout(&[TabKind::Local, TabKind::Connection(a), TabKind::Connection(b)], 1).unwrap();

        assert_eq!(
            store.load_layout().unwrap(),
            (vec![TabKind::Local, TabKind::Connection(a), TabKind::Connection(b)], 1)
        );

        store.delete_connection(a).unwrap();
        assert_eq!(store.load_layout().unwrap().0, vec![TabKind::Local, TabKind::Connection(b)]);
    }

    #[test]
    fn empty_database_has_empty_layout() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.load_layout().unwrap(), (vec![], 0));
    }

    #[test]
    fn migrations_are_idempotent_on_reopen() {
        let dir = std::env::temp_dir().join(format!("omniterm-test-{}", std::process::id()));
        let path = dir.join("db.sqlite");
        {
            let store = Store::open(&path).unwrap();
            store.save_connection(&sample("kept")).unwrap();
        }
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.list_connections().unwrap().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn secrets_are_upserted_and_cleared() {
        let store = Store::open_in_memory().unwrap();
        store.put_secret("conn:1", b"n1", b"c1").unwrap();
        store.put_secret("conn:1", b"n2", b"c2").unwrap();
        assert_eq!(store.get_secret("conn:1").unwrap(), Some((b"n2".to_vec(), b"c2".to_vec())));
        store.set_meta("kdf_salt", b"salt").unwrap();
        store.clear_secrets().unwrap();
        assert_eq!(store.get_secret("conn:1").unwrap(), None);
        assert_eq!(store.meta("kdf_salt").unwrap(), None);
    }
}

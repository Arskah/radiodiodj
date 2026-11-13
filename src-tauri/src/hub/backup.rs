//! A copy of what the hub holds, written to this machine before the hub is
//! emptied. See `docs/shared-library.md#the-hub`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use tokio_postgres::Transaction;

use super::client::timed;

/// Rows per read. A track carries its waveform, so this bounds what is held
/// in memory between the hub and the file.
const PAGE: i64 = 200;

const TABLES: &str = "
CREATE TABLE hub_station (library_id TEXT NOT NULL, protocol INTEGER NOT NULL, owner TEXT);
CREATE TABLE hub_machines (id TEXT PRIMARY KEY, name TEXT NOT NULL, build TEXT NOT NULL,
                           seen_at INTEGER NOT NULL);
CREATE TABLE hub_rows (kind TEXT NOT NULL, key TEXT NOT NULL, rev INTEGER NOT NULL,
                       machine TEXT NOT NULL, edited_at INTEGER NOT NULL,
                       deleted INTEGER NOT NULL, doc TEXT, waveform BLOB, levels BLOB,
                       PRIMARY KEY (kind, key));
";

struct Row {
    kind: String,
    key: String,
    rev: i64,
    machine: String,
    edited_at: i64,
    deleted: bool,
    doc: Option<String>,
    waveform: Option<Vec<u8>>,
    levels: Option<Vec<u8>>,
}

/// Write everything the hub holds to a SQLite file in `dir`, as the hub's own
/// tables, and return where. `None` for a hub with no rows: there is nothing
/// an emptying would lose.
///
/// Read inside `tx`, which holds the station lock, so the copy is of exactly
/// what the caller goes on to delete. A copy that cannot be finished is an
/// error and leaves no file under a backup's name.
pub async fn keep(tx: &Transaction<'_>, dir: &Path) -> Result<Option<PathBuf>> {
    let any: bool = timed(tx.query_one("SELECT EXISTS (SELECT 1 FROM hub_rows)", &[]))
        .await?
        .get(0);
    if !any {
        return Ok(None);
    }
    let path = free_name(dir);
    let partial = path.with_extension("db.partial");
    let _ = std::fs::remove_file(&partial);

    let station = timed(tx.query("SELECT library_id, protocol, owner FROM hub_station", &[]))
        .await?
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect::<Vec<(String, i32, Option<String>)>>();
    let machines = timed(tx.query(
        "SELECT id, name, build, extract(epoch FROM seen_at)::bigint FROM hub_machines",
        &[],
    ))
    .await?
    .iter()
    .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
    .collect::<Vec<(String, String, String, i64)>>();
    let target = partial.clone();
    let mut file = blocking(move || {
        let file = Connection::open(&target).context("create the hub backup")?;
        file.execute_batch(TABLES)?;
        for (library_id, protocol, owner) in station {
            file.execute(
                "INSERT INTO hub_station VALUES (?, ?, ?)",
                params![library_id, protocol, owner],
            )?;
        }
        for (id, name, build, seen_at) in machines {
            file.execute(
                "INSERT INTO hub_machines VALUES (?, ?, ?, ?)",
                params![id, name, build, seen_at],
            )?;
        }
        Ok(file)
    })
    .await?;

    let (mut kind, mut key) = (String::new(), String::new());
    loop {
        let page: Vec<Row> = timed(tx.query(
            "SELECT kind, key, rev, machine, edited_at, deleted, doc::text, waveform, levels \
             FROM hub_rows WHERE (kind, key) > ($1, $2) ORDER BY kind, key LIMIT $3",
            &[&kind, &key, &PAGE],
        ))
        .await?
        .iter()
        .map(|r| Row {
            kind: r.get(0),
            key: r.get(1),
            rev: r.get(2),
            machine: r.get(3),
            edited_at: r.get(4),
            deleted: r.get(5),
            doc: r.get(6),
            waveform: r.get(7),
            levels: r.get(8),
        })
        .collect();
        let Some(last) = page.last() else { break };
        (kind, key) = (last.kind.clone(), last.key.clone());
        file = blocking(move || {
            let tx = file.transaction()?;
            for r in &page {
                tx.execute(
                    "INSERT INTO hub_rows VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        r.kind,
                        r.key,
                        r.rev,
                        r.machine,
                        r.edited_at,
                        r.deleted,
                        r.doc,
                        r.waveform,
                        r.levels
                    ],
                )?;
            }
            tx.commit()?;
            Ok(file)
        })
        .await?;
    }
    let done = path.clone();
    blocking(move || {
        file.close().map_err(|(_, e)| e)?;
        std::fs::rename(&partial, &done).context("name the hub backup")
    })
    .await?;
    log::warn!(
        "shared library: what the hub held is kept in {}",
        path.display()
    );
    Ok(Some(path))
}

/// `radiodiodj.hub.bak.db`, or the first numbered name after it that is free:
/// an earlier backup is never written over.
fn free_name(dir: &Path) -> PathBuf {
    (1..)
        .map(|n| match n {
            1 => dir.join("radiodiodj.hub.bak.db"),
            n => dir.join(format!("radiodiodj.hub.{n}.bak.db")),
        })
        .find(|candidate| !candidate.exists())
        .expect("an unbounded range")
}

/// File work, off the async runtime's own threads.
async fn blocking<T, F>(call: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(call).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_earlier_backup_is_never_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let first = free_name(dir.path());
        assert_eq!(first, dir.path().join("radiodiodj.hub.bak.db"));
        std::fs::write(&first, b"").unwrap();

        let second = free_name(dir.path());
        assert_eq!(second, dir.path().join("radiodiodj.hub.2.bak.db"));
        std::fs::write(&second, b"").unwrap();

        assert_eq!(
            free_name(dir.path()),
            dir.path().join("radiodiodj.hub.3.bak.db")
        );
    }
}

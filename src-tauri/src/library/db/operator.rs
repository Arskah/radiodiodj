//! Operator work as it crosses the hub: the cue set, a metadata edit, the
//! hidden mark, a saved playlist and a dismissal, each as a document going out
//! and a write coming in. Which of two machines' versions stands is not
//! decided here. See `docs/shared-library.md#what-travels`.

use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value as Json};

use super::sync::{Outgoing, EDITABLE};

/// The kinds any machine may write. The rest are the owner's.
pub const KINDS: &[&str] = &["cue", "edit", "hidden", "playlist", "dismissal"];

/// The track a group belongs to, for the three kinds that are part of one.
pub fn track_of(kind: &str, key: &str) -> Option<i64> {
    match kind {
        "cue" | "hidden" => key.parse().ok(),
        "edit" => key.split_once(':')?.0.parse().ok(),
        _ => None,
    }
}

/// An edit's key as the track and the column with its `edited_fields` bit.
fn edit_key(key: &str) -> Option<(i64, &'static str, i64)> {
    let (id, column) = key.split_once(':')?;
    let (column, bit) = EDITABLE.iter().find(|(c, _)| *c == column)?;
    Some((id.parse().ok()?, column, *bit))
}

/// A document's value as the column takes it.
pub(super) fn sql_value(value: &Json) -> Value {
    match value {
        Json::Bool(b) => Value::Integer(i64::from(*b)),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => n.as_f64().map_or(Value::Null, Value::Real),
        },
        Json::String(text) => Value::Text(text.clone()),
        _ => Value::Null,
    }
}

fn json_value(value: rusqlite::types::ValueRef<'_>) -> Json {
    use rusqlite::types::ValueRef as V;
    match value {
        V::Integer(n) => n.into(),
        V::Real(x) => x.into(),
        V::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
        V::Null | V::Blob(_) => Json::Null,
    }
}

/// Give `group` its document as the library reads now. `None` when what it
/// names is gone, which the caller sends as a tombstone.
pub fn document(conn: &Connection, group: &Outgoing) -> Result<Option<Json>> {
    let key = group.key.as_str();
    Ok(match group.kind.as_str() {
        "cue" => conn
            .query_row(
                "SELECT fade_in_ms, fade_out_ms, cue_in_ms, cue_out_ms, next_start_ms, \
                        auto_cue_state = 'manual' \
                 FROM tracks WHERE id = ?",
                [key],
                |r| {
                    let manual: bool = r.get(5)?;
                    Ok(json!({
                        "fade_in_ms": r.get::<_, Option<i64>>(0)?,
                        "fade_out_ms": r.get::<_, Option<i64>>(1)?,
                        // The trio is the owner's until a hand has moved it.
                        "trio": manual.then(|| json!({
                            "cue_in_ms": r.get::<_, Option<i64>>(2).ok().flatten(),
                            "cue_out_ms": r.get::<_, Option<i64>>(3).ok().flatten(),
                            "next_start_ms": r.get::<_, Option<i64>>(4).ok().flatten(),
                        })),
                    }))
                },
            )
            .optional()?,
        "edit" => match edit_key(key) {
            // A revert carries the value the column went back to: the track
            // document that would bring it is not sent again for a revert
            // made on another machine.
            Some((id, column, _)) => conn
                .query_row(
                    &format!("SELECT {column} FROM tracks WHERE id = ?"),
                    [id],
                    |r| Ok(json!({ "value": json_value(r.get_ref(0)?) })),
                )
                .optional()?,
            None => None,
        },
        "hidden" => conn
            .query_row("SELECT hidden_at FROM tracks WHERE id = ?", [key], |r| {
                Ok(json!({ "hidden_at": r.get::<_, Option<i64>>(0)? }))
            })
            .optional()?,
        "playlist" => {
            let list: Option<(i64, String, i64, i64)> = conn
                .query_row(
                    "SELECT id, name, created_at, updated_at FROM saved_playlists WHERE uid = ?",
                    [key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            match list {
                Some((id, name, created_at, updated_at)) => {
                    let mut stmt = conn.prepare(
                        "SELECT track_id, fingerprint, artist, title, duration, content_type \
                         FROM saved_playlist_entries WHERE playlist_id = ? ORDER BY position",
                    )?;
                    let entries = stmt
                        .query_map([id], |r| {
                            Ok(json!({
                                "track_id": r.get::<_, Option<i64>>(0)?,
                                "fingerprint": r.get::<_, Option<String>>(1)?,
                                "artist": r.get::<_, Option<String>>(2)?,
                                "title": r.get::<_, Option<String>>(3)?,
                                "duration": r.get::<_, Option<f64>>(4)?,
                                "content_type": r.get::<_, Option<String>>(5)?,
                            }))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Some(json!({
                        "name": name,
                        "created_at": created_at,
                        "updated_at": updated_at,
                        "entries": entries,
                    }))
                }
                None => None,
            }
        }
        "dismissal" => match key.split_once(':') {
            Some((kind, finding)) => conn
                .query_row(
                    "SELECT value FROM health_dismissals WHERE kind = ?1 AND key = ?2",
                    params![kind, finding],
                    |r| Ok(json!({ "value": r.get::<_, String>(0)? })),
                )
                .optional()?,
            None => None,
        },
        _ => None,
    })
}

/// Write one group as another machine sent it. `false` when it changed
/// nothing here — which for a group that is part of a track means the track
/// is not here, and is the caller's to park.
pub fn write(
    conn: &Connection,
    kind: &str,
    key: &str,
    deleted: bool,
    doc: Option<&Json>,
) -> Result<bool> {
    let field = |name: &str| doc.and_then(|d| d.get(name)).unwrap_or(&Json::Null);
    Ok(match kind {
        "cue" => {
            let fades = params![
                sql_value(field("fade_in_ms")),
                sql_value(field("fade_out_ms")),
                key
            ];
            let mut changed = conn.execute(
                "UPDATE tracks SET fade_in_ms = ?1, fade_out_ms = ?2 WHERE id = ?3",
                fades,
            )?;
            if let Some(trio) = field("trio").as_object() {
                let of = |name: &str| sql_value(trio.get(name).unwrap_or(&Json::Null));
                changed += conn.execute(
                    "UPDATE tracks SET cue_in_ms = ?1, cue_out_ms = ?2, next_start_ms = ?3, \
                            auto_cue_state = 'manual' \
                     WHERE id = ?4",
                    params![of("cue_in_ms"), of("cue_out_ms"), of("next_start_ms"), key],
                )?;
            }
            changed > 0
        }
        "edit" => {
            let Some((id, column, bit)) = edit_key(key) else {
                return Ok(false);
            };
            let flag = if deleted {
                format!("edited_fields & ~{bit}")
            } else {
                format!("edited_fields | {bit}")
            };
            conn.execute(
                &format!("UPDATE tracks SET {column} = ?1, edited_fields = {flag} WHERE id = ?2"),
                params![sql_value(field("value")), id],
            )? > 0
        }
        "hidden" => {
            conn.execute(
                "UPDATE tracks SET hidden_at = ?1 WHERE id = ?2",
                params![sql_value(field("hidden_at")), key],
            )? > 0
        }
        "playlist" => write_playlist(conn, key, deleted, doc)?,
        "dismissal" => {
            let Some((finding_kind, finding)) = key.split_once(':') else {
                return Ok(false);
            };
            let changed = match (deleted, field("value").as_str()) {
                (false, Some(value)) => conn.execute(
                    "INSERT INTO health_dismissals (kind, key, value) VALUES (?1, ?2, ?3) \
                     ON CONFLICT (kind, key) DO UPDATE SET value = excluded.value",
                    params![finding_kind, finding, value],
                )?,
                _ => conn.execute(
                    "DELETE FROM health_dismissals WHERE kind = ?1 AND key = ?2",
                    params![finding_kind, finding],
                )?,
            };
            changed > 0
        }
        _ => false,
    })
}

/// Replace the saved playlist known everywhere as `uid`, whole. An entry is
/// bound to the track it names if this library has it, and is left for
/// fingerprint binding otherwise — as an imported one is.
fn write_playlist(conn: &Connection, uid: &str, deleted: bool, doc: Option<&Json>) -> Result<bool> {
    let existing: Option<i64> = conn
        .query_row("SELECT id FROM saved_playlists WHERE uid = ?", [uid], |r| {
            r.get(0)
        })
        .optional()?;
    let (false, Some(doc)) = (deleted, doc) else {
        let Some(id) = existing else {
            return Ok(false);
        };
        conn.execute(
            "DELETE FROM saved_playlist_entries WHERE playlist_id = ?",
            [id],
        )?;
        conn.execute("DELETE FROM saved_playlists WHERE id = ?", [id])?;
        return Ok(true);
    };
    let Some(name) = doc.get("name").and_then(Json::as_str) else {
        return Ok(false);
    };
    let time = |field: &str| doc.get(field).and_then(Json::as_i64).unwrap_or(0);
    let id = match existing {
        Some(id) => {
            conn.execute(
                "UPDATE saved_playlists SET name = ?1, updated_at = ?2 WHERE id = ?3",
                params![name, time("updated_at"), id],
            )?;
            conn.execute(
                "DELETE FROM saved_playlist_entries WHERE playlist_id = ?",
                [id],
            )?;
            id
        }
        None => {
            conn.execute(
                "INSERT INTO saved_playlists (name, created_at, updated_at, uid) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![name, time("created_at"), time("updated_at"), uid],
            )?;
            conn.last_insert_rowid()
        }
    };
    let mut insert = conn.prepare_cached(
        "INSERT INTO saved_playlist_entries \
           (playlist_id, position, track_id, fingerprint, artist, title, duration, content_type) \
         VALUES (?1, ?2, (SELECT id FROM tracks WHERE id = ?3), ?4, ?5, ?6, ?7, ?8)",
    )?;
    let entries = doc.get("entries").and_then(Json::as_array);
    for (position, entry) in entries.into_iter().flatten().enumerate() {
        let of = |name: &str| sql_value(entry.get(name).unwrap_or(&Json::Null));
        insert.execute(params![
            id,
            position as i64,
            of("track_id"),
            of("fingerprint"),
            of("artist"),
            of("title"),
            of("duration"),
            of("content_type"),
        ])?;
    }
    Ok(true)
}

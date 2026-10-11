//! The one task that talks to the hub. Nothing waits on it: a hub that is
//! slow, away or wrong costs the library freshness and nothing else.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use super::client::{Claim, Hub, Machine};
use crate::library::db::Db;
use crate::library::scanner::now_ms;
use crate::persist::config::{Config, LibraryRole};

/// How often the hub is visited. A change waits this long at most.
const INTERVAL: Duration = Duration::from_secs(30);
/// Groups per transaction. A track carries its waveform, so this bounds what
/// one statement batch holds in memory.
const BATCH: usize = 200;
/// What the owner publishes. Operator work goes with the outbox.
const OWNER_KINDS: &[&str] = &["root", "track"];

/// What one visit to the hub came to.
#[derive(Debug, PartialEq, Eq)]
enum Visit {
    Published(usize),
    NotTheOwner { owner: String },
}

/// Take up the role `config.json` gives this install. Read once: a changed
/// role takes effect at the next launch.
pub fn start(db: Arc<Db>, config: Arc<Config>) {
    if let Err(e) = try_start(db, &config) {
        log::error!("shared library: not started: {e:#}");
    }
}

fn try_start(db: Arc<Db>, config: &Config) -> Result<()> {
    let settings = config.external_library();
    let url = settings.url.filter(|u| !u.trim().is_empty());
    let (LibraryRole::Owner, Some(url)) = (settings.role, url) else {
        return db.stop_capture();
    };
    let hub: tokio_postgres::Config = url
        .parse()
        .context("externalLibrary.url is not a postgresql:// connection URL")?;
    let (id, name) = config.machine()?;
    db.start_capture(now_ms())?;
    log::info!("shared library: this machine ({name}) is the library owner");
    tauri::async_runtime::spawn(run(db, hub, Machine { id, name }));
    Ok(())
}

async fn run(db: Arc<Db>, hub: tokio_postgres::Config, machine: Machine) {
    let mut prepared = false;
    // Said once per change, not once per visit: an unreachable hub would
    // otherwise write the same line every half minute for as long as it is away.
    let mut last = String::new();
    loop {
        let visit = visit(&db, &hub, &machine, &mut prepared).await;
        let line = match &visit {
            Ok(Visit::Published(0)) => "the hub is up to date".to_owned(),
            Ok(Visit::Published(n)) => format!("published {n} changes"),
            Ok(Visit::NotTheOwner { owner }) => {
                format!("{owner} is the library owner, so nothing is published from here")
            }
            Err(e) => {
                prepared = false;
                format!("{e:#}")
            }
        };
        if line != last || matches!(visit, Ok(Visit::Published(n)) if n > 0) {
            match visit {
                Ok(Visit::Published(_)) => log::info!("shared library: {line}"),
                _ => log::warn!("shared library: {line}"),
            }
            last = line;
        }
        tokio::time::sleep(INTERVAL).await;
    }
}

/// Claim the owner role and publish what is owed.
async fn visit(
    db: &Arc<Db>,
    config: &tokio_postgres::Config,
    machine: &Machine,
    prepared: &mut bool,
) -> Result<Visit> {
    let mut hub = Hub::connect_with(config.clone()).await?;
    if !*prepared {
        hub.create_tables().await?;
        *prepared = true;
    }
    let known = on(db, Db::library_id).await?;
    match hub.claim(machine, known.as_deref()).await? {
        Claim::Taken { by } => return Ok(Visit::NotTheOwner { owner: by }),
        Claim::Replaced { library_id } => {
            on(db, move |db| db.publish_as(&library_id, now_ms())).await?;
        }
        Claim::Held { .. } => {}
    }
    hub.check_in(machine).await?;
    let mut published = 0;
    loop {
        let batch = on(db, |db| db.outgoing(OWNER_KINDS, BATCH)).await?;
        if batch.is_empty() {
            return Ok(Visit::Published(published));
        }
        hub.publish(machine, &batch).await?;
        published += batch.len();
        on(db, move |db| db.mark_sent(&batch)).await?;
    }
}

/// A library call, off the async runtime's own threads.
async fn on<T, F>(db: &Arc<Db>, call: F) -> Result<T>
where
    F: FnOnce(&Db) -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    let db = Arc::clone(db);
    tauri::async_runtime::spawn_blocking(move || call(&db)).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::db::TrackInsert;

    /// A hub of this test's own: a schema in the database `RADIODIODJ_TEST_HUB`
    /// names. `None` without the variable, and the test passes without running.
    async fn test_hub() -> Option<tokio_postgres::Config> {
        let url = std::env::var("RADIODIODJ_TEST_HUB").ok()?;
        let mut config: tokio_postgres::Config = url.parse().unwrap();
        let schema = format!("hub_{}", uuid::Uuid::new_v4().simple());
        let (client, connection) = config.connect(tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(connection);
        client
            .batch_execute(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        config.options(format!("-c search_path={schema}"));
        Some(config)
    }

    async fn query(hub: &tokio_postgres::Config, sql: &str) -> Vec<tokio_postgres::Row> {
        let (client, connection) = hub.connect(tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(connection);
        client.query(sql, &[]).await.unwrap()
    }

    fn machine(name: &str) -> Machine {
        Machine {
            id: format!("id-{name}"),
            name: name.into(),
        }
    }

    fn library(titles: &[&str]) -> Arc<Db> {
        let db = Db::open_in_memory().unwrap();
        for title in titles {
            db.insert_track(&TrackInsert {
                path: format!("/{title}.mp3"),
                content_type: "music".into(),
                title: Some((*title).into()),
                ..Default::default()
            })
            .unwrap();
        }
        db.start_capture(1).unwrap();
        Arc::new(db)
    }

    async fn titles(hub: &tokio_postgres::Config) -> Vec<String> {
        query(
            hub,
            "SELECT doc->>'title' FROM hub_rows WHERE kind = 'track' AND NOT deleted ORDER BY rev",
        )
        .await
        .iter()
        .map(|r| r.get(0))
        .collect()
    }

    #[tokio::test]
    async fn the_owner_publishes_its_library_to_an_empty_hub() {
        let Some(hub) = test_hub().await else { return };
        let db = library(&["One", "Two"]);
        db.add_root("music").unwrap();

        let visit = visit(&db, &hub, &machine("office"), &mut false)
            .await
            .unwrap();

        assert_eq!(visit, Visit::Published(3));
        assert_eq!(titles(&hub).await, ["One", "Two"]);
        let station = query(&hub, "SELECT owner, library_id, protocol FROM hub_station").await;
        assert_eq!(station[0].get::<_, String>(0), "id-office");
        assert_eq!(
            Some(station[0].get::<_, String>(1)),
            db.library_id().unwrap()
        );
        let seen = query(&hub, "SELECT name FROM hub_machines").await;
        assert_eq!(seen[0].get::<_, String>(0), "office");
    }

    #[tokio::test]
    async fn a_second_visit_publishes_only_what_changed() {
        let Some(hub) = test_hub().await else { return };
        let db = library(&["One", "Two"]);
        let me = machine("office");
        visit(&db, &hub, &me, &mut false).await.unwrap();
        assert_eq!(
            visit(&db, &hub, &me, &mut true).await.unwrap(),
            Visit::Published(0)
        );

        db.insert_track(&TrackInsert {
            path: "/One.mp3".into(),
            content_type: "music".into(),
            title: Some("One again".into()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(
            visit(&db, &hub, &me, &mut true).await.unwrap(),
            Visit::Published(1)
        );
        assert_eq!(
            titles(&hub).await,
            ["Two", "One again"],
            "the changed row has the newest revision"
        );
    }

    #[tokio::test]
    async fn a_second_owner_is_refused_and_publishes_nothing() {
        let Some(hub) = test_hub().await else { return };
        visit(&library(&["One"]), &hub, &machine("office"), &mut false)
            .await
            .unwrap();
        let other = library(&["Intruder"]);

        let visit = visit(&other, &hub, &machine("laptop"), &mut true)
            .await
            .unwrap();

        assert_eq!(
            visit,
            Visit::NotTheOwner {
                owner: "office".into()
            }
        );
        assert_eq!(titles(&hub).await, ["One"]);
        assert_eq!(
            other.outgoing(OWNER_KINDS, 10).unwrap().len(),
            1,
            "still owed"
        );
    }

    /// Its ids are another database's, so nothing the hub held can be kept.
    #[tokio::test]
    async fn an_owner_whose_database_was_replaced_starts_the_hub_again() {
        let Some(hub) = test_hub().await else { return };
        let me = machine("office");
        let before = library(&["Old one", "Old two"]);
        visit(&before, &hub, &me, &mut false).await.unwrap();
        let after = library(&["New"]);

        visit(&after, &hub, &me, &mut true).await.unwrap();

        assert_eq!(titles(&hub).await, ["New"]);
        assert_ne!(after.library_id().unwrap(), before.library_id().unwrap());
    }

    #[tokio::test]
    async fn a_purged_track_is_published_as_a_tombstone() {
        let Some(hub) = test_hub().await else { return };
        let db = library(&["One"]);
        let me = machine("office");
        visit(&db, &hub, &me, &mut false).await.unwrap();
        let id = db.search("", None, None, None).unwrap()[0].id;
        db.reconcile(&crate::library::db::Reconcile {
            gone: vec![id],
            now_ms: 5,
            ..Default::default()
        })
        .unwrap();
        db.purge_tracks(&[id]).unwrap();

        visit(&db, &hub, &me, &mut true).await.unwrap();

        let rows = query(&hub, "SELECT deleted, doc IS NULL FROM hub_rows").await;
        assert_eq!(rows.len(), 1);
        assert!(rows[0].get::<_, bool>(0) && rows[0].get::<_, bool>(1));
    }

    #[tokio::test]
    async fn a_hub_on_a_later_protocol_is_left_alone() {
        let Some(hub) = test_hub().await else { return };
        let db = library(&["One"]);
        let me = machine("office");
        visit(&db, &hub, &me, &mut false).await.unwrap();
        query(&hub, "UPDATE hub_station SET protocol = protocol + 1").await;
        query(&hub, "DELETE FROM hub_rows").await;
        db.publish_as(&db.library_id().unwrap().unwrap(), 9)
            .unwrap();

        let refused = visit(&db, &hub, &me, &mut true).await.unwrap_err();

        assert!(format!("{refused:#}").contains("protocol"), "{refused:#}");
        assert!(titles(&hub).await.is_empty());
    }
}

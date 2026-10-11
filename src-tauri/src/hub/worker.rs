//! The one task that talks to the hub. Nothing waits on it: a hub that is
//! slow, away or wrong costs the library freshness and nothing else.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use super::client::{Claim, Hub, Link, Machine};
use super::schema::PROTOCOL;
use super::{OWNER_COMMANDS, STUDIO_ERROR};
use crate::library::db::{Applied, Db, OPERATOR_KINDS};
use crate::library::health::Health;
use crate::library::scanner::now_ms;
use crate::persist::config::{Config, LibraryRole};

/// How often the hub is visited. A change waits this long at most.
const INTERVAL: Duration = Duration::from_secs(30);
/// Groups per transaction. A track carries its waveform, so this bounds what
/// one statement batch holds in memory.
const BATCH: usize = 200;
/// How often the local outbox is looked at. An operator's save should not
/// wait for the next visit, and looking costs one read of a small table.
const GLANCE: Duration = Duration::from_secs(2);
/// What the owner sends: the library, and operator work done on this machine.
const OWNER_SENDS: &[&str] = &[
    "root",
    "track",
    "cue",
    "edit",
    "hidden",
    "playlist",
    "dismissal",
];
/// What a studio sends: operator work, which is all that is a studio's.
const STUDIO_SENDS: &[&str] = OPERATOR_KINDS;

/// What one visit to the hub came to.
#[derive(Debug, PartialEq, Eq)]
enum Visit {
    /// The owner sent this many groups.
    Published(usize),
    /// A studio sent this many groups, took this many rows, and this many of
    /// them changed its copy.
    Pulled {
        sent: usize,
        rows: usize,
        applied: usize,
        owner: OwnerSeen,
    },
    NotTheOwner {
        owner: String,
    },
    /// A studio found a hub no owner has published to.
    NothingToCopy,
}

/// Whether anything is feeding the library a studio copies.
#[derive(Debug, PartialEq, Eq)]
enum OwnerSeen {
    Lately,
    /// The owner has not checked in for this many seconds.
    Quiet {
        name: String,
        secs: i64,
    },
    /// No machine holds the owner role.
    Nobody,
}

/// How long an owner may go without checking in before a studio says so:
/// long enough that one slow visit is not news.
const OWNER_QUIET_AFTER: i64 = 10 * INTERVAL.as_secs() as i64;

/// A duration as an operator reads it.
fn ago(secs: i64) -> String {
    let one = |n: i64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        s if s < 3600 => one((s / 60).max(1), "minute"),
        s if s < 86_400 => one(s / 3600, "hour"),
        s => one(s / 86_400, "day"),
    }
}

/// An error chain as a sentence.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out: String = first.to_uppercase().chain(chars).collect();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// Where the shared library stands, for the Settings page.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The role this launch took up, which a saved change does not move.
    pub role: LibraryRole,
    /// Whether the last visit did what the role asks.
    pub ok: bool,
    pub message: String,
    /// When the hub last answered, unix ms.
    pub reached_at: Option<i64>,
    /// Whether the last connection to the hub was encrypted, once one opened.
    pub encrypted: Option<bool>,
    /// How many of this machine's changes the hub has not been sent yet.
    pub waiting: usize,
}

/// Emitted with a [`Status`] whenever it changes.
pub const STATE_EVENT: &str = "hub:state";
/// Emitted when a pull changed this machine's copy of the library.
pub const LIBRARY_EVENT: &str = "hub:library-changed";

/// The role `config.json` asks for, which needs an address to mean anything.
pub fn role_in_effect(config: &Config) -> LibraryRole {
    let settings = config.external_library();
    match settings.url {
        Some(url) if !url.trim().is_empty() => settings.role,
        _ => LibraryRole::Standalone,
    }
}

/// This machine's part in a shared library, and the worker that plays it.
pub struct Service {
    role: LibraryRole,
    status: Mutex<Status>,
    app: AppHandle,
    /// Told what a pull changed, for the copies of it held outside the
    /// library: the playlist's cue points, the saved-playlist list, the files
    /// a metadata edit is written to.
    on_applied: Box<dyn Fn(&Applied) + Send + Sync>,
}

impl Service {
    /// Take up `role`, which the caller has already prepared the database for.
    /// Read once: a changed role takes effect at the next launch.
    pub fn start(
        app: AppHandle,
        role: LibraryRole,
        db: Arc<Db>,
        config: &Config,
        health: Arc<Health>,
        on_applied: Box<dyn Fn(&Applied) + Send + Sync>,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            on_applied,
            role,
            status: Mutex::new(Status {
                role,
                ok: role == LibraryRole::Standalone,
                message: String::new(),
                reached_at: None,
                encrypted: None,
                waiting: 0,
            }),
            app,
        });
        if let Err(e) = Arc::clone(&service).spawn(db, config, health) {
            log::error!("shared library: not started: {e:#}");
            service.report(false, sentence(&format!("{e:#}")), false);
        }
        service
    }

    fn spawn(self: Arc<Self>, db: Arc<Db>, config: &Config, health: Arc<Health>) -> Result<()> {
        if self.role == LibraryRole::Standalone {
            return db.stop_capture();
        }
        let hub = Link::new(&config.external_library())?;
        let (id, name) = config.machine()?;
        db.start_capture(now_ms())?;
        log::info!("shared library: this machine ({name}) is {:?}", self.role);
        tauri::async_runtime::spawn(self.run(db, hub, Machine { id, name }, health));
        Ok(())
    }

    pub fn is_studio(&self) -> bool {
        self.role == LibraryRole::Studio
    }

    pub fn status(&self) -> Status {
        self.status.lock().clone()
    }

    /// Refuse a command that is the owner's while this machine is a studio.
    pub fn gate(&self, command: &str) -> Result<(), &'static str> {
        if self.is_studio() && OWNER_COMMANDS.contains(&command) {
            return Err(STUDIO_ERROR);
        }
        Ok(())
    }

    /// Store and announce where things stand. Logged once per change, not once
    /// per visit: an unreachable hub would otherwise write the same line every
    /// half minute for as long as it is away.
    fn report(&self, ok: bool, message: String, reached: bool) {
        let next = {
            let mut status = self.status.lock();
            if status.message != message {
                if ok {
                    log::info!("shared library: {message}");
                } else {
                    log::warn!("shared library: {message}");
                }
            }
            status.ok = ok;
            status.message = message;
            if reached {
                status.reached_at = Some(now_ms());
            }
            status.clone()
        };
        let _ = self.app.emit(STATE_EVENT, &next);
    }

    async fn run(self: Arc<Self>, db: Arc<Db>, hub: Link, machine: Machine, health: Arc<Health>) {
        let sends = match self.role {
            LibraryRole::Owner => OWNER_SENDS,
            _ => STUDIO_SENDS,
        };
        let mut prepared = false;
        // A visit is due at once, then every `INTERVAL` — or as soon as this
        // machine has something to send, unless the last visit could not send:
        // a hub that is away, or has no place for what this machine owes, is
        // not asked again every two seconds.
        let mut due = Instant::now();
        let mut blocked = false;
        loop {
            let waiting = self.count_waiting(&db, sends).await;
            if Instant::now() < due && (blocked || waiting == 0) {
                tokio::time::sleep(GLANCE).await;
                continue;
            }
            let visit = match self.role {
                LibraryRole::Owner => owner_visit(&db, &hub, &machine, &mut prepared).await,
                _ => studio_visit(&db, &hub, &machine).await,
            };
            due = Instant::now() + INTERVAL;
            blocked = !matches!(visit, Ok((Visit::Published(_) | Visit::Pulled { .. }, _)));
            self.status.lock().encrypted = hub.encrypted();
            let taken = match visit {
                Ok((visit, taken)) => {
                    self.say(visit, &taken);
                    taken
                }
                Err(e) => {
                    prepared = false;
                    self.report(false, sentence(&format!("{e:#}")), false);
                    Applied::default()
                }
            };
            self.count_waiting(&db, sends).await;
            if taken.changed > 0 {
                // Entries that were waiting for these tracks, then everything
                // that lists or holds a copy of what the library holds.
                let _ = on(&db, Db::bind_saved_entries).await;
                // Both read the library, the second all of it: not on one of
                // the async runtime's own threads.
                let (me, report) = (Arc::clone(&self), Arc::clone(&health));
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    (me.on_applied)(&taken);
                    report.refresh();
                })
                .await;
                let _ = self.app.emit(LIBRARY_EVENT, ());
            }
            tokio::time::sleep(GLANCE).await;
        }
    }

    /// Count what this machine still owes the hub, and say so if it changed:
    /// the page shows it, and an operator watching a save go out sees it go.
    async fn count_waiting(&self, db: &Arc<Db>, sends: &'static [&'static str]) -> usize {
        let waiting = on(db, move |db| db.outgoing_count(sends))
            .await
            .unwrap_or(0);
        let changed = {
            let mut status = self.status.lock();
            let changed = status.waiting != waiting;
            status.waiting = waiting;
            changed.then(|| status.clone())
        };
        if let Some(status) = changed {
            let _ = self.app.emit(STATE_EVENT, &status);
        }
        waiting
    }

    /// Report what a visit came to.
    fn say(&self, visit: Visit, taken: &Applied) {
        let also_took = |message: &mut String| {
            if taken.changed > 0 {
                message.push_str(&format!(" Took {} from other computers.", taken.changed));
            }
        };
        match visit {
            Visit::Published(sent) => {
                let mut message = match sent {
                    0 => "The hub is up to date.".to_owned(),
                    n => format!("Published {n} changes."),
                };
                also_took(&mut message);
                self.report(true, message, true);
            }
            Visit::Pulled {
                sent,
                rows: _,
                applied,
                owner,
            } => {
                let mut message = match applied {
                    0 => "This copy is up to date.".to_owned(),
                    n => format!("Took {n} changes from the hub."),
                };
                if sent > 0 {
                    message.push_str(&format!(" Sent {sent}."));
                }
                match owner {
                    OwnerSeen::Lately => {}
                    OwnerSeen::Quiet { name, secs } => message.push_str(&format!(
                        " The library owner, {name}, was last seen {}.",
                        ago(secs)
                    )),
                    OwnerSeen::Nobody => message.push_str(
                        " No computer is the library owner now, so nothing new will arrive.",
                    ),
                }
                self.report(true, message, true);
            }
            Visit::NotTheOwner { owner } => self.report(
                false,
                format!("{owner} is the library owner, so nothing is published from here."),
                true,
            ),
            Visit::NothingToCopy => self.report(
                false,
                "The hub holds no library yet: no owner has published to it.".into(),
                true,
            ),
        }
    }
}

/// Send what this machine owes, a transaction at a time. Returns how many
/// groups went and what the later saves some of them lost to changed here.
async fn send(
    db: &Arc<Db>,
    hub: &mut Hub,
    machine: &Machine,
    kinds: &'static [&'static str],
    owner: bool,
) -> Result<(usize, Applied)> {
    let (mut sent, mut taken) = (0, Applied::default());
    loop {
        let batch = on(db, move |db| db.outgoing(kinds, BATCH)).await?;
        if batch.is_empty() {
            return Ok((sent, taken));
        }
        let winners = hub.send(machine, &batch, owner).await?;
        sent += batch.len();
        let me = machine.id.clone();
        taken.absorb(
            on(db, move |db| {
                db.mark_sent(&batch)?;
                db.adopt(&winners, &me, now_ms())
            })
            .await?,
        );
    }
}

/// Take what the hub has that this library does not, a page at a time.
/// Returns how many rows came and what they changed.
async fn take(
    db: &Arc<Db>,
    hub: &Hub,
    machine: &Machine,
    skip_own: bool,
) -> Result<(usize, Applied)> {
    let (mut rows, mut taken) = (0, Applied::default());
    loop {
        let after = on(db, Db::pulled_rev).await?;
        let page = hub
            .fetch(after, BATCH as i64, skip_own.then_some(machine))
            .await?;
        if page.is_empty() {
            return Ok((rows, taken));
        }
        rows += page.len();
        let me = machine.id.clone();
        taken.absorb(on(db, move |db| db.apply(&page, &me, now_ms())).await?);
    }
}

/// The owner's visit: claim the role, publish what is owed, and take the
/// operator work the studios have sent.
async fn owner_visit(
    db: &Arc<Db>,
    link: &Link,
    machine: &Machine,
    prepared: &mut bool,
) -> Result<(Visit, Applied)> {
    let mut hub = link.connect().await?;
    if !*prepared {
        hub.create_tables().await?;
        *prepared = true;
    }
    let known = on(db, Db::library_id).await?;
    match hub.claim(machine, known.as_deref()).await? {
        Claim::Taken { by } => return Ok((Visit::NotTheOwner { owner: by }, Applied::default())),
        Claim::Replaced { library_id } => {
            on(db, move |db| db.publish_as(&library_id, now_ms())).await?;
        }
        Claim::Held { .. } => {}
    }
    hub.check_in(machine).await?;
    // Sent before anything is taken, so the hub has this machine's saves to
    // compare with before a pull could write over one.
    let (sent, mut taken) = send(db, &mut hub, machine, OWNER_SENDS, true).await?;
    taken.absorb(take(db, &hub, machine, true).await?.1);
    Ok((Visit::Published(sent), taken))
}

/// A studio's visit: send its operator work, and take what the hub has that
/// this copy does not.
async fn studio_visit(db: &Arc<Db>, link: &Link, machine: &Machine) -> Result<(Visit, Applied)> {
    let mut hub = link.connect().await?;
    let Some(station) = hub.station().await? else {
        return Ok((Visit::NothingToCopy, Applied::default()));
    };
    if station.protocol > PROTOCOL {
        bail!(
            "the hub speaks protocol {} and this version speaks {PROTOCOL}: \
             update RadiodioDJ on this machine",
            station.protocol
        );
    }
    match on(db, Db::library_id).await? {
        None => {
            let id = station.library_id.clone();
            on(db, move |db| db.follow(&id)).await?;
        }
        Some(held) if held == station.library_id => {}
        Some(_) => bail!(
            "the hub now holds a different library than this copy was made from: \
             join the shared library again"
        ),
    }
    hub.check_in(machine).await?;
    let owner = match station.owner {
        Some(owner) if owner.id == machine.id => {
            hub.release(machine).await?;
            OwnerSeen::Nobody
        }
        Some(owner) if owner.quiet_for > OWNER_QUIET_AFTER => OwnerSeen::Quiet {
            name: owner.name,
            secs: owner.quiet_for,
        },
        Some(_) => OwnerSeen::Lately,
        None => OwnerSeen::Nobody,
    };
    let (sent, mut taken) = send(db, &mut hub, machine, STUDIO_SENDS, false).await?;
    let (rows, pulled) = take(db, &hub, machine, false).await?;
    taken.absorb(pulled);
    let visit = Visit::Pulled {
        sent,
        rows,
        applied: taken.changed,
        owner,
    };
    Ok((visit, taken))
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

    async fn publish(
        db: &Arc<Db>,
        link: &Link,
        machine: &Machine,
        prepared: &mut bool,
    ) -> Result<Visit> {
        Ok(owner_visit(db, link, machine, prepared).await?.0)
    }

    async fn pull(db: &Arc<Db>, link: &Link, machine: &Machine) -> Result<Visit> {
        Ok(studio_visit(db, link, machine).await?.0)
    }

    /// A hub of this test's own: a schema in the database `RADIODIODJ_TEST_HUB`
    /// names. `None` without the variable, and the test passes without running.
    async fn test_hub() -> Option<Link> {
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
        Some(Link::over(config, true, Vec::new()))
    }

    async fn query(hub: &Link, sql: &str) -> Vec<tokio_postgres::Row> {
        let (client, connection) = hub.config().connect(tokio_postgres::NoTls).await.unwrap();
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

    async fn titles(hub: &Link) -> Vec<String> {
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

        let visit = publish(&db, &hub, &machine("office"), &mut false)
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
        publish(&db, &hub, &me, &mut false).await.unwrap();
        assert_eq!(
            publish(&db, &hub, &me, &mut true).await.unwrap(),
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
            publish(&db, &hub, &me, &mut true).await.unwrap(),
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
        publish(&library(&["One"]), &hub, &machine("office"), &mut false)
            .await
            .unwrap();
        let other = library(&["Intruder"]);

        let visit = publish(&other, &hub, &machine("laptop"), &mut true)
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
            other.outgoing(OWNER_SENDS, 10).unwrap().len(),
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
        publish(&before, &hub, &me, &mut false).await.unwrap();
        let after = library(&["New"]);

        publish(&after, &hub, &me, &mut true).await.unwrap();

        assert_eq!(titles(&hub).await, ["New"]);
        assert_ne!(after.library_id().unwrap(), before.library_id().unwrap());
    }

    /// Its revisions start over, so a cursor kept from before would step over
    /// everything published to it.
    #[tokio::test]
    async fn a_hub_that_was_made_again_holds_a_new_library() {
        let Some(hub) = test_hub().await else { return };
        let me = machine("office");
        let db = library(&["One"]);
        publish(&db, &hub, &me, &mut false).await.unwrap();
        let studio = Arc::new(Db::open_in_memory().unwrap());
        studio.become_replica().unwrap();
        pull(&studio, &hub, &machine("studio")).await.unwrap();
        let before = db.library_id().unwrap();
        query(&hub, "DROP TABLE hub_station, hub_machines, hub_rows").await;
        query(&hub, "DROP SEQUENCE hub_rev").await;

        let visit = publish(&db, &hub, &me, &mut false).await.unwrap();

        assert_eq!(visit, Visit::Published(1));
        assert_ne!(db.library_id().unwrap(), before);
        let refused = pull(&studio, &hub, &machine("studio")).await.unwrap_err();
        assert!(format!("{refused:#}").contains("join"), "{refused:#}");
    }

    #[tokio::test]
    async fn a_purged_track_is_published_as_a_tombstone() {
        let Some(hub) = test_hub().await else { return };
        let db = library(&["One"]);
        let me = machine("office");
        publish(&db, &hub, &me, &mut false).await.unwrap();
        let id = db.search("", None, None, None).unwrap()[0].id;
        db.reconcile(&crate::library::db::Reconcile {
            gone: vec![id],
            now_ms: 5,
            ..Default::default()
        })
        .unwrap();
        db.purge_tracks(&[id]).unwrap();

        publish(&db, &hub, &me, &mut true).await.unwrap();

        let rows = query(&hub, "SELECT deleted, doc IS NULL FROM hub_rows").await;
        assert_eq!(rows.len(), 1);
        assert!(rows[0].get::<_, bool>(0) && rows[0].get::<_, bool>(1));
    }

    #[tokio::test]
    async fn a_hub_on_a_later_protocol_is_left_alone() {
        let Some(hub) = test_hub().await else { return };
        let db = library(&["One"]);
        let me = machine("office");
        publish(&db, &hub, &me, &mut false).await.unwrap();
        query(&hub, "UPDATE hub_station SET protocol = protocol + 1").await;
        query(&hub, "DELETE FROM hub_rows").await;
        db.publish_as(&db.library_id().unwrap().unwrap(), 9)
            .unwrap();

        let refused = publish(&db, &hub, &me, &mut true).await.unwrap_err();

        assert!(format!("{refused:#}").contains("protocol"), "{refused:#}");
        assert!(titles(&hub).await.is_empty());
    }

    #[tokio::test]
    async fn a_studio_copies_what_the_owner_published() {
        let Some(hub) = test_hub().await else { return };
        let owner = library(&["One", "Two"]);
        publish(&owner, &hub, &machine("office"), &mut false)
            .await
            .unwrap();
        let studio = Arc::new(Db::open_in_memory().unwrap());
        studio.become_replica().unwrap();

        let visit = pull(&studio, &hub, &machine("studio")).await.unwrap();

        assert_eq!(
            visit,
            Visit::Pulled {
                sent: 0,
                rows: 2,
                applied: 2,
                owner: OwnerSeen::Lately
            }
        );
        assert_eq!(studio.library_id().unwrap(), owner.library_id().unwrap());
        let ids = |db: &Db| -> Vec<(i64, String)> {
            let mut all: Vec<_> = db
                .search("", None, None, None)
                .unwrap()
                .into_iter()
                .map(|t| (t.id, t.title))
                .collect();
            all.sort();
            all
        };
        assert_eq!(ids(&studio), ids(&owner));
        let seen = query(&hub, "SELECT name FROM hub_machines ORDER BY name").await;
        assert_eq!(seen.len(), 2);

        assert_eq!(
            pull(&studio, &hub, &machine("studio")).await.unwrap(),
            Visit::Pulled {
                sent: 0,
                rows: 0,
                applied: 0,
                owner: OwnerSeen::Lately
            }
        );
    }

    #[tokio::test]
    async fn a_studio_takes_a_later_change_and_a_purge() {
        let Some(hub) = test_hub().await else { return };
        let owner = library(&["One", "Two"]);
        let me = machine("office");
        publish(&owner, &hub, &me, &mut false).await.unwrap();
        let studio = Arc::new(Db::open_in_memory().unwrap());
        studio.become_replica().unwrap();
        pull(&studio, &hub, &machine("studio")).await.unwrap();

        let gone = owner.search("Two", None, None, None).unwrap()[0].id;
        owner
            .reconcile(&crate::library::db::Reconcile {
                gone: vec![gone],
                now_ms: 5,
                ..Default::default()
            })
            .unwrap();
        owner.purge_tracks(&[gone]).unwrap();
        owner
            .insert_track(&TrackInsert {
                path: "/One.mp3".into(),
                content_type: "music".into(),
                title: Some("One again".into()),
                ..Default::default()
            })
            .unwrap();
        publish(&owner, &hub, &me, &mut true).await.unwrap();

        pull(&studio, &hub, &machine("studio")).await.unwrap();

        let titles: Vec<String> = studio
            .search("", None, None, None)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, ["One again"]);
    }

    #[tokio::test]
    async fn a_studio_finds_nothing_to_copy_in_a_hub_no_owner_has_used() {
        let Some(hub) = test_hub().await else { return };
        let studio = Arc::new(Db::open_in_memory().unwrap());
        studio.become_replica().unwrap();

        assert_eq!(
            pull(&studio, &hub, &machine("studio")).await.unwrap(),
            Visit::NothingToCopy
        );
    }

    /// Its ids are the old library's, so taking the new one's rows over them
    /// would join two unrelated tracks.
    #[tokio::test]
    async fn a_studio_stops_when_the_hub_holds_another_library() {
        let Some(hub) = test_hub().await else { return };
        let me = machine("office");
        publish(&library(&["Old"]), &hub, &me, &mut false)
            .await
            .unwrap();
        let studio = Arc::new(Db::open_in_memory().unwrap());
        studio.become_replica().unwrap();
        pull(&studio, &hub, &machine("studio")).await.unwrap();
        publish(&library(&["New"]), &hub, &me, &mut true)
            .await
            .unwrap();

        let refused = pull(&studio, &hub, &machine("studio")).await.unwrap_err();

        assert!(format!("{refused:#}").contains("join"), "{refused:#}");
        assert_eq!(studio.search("", None, None, None).unwrap()[0].title, "Old");
    }

    async fn joined_studio(hub: &Link) -> Arc<Db> {
        publish(&library(&["One"]), hub, &machine("office"), &mut false)
            .await
            .unwrap();
        let studio = Arc::new(Db::open_in_memory().unwrap());
        studio.become_replica().unwrap();
        studio
    }

    fn owner_seen(visit: Visit) -> OwnerSeen {
        match visit {
            Visit::Pulled { owner, .. } => owner,
            other => panic!("not a pull: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_studio_says_when_the_owner_has_gone_quiet() {
        let Some(hub) = test_hub().await else { return };
        let studio = joined_studio(&hub).await;
        query(
            &hub,
            "UPDATE hub_machines SET seen_at = now() - interval '3 hours' WHERE name = 'office'",
        )
        .await;

        let seen = owner_seen(pull(&studio, &hub, &machine("studio")).await.unwrap());

        let OwnerSeen::Quiet { name, secs } = seen else {
            panic!("{seen:?}");
        };
        assert_eq!(name, "office");
        assert_eq!(ago(secs), "3 hours ago");
    }

    /// The claim would otherwise name an owner that never publishes again.
    #[tokio::test]
    async fn an_owner_that_became_a_studio_gives_the_role_up() {
        let Some(hub) = test_hub().await else { return };
        let studio = joined_studio(&hub).await;

        let seen = owner_seen(pull(&studio, &hub, &machine("office")).await.unwrap());

        assert_eq!(seen, OwnerSeen::Nobody);
        let owner = query(&hub, "SELECT owner FROM hub_station").await;
        assert_eq!(owner[0].get::<_, Option<String>>(0), None);
    }

    #[test]
    fn a_duration_reads_in_its_largest_whole_unit() {
        assert_eq!(ago(20), "1 minute ago");
        assert_eq!(ago(300), "5 minutes ago");
        assert_eq!(ago(3600), "1 hour ago");
        assert_eq!(ago(86_400 * 2 + 5), "2 days ago");
    }

    #[test]
    fn an_error_is_shown_as_a_sentence() {
        assert_eq!(
            sentence("the hub cannot be reached: connection refused"),
            "The hub cannot be reached: connection refused."
        );
        assert_eq!(sentence("Done."), "Done.");
        assert_eq!(sentence(""), "");
    }

    /// A studio that has joined `hub` and captures, as one in a shared
    /// library does.
    async fn studio(hub: &Link, name: &str) -> Arc<Db> {
        let db = Arc::new(Db::open_in_memory().unwrap());
        db.become_replica().unwrap();
        pull(&db, hub, &machine(name)).await.unwrap();
        db.start_capture(1).unwrap();
        db
    }

    fn only_track(db: &Db) -> crate::library::db::Track {
        db.search("", None, None, None).unwrap().remove(0)
    }

    fn fade_in(db: &Db, ms: i64) {
        let id = only_track(db).id;
        db.set_cue_points(
            id,
            crate::audio::cue_points::CuePoints {
                fade_in_ms: Some(ms),
                ..Default::default()
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn an_edit_made_on_a_studio_reaches_the_owner_and_the_other_studio() {
        let Some(hub) = test_hub().await else { return };
        let owner = library(&["One"]);
        let office = machine("office");
        publish(&owner, &hub, &office, &mut false).await.unwrap();
        let a = studio(&hub, "a").await;
        let b = studio(&hub, "b").await;
        let id = only_track(&a).id;
        a.update_track_metadata(&crate::library::db::TrackMetadataUpdate {
            id,
            title: Some("Fixed in studio A".into()),
            ..Default::default()
        })
        .unwrap();

        let sent = pull(&a, &hub, &machine("a")).await.unwrap();
        assert!(matches!(sent, Visit::Pulled { sent: 1, .. }), "{sent:?}");

        let (_, taken) = owner_visit(&owner, &hub, &office, &mut true).await.unwrap();
        assert_eq!(taken.edited_tracks, [id], "the owner writes it to the file");
        assert_eq!(only_track(&owner).title, "Fixed in studio A");

        pull(&b, &hub, &machine("b")).await.unwrap();
        assert_eq!(only_track(&b).title, "Fixed in studio A");
    }

    #[tokio::test]
    async fn an_edit_made_on_the_owner_reaches_a_studio() {
        let Some(hub) = test_hub().await else { return };
        let owner = library(&["One"]);
        let office = machine("office");
        publish(&owner, &hub, &office, &mut false).await.unwrap();
        let a = studio(&hub, "a").await;
        owner.hide_tracks(&[only_track(&owner).id], 5).unwrap();

        publish(&owner, &hub, &office, &mut true).await.unwrap();
        pull(&a, &hub, &machine("a")).await.unwrap();

        assert_eq!(a.hidden_tracks().unwrap().len(), 1);
    }

    /// Whichever of the two reaches the hub first.
    #[tokio::test]
    async fn the_later_of_two_saves_stands_on_every_machine() {
        for later_first in [false, true] {
            let Some(hub) = test_hub().await else { return };
            let owner = library(&["One"]);
            let office = machine("office");
            publish(&owner, &hub, &office, &mut false).await.unwrap();
            let a = studio(&hub, "a").await;
            let b = studio(&hub, "b").await;
            fade_in(&a, 1000);
            a.restamp_pending(100);
            fade_in(&b, 2000);
            b.restamp_pending(200);

            let order: [(&Arc<Db>, &str); 2] = if later_first {
                [(&b, "b"), (&a, "a")]
            } else {
                [(&a, "a"), (&b, "b")]
            };
            for (db, name) in order {
                pull(db, &hub, &machine(name)).await.unwrap();
            }
            for (db, name) in [(&a, "a"), (&b, "b")] {
                pull(db, &hub, &machine(name)).await.unwrap();
            }
            owner_visit(&owner, &hub, &office, &mut true).await.unwrap();

            for db in [&a, &b, &owner] {
                assert_eq!(
                    only_track(db).cue_points.fade_in_ms,
                    Some(2000),
                    "later first: {later_first}"
                );
            }
        }
    }

    /// Its clock is behind, so its save loses to one it had already pulled —
    /// which no pull brings again.
    #[tokio::test]
    async fn a_machine_whose_save_lost_ends_up_with_the_one_that_won() {
        let Some(hub) = test_hub().await else { return };
        let owner = library(&["One"]);
        publish(&owner, &hub, &machine("office"), &mut false)
            .await
            .unwrap();
        let a = studio(&hub, "a").await;
        let b = studio(&hub, "b").await;
        fade_in(&b, 2000);
        b.restamp_pending(200);
        pull(&b, &hub, &machine("b")).await.unwrap();
        pull(&a, &hub, &machine("a")).await.unwrap();
        fade_in(&a, 1000);
        a.restamp_pending(100);

        let (_, taken) = studio_visit(&a, &hub, &machine("a")).await.unwrap();

        assert!(taken.cue_points);
        assert_eq!(only_track(&a).cue_points.fade_in_ms, Some(2000));
        assert_eq!(a.outgoing_count(STUDIO_SENDS).unwrap(), 0);
    }

    #[tokio::test]
    async fn a_studio_is_not_changed_by_its_own_save_coming_back() {
        let Some(hub) = test_hub().await else { return };
        let owner = library(&["One"]);
        publish(&owner, &hub, &machine("office"), &mut false)
            .await
            .unwrap();
        let a = studio(&hub, "a").await;
        fade_in(&a, 1000);

        let visit = pull(&a, &hub, &machine("a")).await.unwrap();

        assert!(
            matches!(
                visit,
                Visit::Pulled {
                    sent: 1,
                    applied: 0,
                    ..
                }
            ),
            "{visit:?}"
        );
    }

    #[tokio::test]
    async fn work_sent_for_a_track_already_purged_is_not_kept() {
        let Some(hub) = test_hub().await else { return };
        let owner = library(&["One"]);
        let office = machine("office");
        publish(&owner, &hub, &office, &mut false).await.unwrap();
        let a = studio(&hub, "a").await;
        let id = only_track(&owner).id;
        owner
            .reconcile(&crate::library::db::Reconcile {
                gone: vec![id],
                now_ms: 5,
                ..Default::default()
            })
            .unwrap();
        owner.purge_tracks(&[id]).unwrap();
        publish(&owner, &hub, &office, &mut true).await.unwrap();
        fade_in(&a, 1000);

        pull(&a, &hub, &machine("a")).await.unwrap();

        let left = query(&hub, "SELECT kind FROM hub_rows WHERE kind <> 'track'").await;
        assert!(left.is_empty(), "{} rows", left.len());
        assert_eq!(a.outgoing_count(STUDIO_SENDS).unwrap(), 0);
    }

    #[tokio::test]
    async fn a_purge_takes_the_tracks_operator_work_out_of_the_hub() {
        let Some(hub) = test_hub().await else { return };
        let owner = library(&["One"]);
        let office = machine("office");
        publish(&owner, &hub, &office, &mut false).await.unwrap();
        let a = studio(&hub, "a").await;
        fade_in(&a, 1000);
        pull(&a, &hub, &machine("a")).await.unwrap();
        let id = only_track(&owner).id;
        owner
            .reconcile(&crate::library::db::Reconcile {
                gone: vec![id],
                now_ms: 5,
                ..Default::default()
            })
            .unwrap();
        owner.purge_tracks(&[id]).unwrap();

        publish(&owner, &hub, &office, &mut true).await.unwrap();

        let left = query(&hub, "SELECT kind FROM hub_rows WHERE kind <> 'track'").await;
        assert!(left.is_empty(), "{} rows", left.len());
    }

    #[tokio::test]
    async fn the_owner_is_not_handed_back_what_it_published() {
        let Some(hub) = test_hub().await else { return };
        let owner = library(&["One", "Two"]);

        let (_, taken) = owner_visit(&owner, &hub, &machine("office"), &mut false)
            .await
            .unwrap();

        assert_eq!(taken, Applied::default());
        assert_eq!(owner.pulled_rev().unwrap(), 0);
    }

    #[tokio::test]
    async fn a_studio_cannot_send_what_is_the_owners() {
        let Some(hub) = test_hub().await else { return };
        let owner = library(&["One"]);
        publish(&owner, &hub, &machine("office"), &mut false)
            .await
            .unwrap();
        owner.publish_as("again", 1).unwrap();
        let forged = owner.outgoing(&["track"], 10).unwrap();
        assert_eq!(forged.len(), 1);

        let refused = hub
            .connect()
            .await
            .unwrap()
            .send(&machine("a"), &forged, false)
            .await;

        assert!(refused.is_err());
    }
}

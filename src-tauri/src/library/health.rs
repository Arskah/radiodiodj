//! The library health report: missing tracks, duplicates, and what the
//! operator has already dismissed. See `docs/library-health.md`.

use anyhow::{bail, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Listener};

use super::check::CheckReport;
use super::db::{Db, Dismissal, HealthRow, Track};
use super::listing::{self, ScanRoot};
use crate::persist::config::Config;

pub const HEALTH_EVENT: &str = "library-health";

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub missing: Vec<MissingTrack>,
    /// The operator has seen every track in `missing`.
    pub missing_dismissed: bool,
    pub exact: Vec<DuplicateGroup>,
    pub possible: Vec<DuplicateGroup>,
    /// Present tracks not fingerprinted yet, so not in any exact group.
    pub unhashed: i64,
    /// The latest library check, until a scan makes it moot.
    pub check: Option<CheckReport>,
    pub check_dismissed: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MissingTrack {
    pub id: i64,
    pub title: String,
    pub artist: String,
    /// Where the file was last seen.
    pub path: String,
    pub missing_since: i64,
    pub play_count: i64,
    pub has_cue_points: bool,
    /// No configured library path contains `path`: the path was removed
    /// rather than the file.
    pub outside_roots: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub key: String,
    pub dismissed: bool,
    pub tracks: Vec<DuplicateMember>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMember {
    pub track: Track,
    pub path: String,
    pub content_type: String,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FindingKind {
    Exact,
    Possible,
    Missing,
    Check,
}

impl FindingKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Possible => "possible",
            Self::Missing => "missing",
            Self::Check => "check",
        }
    }
}

/// Keeps the latest report and pushes it to the renderer whenever it changes.
pub struct Health {
    db: Arc<Db>,
    config: Arc<Config>,
    app: AppHandle,
    report: Mutex<HealthReport>,
    check: Mutex<Option<CheckReport>>,
    /// Signature of the check report the operator dismissed. Not stored: the
    /// next launch checks again anyway.
    check_dismissed: Mutex<Option<u64>>,
}

impl Health {
    pub fn new(app: AppHandle, db: Arc<Db>, config: Arc<Config>) -> Arc<Self> {
        let health = Arc::new(Self {
            db,
            config,
            app,
            report: Mutex::new(HealthReport::default()),
            check: Mutex::new(None),
            check_dismissed: Mutex::new(None),
        });
        health.refresh();
        health
    }

    /// Rebuild when a scan or the analysis pass changes state: either can
    /// mark, revive or fingerprint tracks.
    pub fn attach_to_app(self: &Arc<Self>, app: &AppHandle) {
        for event in ["scan-state-changed", "waveform-state-changed"] {
            let health = Arc::clone(self);
            app.listen(event, move |_| health.refresh());
        }
    }

    pub fn report(&self) -> HealthReport {
        self.report.lock().clone()
    }

    pub fn refresh(&self) {
        let roots = listing::configured_roots(&self.config);
        match build(&self.db, &roots) {
            Ok(mut report) => {
                let check = self.check.lock().clone();
                report.check_dismissed = check
                    .as_ref()
                    .is_some_and(|c| *self.check_dismissed.lock() == Some(c.signature()));
                report.check = check;
                *self.report.lock() = report.clone();
                let _ = self.app.emit(HEALTH_EVENT, &report);
            }
            Err(e) => log::error!("library health: {e:#}"),
        }
    }

    /// Replace the library check result. `None` once a scan has applied it.
    pub fn set_check(&self, report: Option<CheckReport>) {
        *self.check.lock() = report;
        self.refresh();
    }

    pub fn dismiss(&self, kind: FindingKind, key: &str) -> Result<()> {
        if kind == FindingKind::Check {
            let signature = match self.check.lock().as_ref() {
                Some(check) => check.signature(),
                None => bail!("there is no library check to dismiss"),
            };
            *self.check_dismissed.lock() = Some(signature);
            self.refresh();
            return Ok(());
        }
        let value = {
            let report = self.report.lock();
            match kind {
                FindingKind::Missing => match missing_signature(&report.missing) {
                    Some(value) => value,
                    None => bail!("no missing tracks to dismiss"),
                },
                FindingKind::Check => unreachable!("handled above"),
                FindingKind::Exact | FindingKind::Possible => {
                    let groups = match kind {
                        FindingKind::Exact => &report.exact,
                        _ => &report.possible,
                    };
                    match groups.iter().find(|g| g.key == key) {
                        Some(group) => group_signature(group),
                        None => bail!("that duplicate group no longer exists"),
                    }
                }
            }
        };
        self.db.set_dismissal(&Dismissal {
            kind: kind.as_str().into(),
            key: dismissal_key(kind, key),
            value,
        })?;
        self.refresh();
        Ok(())
    }

    pub fn undismiss(&self, kind: FindingKind, key: &str) -> Result<()> {
        if kind == FindingKind::Check {
            *self.check_dismissed.lock() = None;
            self.refresh();
            return Ok(());
        }
        self.db
            .delete_dismissals(&[(kind.as_str().into(), dismissal_key(kind, key))])?;
        self.refresh();
        Ok(())
    }
}

/// Missing tracks have one dismissal for the whole list.
fn dismissal_key(kind: FindingKind, key: &str) -> String {
    match kind {
        FindingKind::Missing => String::new(),
        _ => key.to_string(),
    }
}

/// Build the report, and forget dismissals whose finding is gone.
pub fn build(db: &Db, roots: &[ScanRoot]) -> Result<HealthReport> {
    let missing: Vec<MissingTrack> = db
        .missing_tracks()?
        .into_iter()
        .map(|row| MissingTrack {
            outside_roots: !roots
                .iter()
                .any(|root| Path::new(&row.path).starts_with(&root.path)),
            id: row.id,
            title: row.title,
            artist: row.artist,
            path: row.path,
            missing_since: row.missing_since,
            play_count: row.play_count,
            has_cue_points: row.has_cue_points,
        })
        .collect();
    let mut exact = exact_groups(db.fingerprint_twins()?);
    let mut possible = possible_groups(db.tagged_music()?);

    let mut dismissed: HashMap<(String, String), String> = db
        .dismissals()?
        .into_iter()
        .map(|d| ((d.kind, d.key), d.value))
        .collect();
    let mut live: HashSet<(String, String)> = HashSet::new();
    for (kind, groups) in [("exact", &mut exact), ("possible", &mut possible)] {
        for group in groups.iter_mut() {
            let id = (kind.to_string(), group.key.clone());
            group.dismissed = dismissed.get(&id) == Some(&group_signature(group));
            live.insert(id);
        }
    }
    let missing_key = ("missing".to_string(), String::new());
    // Dismissed as long as nothing went missing after the dismissal.
    let missing_dismissed = match (
        dismissed
            .get(&missing_key)
            .and_then(|v| v.parse::<i64>().ok()),
        missing.first(),
    ) {
        (Some(seen), Some(newest)) => newest.missing_since <= seen,
        _ => false,
    };
    if !missing.is_empty() {
        live.insert(missing_key);
    }

    dismissed.retain(|id, _| !live.contains(id));
    if !dismissed.is_empty() {
        let stale: Vec<(String, String)> = dismissed.into_keys().collect();
        db.delete_dismissals(&stale)?;
    }

    Ok(HealthReport {
        missing,
        missing_dismissed,
        exact,
        possible,
        unhashed: db.unhashed_count()?,
        check: None,
        check_dismissed: false,
    })
}

fn group_signature(group: &DuplicateGroup) -> String {
    let mut ids: Vec<i64> = group.tracks.iter().map(|m| m.track.id).collect();
    ids.sort_unstable();
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// `missing` is sorted newest first.
fn missing_signature(missing: &[MissingTrack]) -> Option<String> {
    missing.first().map(|t| t.missing_since.to_string())
}

fn member(row: HealthRow) -> DuplicateMember {
    DuplicateMember {
        track: row.track,
        path: row.path,
        content_type: row.content_type,
    }
}

/// `rows` arrive ordered by fingerprint, so each group is a contiguous run.
fn exact_groups(rows: Vec<HealthRow>) -> Vec<DuplicateGroup> {
    let mut groups: Vec<DuplicateGroup> = Vec::new();
    for row in rows {
        let key = row.fingerprint.clone().unwrap_or_default();
        match groups.last_mut() {
            Some(group) if group.key == key => group.tracks.push(member(row)),
            _ => groups.push(DuplicateGroup {
                key,
                dismissed: false,
                tracks: vec![member(row)],
            }),
        }
    }
    sort_groups(&mut groups);
    groups
}

/// Group tracks by normalised artist and title. A group whose tracks all share
/// one fingerprint is an exact group and is left to that list.
fn possible_groups(rows: Vec<HealthRow>) -> Vec<DuplicateGroup> {
    let mut by_key: HashMap<String, Vec<HealthRow>> = HashMap::new();
    for row in rows {
        let artist = normalise(&row.track.artist);
        let title = normalise(&row.track.title);
        // The scanner fills an untagged file's artist with "Unknown".
        if artist.is_empty() || title.is_empty() || artist == "unknown" {
            continue;
        }
        by_key
            .entry(format!("{artist}\u{1f}{title}"))
            .or_default()
            .push(row);
    }
    let mut groups: Vec<DuplicateGroup> = by_key
        .into_iter()
        .filter(|(_, rows)| rows.len() > 1)
        .filter(|(_, rows)| {
            let first = &rows[0].fingerprint;
            first.is_none() || rows.iter().any(|r| &r.fingerprint != first)
        })
        .map(|(key, rows)| DuplicateGroup {
            key,
            dismissed: false,
            tracks: rows.into_iter().map(member).collect(),
        })
        .collect();
    sort_groups(&mut groups);
    groups
}

fn sort_groups(groups: &mut [DuplicateGroup]) {
    groups.sort_by_cached_key(|g| {
        let first = &g.tracks[0].track;
        (normalise(&first.artist), normalise(&first.title), first.id)
    });
}

/// Lower-case, and collapse every run of anything but letters and digits into
/// one space. Words are kept, so "(Remix)" still tells two titles apart.
pub fn normalise(s: &str) -> String {
    let lower = s.to_lowercase();
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::cue_points::CuePoints;
    use crate::library::db::{Reconcile, TrackInsert};

    fn insert(
        db: &Db,
        path: &str,
        content_type: &str,
        artist: &str,
        title: &str,
        fp: Option<&str>,
    ) -> i64 {
        db.insert_track(&TrackInsert {
            path: path.into(),
            content_type: content_type.into(),
            title: Some(title.into()),
            artist: Some(artist.into()),
            album: Some("Album".into()),
            duration: Some(200.0),
            mtime: Some(1),
            fingerprint: fp.map(Into::into),
            ..Default::default()
        })
        .unwrap();
        db.track_index()
            .unwrap()
            .into_iter()
            .find(|r| r.path == path)
            .unwrap()
            .id
    }

    fn mark_missing(db: &Db, ids: &[i64], now_ms: i64) {
        db.reconcile(&Reconcile {
            gone: ids.to_vec(),
            now_ms,
            ..Default::default()
        })
        .unwrap();
    }

    fn music_root() -> Vec<ScanRoot> {
        vec![ScanRoot {
            content_type: "music",
            path: "/music".into(),
        }]
    }

    fn ids(group: &DuplicateGroup) -> Vec<i64> {
        group.tracks.iter().map(|m| m.track.id).collect()
    }

    #[test]
    fn normalise_folds_case_and_punctuation_but_keeps_words() {
        assert_eq!(normalise("  Don't   Stop!! "), "don t stop");
        assert_eq!(normalise("DON'T STOP"), "don t stop");
        assert_eq!(normalise("Björk"), "björk");
        assert_eq!(normalise("Song (Remix)"), "song remix");
        assert_ne!(normalise("Song (Remix)"), normalise("Song"));
        assert_eq!(normalise("?!"), "");
    }

    #[test]
    fn present_tracks_sharing_a_fingerprint_form_one_exact_group() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", Some("v1:1"));
        let b = insert(&db, "/music/b.mp3", "jingle", "Y", "Two", Some("v1:1"));
        insert(&db, "/music/c.mp3", "music", "Z", "Three", Some("v1:2"));
        insert(&db, "/music/d.mp3", "music", "W", "Four", None);
        let report = build(&db, &music_root()).unwrap();
        assert_eq!(report.exact.len(), 1);
        assert_eq!(report.exact[0].key, "v1:1");
        assert_eq!(ids(&report.exact[0]), vec![a, b]);
        assert_eq!(report.unhashed, 1);
    }

    #[test]
    fn a_missing_copy_is_not_a_duplicate() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/music/a.mp3", "music", "X", "One", Some("v1:1"));
        let b = insert(&db, "/music/b.mp3", "music", "X", "One", Some("v1:1"));
        mark_missing(&db, &[b], 10);
        let report = build(&db, &music_root()).unwrap();
        assert!(report.exact.is_empty());
        assert!(report.possible.is_empty());
    }

    #[test]
    fn music_with_the_same_artist_and_title_is_a_possible_duplicate() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(
            &db,
            "/music/a.mp3",
            "music",
            "The Band",
            "Song",
            Some("v1:1"),
        );
        let b = insert(
            &db,
            "/music/b.flac",
            "music",
            "the band",
            "Song!",
            Some("v1:2"),
        );
        let c = insert(&db, "/music/c.mp3", "music", "THE BAND", "song", None);
        insert(
            &db,
            "/music/d.mp3",
            "music",
            "The Band",
            "Song (Remix)",
            Some("v1:3"),
        );
        let report = build(&db, &music_root()).unwrap();
        assert_eq!(report.possible.len(), 1);
        assert_eq!(ids(&report.possible[0]), vec![a, b, c]);
        assert!(report.exact.is_empty());
    }

    #[test]
    fn possible_duplicates_leave_out_station_material_and_untagged_files() {
        let db = Db::open_in_memory().unwrap();
        insert(
            &db,
            "/music/j1.mp3",
            "jingle",
            "Station",
            "ID",
            Some("v1:1"),
        );
        insert(
            &db,
            "/music/j2.mp3",
            "jingle",
            "Station",
            "ID",
            Some("v1:2"),
        );
        insert(
            &db,
            "/music/u1.mp3",
            "music",
            "Unknown",
            "Track",
            Some("v1:3"),
        );
        insert(
            &db,
            "/music/u2.mp3",
            "music",
            "Unknown",
            "Track",
            Some("v1:4"),
        );
        insert(&db, "/music/e1.mp3", "music", "", "Blank", Some("v1:5"));
        insert(&db, "/music/e2.mp3", "music", "", "Blank", Some("v1:6"));
        let report = build(&db, &music_root()).unwrap();
        assert!(report.possible.is_empty());
    }

    #[test]
    fn a_title_group_that_is_already_exact_is_listed_once() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/music/a.mp3", "music", "X", "One", Some("v1:1"));
        insert(&db, "/music/b.mp3", "music", "X", "One", Some("v1:1"));
        let report = build(&db, &music_root()).unwrap();
        assert_eq!(report.exact.len(), 1);
        assert!(report.possible.is_empty());
    }

    #[test]
    fn missing_tracks_are_listed_newest_first_with_what_a_purge_loses() {
        let db = Db::open_in_memory().unwrap();
        let old = insert(&db, "/music/old.mp3", "music", "X", "Old", None);
        let new = insert(&db, "/gone-root/new.mp3", "music", "Y", "New", None);
        insert(&db, "/music/here.mp3", "music", "Z", "Here", None);
        db.set_cue_points(
            old,
            CuePoints {
                cue_in_ms: Some(500),
                ..Default::default()
            },
        )
        .unwrap();
        db.increment_play_count(old).unwrap();
        mark_missing(&db, &[old], 10);
        mark_missing(&db, &[new], 20);
        let report = build(&db, &music_root()).unwrap();
        let listed: Vec<(i64, i64, bool, i64, bool)> = report
            .missing
            .iter()
            .map(|m| {
                (
                    m.id,
                    m.missing_since,
                    m.has_cue_points,
                    m.play_count,
                    m.outside_roots,
                )
            })
            .collect();
        assert_eq!(
            listed,
            vec![(new, 20, false, 0, true), (old, 10, true, 1, false)]
        );
    }

    fn dismiss(db: &Db, kind: FindingKind, key: &str, value: &str) {
        db.set_dismissal(&Dismissal {
            kind: kind.as_str().into(),
            key: key.into(),
            value: value.into(),
        })
        .unwrap();
    }

    #[test]
    fn a_dismissed_group_lights_again_when_its_members_change() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/music/a.mp3", "music", "X", "One", Some("v1:1"));
        insert(&db, "/music/b.mp3", "music", "Y", "Two", Some("v1:1"));
        let group = build(&db, &music_root()).unwrap().exact.remove(0);
        dismiss(
            &db,
            FindingKind::Exact,
            &group.key,
            &group_signature(&group),
        );
        assert!(build(&db, &music_root()).unwrap().exact[0].dismissed);

        insert(&db, "/music/c.mp3", "music", "Z", "Three", Some("v1:1"));
        let report = build(&db, &music_root()).unwrap();
        assert_eq!(report.exact[0].tracks.len(), 3);
        assert!(!report.exact[0].dismissed);
    }

    #[test]
    fn a_dismissal_is_forgotten_once_its_group_is_gone() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/music/a.mp3", "music", "X", "One", Some("v1:1"));
        let b = insert(&db, "/music/b.mp3", "music", "Y", "Two", Some("v1:1"));
        let group = build(&db, &music_root()).unwrap().exact.remove(0);
        dismiss(
            &db,
            FindingKind::Exact,
            &group.key,
            &group_signature(&group),
        );
        dismiss(&db, FindingKind::Possible, "nobody\u{1f}nothing", "1,2");

        mark_missing(&db, &[b], 10);
        build(&db, &music_root()).unwrap();
        assert!(db.dismissals().unwrap().is_empty());
    }

    #[test]
    fn dismissed_missing_tracks_light_again_when_another_goes_missing() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", None);
        let b = insert(&db, "/music/b.mp3", "music", "Y", "Two", None);
        mark_missing(&db, &[a], 10);
        dismiss(&db, FindingKind::Missing, "", "10");
        assert!(build(&db, &music_root()).unwrap().missing_dismissed);

        mark_missing(&db, &[b], 20);
        assert!(!build(&db, &music_root()).unwrap().missing_dismissed);
    }

    #[test]
    fn purge_deletes_only_the_chosen_missing_tracks() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", None);
        let b = insert(&db, "/music/b.mp3", "music", "Y", "Two", None);
        let present = insert(&db, "/music/c.mp3", "music", "Z", "Three", None);
        mark_missing(&db, &[a, b], 10);
        let deleted = db.purge_tracks(&[a, present, 999]).unwrap();
        assert_eq!(deleted, vec![a]);
        assert!(db.get_track(a).unwrap().is_none());
        assert!(db.get_track(b).unwrap().is_some());
        assert!(db.get_track(present).unwrap().is_some());
    }
}

//! The library health report: missing tracks, hidden tracks, duplicates, files
//! that cannot be decoded, and what the operator has already dismissed. See `docs/library-health.md`.

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
use super::saved_playlists;
use super::tag_write::{TagWriteFailure, TagWriter};

pub const HEALTH_EVENT: &str = "library-health";

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub missing: Vec<MissingTrack>,
    /// The operator has seen every track in `missing`.
    pub missing_dismissed: bool,
    /// Tracks an admin hid from the library, whose file is still there.
    pub hidden: Vec<HiddenTrack>,
    pub exact: Vec<DuplicateGroup>,
    pub possible: Vec<DuplicateGroup>,
    /// Present tracks still waiting to be fingerprinted, so not in any exact
    /// group yet.
    pub unhashed: i64,
    /// Present tracks the analysis pass could not decode. Never in an exact
    /// group unless their fingerprint was taken before the failure.
    pub unreadable: Vec<UnreadableTrack>,
    /// Present tracks whose tags give a length the audio does not have.
    pub bad_durations: Vec<BadDurationTrack>,
    /// The operator has seen every track in `bad_durations`.
    pub bad_durations_dismissed: bool,
    /// The latest library check, until a scan makes it moot.
    pub check: Option<CheckReport>,
    pub check_dismissed: bool,
    /// A library check is running now.
    pub checking: bool,
    /// Edits that could not be written into their file.
    pub tag_write_failures: Vec<TagWriteFailure>,
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
pub struct HiddenTrack {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub path: String,
    pub content_type: String,
    /// Unix ms.
    pub hidden_at: i64,
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

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UnreadableTrack {
    pub track: Track,
    pub path: String,
    pub content_type: String,
    pub error: String,
    /// Unix ms.
    pub failed_at: i64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BadDurationTrack {
    pub track: Track,
    pub path: String,
    pub content_type: String,
    /// Seconds, as the tags claim. `None` when they carry no length.
    pub tag_duration: Option<f64>,
    /// Seconds, as the decode counted. `None` when it counted nothing.
    pub measured_duration: Option<f64>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FindingKind {
    Exact,
    Possible,
    Missing,
    Duration,
    Check,
}

impl FindingKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Possible => "possible",
            Self::Missing => "missing",
            Self::Duration => "duration",
            Self::Check => "check",
        }
    }
}

/// Keeps the latest report and pushes it to the renderer whenever it changes.
pub struct Health {
    db: Arc<Db>,
    tag_writer: Arc<TagWriter>,
    app: AppHandle,
    report: Mutex<HealthReport>,
    check: Mutex<Option<CheckReport>>,
    /// Signature of the check report the operator dismissed. Not stored: the
    /// next launch checks again anyway.
    check_dismissed: Mutex<Option<u64>>,
    checking: Mutex<bool>,
    emits: Emits,
}

/// Keeps the last `library-health` event a listener receives the last report
/// stored. A caller stamps a sequence while it still holds the report lock, so
/// sequence order is store order; `send` holds `sent` across the emit, so two
/// emits cannot interleave between the check and the send and the one that lost
/// the race is dropped rather than delivered after a newer report.
///
/// The report lock itself is never held across the emit: `HEALTH_EVENT` has a
/// backend listener too (`playlist::service`), which Tauri runs on the emitting
/// thread. `sent` is held there on purpose — serialising the delivery is the
/// guarantee, and releasing it before the emit would let two emits reorder
/// again — so nothing a `HEALTH_EVENT` listener reaches may call back into
/// [`Health::refresh`]: `parking_lot`'s mutexes are not reentrant.
#[derive(Default)]
struct Emits {
    next: Mutex<u64>,
    sent: Mutex<u64>,
}

impl Emits {
    /// Take the next sequence. Called with the report lock held.
    fn stamp(&self) -> u64 {
        let mut next = self.next.lock();
        *next += 1;
        *next
    }

    /// Emit unless a newer report has already gone out.
    fn send(&self, seq: u64, emit: impl FnOnce()) {
        let mut sent = self.sent.lock();
        if seq <= *sent {
            return;
        }
        *sent = seq;
        emit();
    }
}

impl Health {
    pub fn new(app: AppHandle, db: Arc<Db>, tag_writer: Arc<TagWriter>) -> Arc<Self> {
        let health = Arc::new(Self {
            db,
            tag_writer,
            app,
            report: Mutex::new(HealthReport::default()),
            check: Mutex::new(None),
            check_dismissed: Mutex::new(None),
            checking: Mutex::new(false),
            emits: Emits::default(),
        });
        let weak = Arc::downgrade(&health);
        health.tag_writer.set_listener(move || {
            if let Some(health) = weak.upgrade() {
                health.refresh();
            }
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
        let roots = listing::configured_roots(&self.db);
        match build(&self.db, &roots) {
            Ok(mut report) => {
                let check = self.check.lock().clone();
                report.check_dismissed = check
                    .as_ref()
                    .is_some_and(|c| *self.check_dismissed.lock() == Some(c.signature()));
                report.check = check;
                report.tag_write_failures = self.tag_writer.failures();
                let seq = {
                    // Under the report lock, so a check starting or ending
                    // meanwhile cannot be overwritten with a stale flag.
                    let mut stored = self.report.lock();
                    report.checking = *self.checking.lock();
                    *stored = report.clone();
                    self.emits.stamp()
                };
                self.emits.send(seq, || {
                    let _ = self.app.emit(HEALTH_EVENT, &report);
                });
                // A saved playlist's missing count moves with the same things
                // this report does, and so does what its entries can bind to.
                saved_playlists::refresh(&self.app, &self.db);
            }
            Err(e) => log::error!("library health: {e:#}"),
        }
    }

    /// Replace the library check result. `None` once a scan has applied it.
    pub fn set_check(&self, report: Option<CheckReport>) {
        *self.check.lock() = report;
        self.refresh();
    }

    /// Say whether a library check is running. Re-sends the current report
    /// rather than rebuilding it.
    pub fn set_checking(&self, checking: bool) {
        let (seq, report) = {
            let mut report = self.report.lock();
            *self.checking.lock() = checking;
            if report.checking == checking {
                return;
            }
            report.checking = checking;
            (self.emits.stamp(), report.clone())
        };
        self.emits.send(seq, || {
            let _ = self.app.emit(HEALTH_EVENT, &report);
        });
    }

    /// Finish a check: store its result, if any, and clear `checking` in the
    /// same report.
    pub fn finish_check(&self, report: Option<CheckReport>) {
        *self.checking.lock() = false;
        match report {
            Some(report) => self.set_check(Some(report)),
            None => self.set_checking(false),
        }
    }

    /// The signature of the check report the operator dismissed, if any. An
    /// automatic scan asks before acting: a dismissed report is them saying not
    /// these changes, and automation does not overrule that.
    pub fn check_dismissed(&self) -> Option<u64> {
        *self.check_dismissed.lock()
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
                FindingKind::Duration => match duration_signature(&report.bad_durations) {
                    Some(value) => value,
                    None => bail!("no bad durations to dismiss"),
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

/// Missing tracks and bad durations have one dismissal for the whole list.
fn dismissal_key(kind: FindingKind, key: &str) -> String {
    match kind {
        FindingKind::Missing | FindingKind::Duration => String::new(),
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

    let bad_durations: Vec<BadDurationTrack> = db
        .bad_duration_tracks()?
        .into_iter()
        .map(|b| BadDurationTrack {
            track: b.row.track,
            path: b.row.path,
            content_type: b.row.content_type,
            tag_duration: b.tag_duration,
            measured_duration: b.measured_duration,
        })
        .collect();
    let duration_key = ("duration".to_string(), String::new());
    // Dismissed as long as every listed finding was listed, with the same two
    // lengths, when the operator dismissed. A file repaired since then leaves
    // the rest dismissed; a new one, or one whose lengths moved, does not.
    let bad_durations_dismissed = !bad_durations.is_empty()
        && dismissed.get(&duration_key).is_some_and(|seen| {
            let seen: HashSet<&str> = seen.split(',').collect();
            bad_durations
                .iter()
                .all(|b| seen.contains(duration_entry(b).as_str()))
        });
    if !bad_durations.is_empty() {
        live.insert(duration_key);
    }

    dismissed.retain(|id, _| !live.contains(id));
    if !dismissed.is_empty() {
        let stale: Vec<(String, String)> = dismissed.into_keys().collect();
        db.delete_dismissals(&stale)?;
    }

    Ok(HealthReport {
        missing,
        missing_dismissed,
        hidden: db
            .hidden_tracks()?
            .into_iter()
            .map(|row| HiddenTrack {
                id: row.id,
                title: row.title,
                artist: row.artist,
                album: row.album,
                path: row.path,
                content_type: row.content_type,
                hidden_at: row.hidden_at,
            })
            .collect(),
        exact,
        possible,
        unhashed: db.unhashed_count()?,
        unreadable: db
            .unreadable_tracks()?
            .into_iter()
            .map(|u| UnreadableTrack {
                track: u.row.track,
                path: u.row.path,
                content_type: u.row.content_type,
                error: u.error,
                failed_at: u.failed_at,
            })
            .collect(),
        bad_durations,
        bad_durations_dismissed,
        check: None,
        check_dismissed: false,
        checking: false,
        tag_write_failures: Vec::new(),
    })
}

fn group_signature(group: &DuplicateGroup) -> String {
    let mut ids: Vec<i64> = group.tracks.iter().map(|m| m.track.id).collect();
    ids.sort_unstable();
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// `missing` is sorted newest first.
/// One bad duration as a dismissal remembers it: the track and both lengths in
/// whole milliseconds, so a finding that changed reads as a different one.
fn duration_entry(bad: &BadDurationTrack) -> String {
    let ms = |seconds: Option<f64>| {
        seconds.map_or_else(
            || "-".to_string(),
            |s| ((s * 1000.0).round() as i64).to_string(),
        )
    };
    format!(
        "{}:{}:{}",
        bad.track.id,
        ms(bad.tag_duration),
        ms(bad.measured_duration)
    )
}

fn duration_signature(bad: &[BadDurationTrack]) -> Option<String> {
    if bad.is_empty() {
        return None;
    }
    Some(bad.iter().map(duration_entry).collect::<Vec<_>>().join(","))
}

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

/// Group tracks by normalised artist and title, whatever album they are on,
/// then join the groups one typo apart: the same artist under two [`near`]
/// titles, or the same title under two near artists. A group whose tracks all
/// share one fingerprint is an exact group and is left to that list.
fn possible_groups(rows: Vec<HealthRow>) -> Vec<DuplicateGroup> {
    let mut by_name: HashMap<(String, String), Vec<HealthRow>> = HashMap::new();
    for row in rows {
        let artist = normalise(&row.track.artist);
        let title = normalise(&row.track.title);
        if artist.is_empty() || title.is_empty() || artist == UNKNOWN {
            continue;
        }
        by_name.entry((artist, title)).or_default().push(row);
    }
    let mut names: Vec<((String, String), Vec<HealthRow>)> = by_name.into_iter().collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));

    let mut by_artist: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_title: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, ((artist, title), _)) in names.iter().enumerate() {
        by_artist.entry(artist).or_default().push(i);
        by_title.entry(title).or_default().push(i);
    }
    // Each name points at an earlier one it was joined to, or at itself.
    let mut parent: Vec<usize> = (0..names.len()).collect();
    let root = |parent: &[usize], mut i: usize| {
        while parent[i] != i {
            i = parent[i];
        }
        i
    };
    let mut join = |siblings: &[usize], fields: Vec<Squashed>| {
        for (x, a) in fields.iter().enumerate() {
            for (y, b) in fields.iter().enumerate().skip(x + 1) {
                if near(a, b) {
                    let (ra, rb) = (root(&parent, siblings[x]), root(&parent, siblings[y]));
                    parent[ra.max(rb)] = ra.min(rb);
                }
            }
        }
    };
    for siblings in by_artist.values().filter(|s| s.len() > 1) {
        let titles = siblings.iter().map(|&i| Squashed::new(&names[i].0 .1));
        join(siblings, titles.collect());
    }
    for siblings in by_title.values().filter(|s| s.len() > 1) {
        let artists = siblings.iter().map(|&i| Squashed::new(&names[i].0 .0));
        join(siblings, artists.collect());
    }

    let roots: Vec<usize> = (0..names.len()).map(|i| root(&parent, i)).collect();
    let mut clusters: HashMap<usize, Vec<HealthRow>> = HashMap::new();
    let mut keys: HashMap<usize, String> = HashMap::new();
    for (i, ((artist, title), rows)) in names.into_iter().enumerate() {
        if roots[i] == i {
            keys.insert(i, format!("{artist}\u{1f}{title}"));
        }
        clusters.entry(roots[i]).or_default().extend(rows);
    }
    let mut groups: Vec<DuplicateGroup> = Vec::new();
    for (i, mut rows) in clusters {
        let first = &rows[0].fingerprint;
        if rows.len() < 2 || (first.is_some() && rows.iter().all(|r| &r.fingerprint == first)) {
            continue;
        }
        rows.sort_by_key(|r| r.track.id);
        groups.push(DuplicateGroup {
            key: keys.remove(&i).unwrap_or_default(),
            dismissed: false,
            tracks: rows.into_iter().map(member).collect(),
        });
    }
    sort_groups(&mut groups);
    groups
}

/// A normalised name beside itself with the spaces taken out, which is how two
/// spellings of one compound (`dope man`, `dopeman`) compare equal.
struct Squashed<'a> {
    name: &'a str,
    squashed: String,
    chars: usize,
}

impl<'a> Squashed<'a> {
    fn new(name: &'a str) -> Self {
        let squashed: String = name.chars().filter(|c| *c != ' ').collect();
        let chars = squashed.chars().count();
        Self {
            name,
            squashed,
            chars,
        }
    }
}

/// The shortest word a typo is looked for in. Below it one edit is as likely a
/// different word: `i` and `ii`, `mix` and `remix`.
const TYPO_WORD_MIN: usize = 4;
/// The shortest name, spaces aside, whose word breaks may differ.
const TYPO_NAME_MIN: usize = 5;

/// Whether two different normalised names are one name with a typo: the same
/// words but for one of them, which is one edit away, or the same letters but
/// for one edit when the words are split differently, unless the edit is a
/// whole word. A transposition is one edit. Never when the edit touches a number, which tells parts and years
/// apart.
fn near(a: &Squashed, b: &Squashed) -> bool {
    if a.chars.abs_diff(b.chars) > 1 {
        return false;
    }
    let (words_a, words_b) = (a.name.split(' '), b.name.split(' '));
    let one_edit = if words_a.clone().count() == words_b.clone().count() {
        let mut differing = words_a.zip(words_b).filter(|(x, y)| x != y);
        match (differing.next(), differing.next()) {
            (Some((x, y)), None) => {
                x.chars().count().min(y.chars().count()) >= TYPO_WORD_MIN
                    && strsim::osa_distance(x, y) <= 1
            }
            _ => false,
        }
    } else {
        a.chars.min(b.chars) >= TYPO_NAME_MIN
            && strsim::osa_distance(&a.squashed, &b.squashed) <= 1
            && !one_word_more(a.name, b.name)
    };
    one_edit && numbers(a.name).eq(numbers(b.name))
}

/// Whether one name is the other with a whole word added: `believe` and
/// `i believe` are an edit apart and two titles.
fn one_word_more(a: &str, b: &str) -> bool {
    let (long, short) = if a.len() > b.len() { (a, b) } else { (b, a) };
    let (long, short): (Vec<&str>, Vec<&str>) =
        (long.split(' ').collect(), short.split(' ').collect());
    long.len() == short.len() + 1
        && (0..long.len()).any(|skip| {
            long.iter()
                .enumerate()
                .filter(|(i, _)| *i != skip)
                .map(|(_, w)| w)
                .eq(short.iter())
        })
}

fn numbers(name: &str) -> impl Iterator<Item = &str> {
    name.split(|c: char| !c.is_numeric())
        .filter(|run| !run.is_empty())
}

/// What the scanner fills in for a missing artist tag, normalised.
const UNKNOWN: &str = "unknown";

fn sort_groups(groups: &mut [DuplicateGroup]) {
    groups.sort_by_cached_key(|g| {
        let first = &g.tracks[0].track;
        (
            normalise(&first.artist),
            normalise(&first.title),
            normalise(&first.album),
            first.id,
        )
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
    use crate::library::db::{Reconcile, SelectionFilter, TrackInsert};

    fn insert(
        db: &Db,
        path: &str,
        content_type: &str,
        artist: &str,
        title: &str,
        fp: Option<&str>,
    ) -> i64 {
        insert_on(db, path, content_type, artist, "Album", title, fp)
    }

    fn insert_on(
        db: &Db,
        path: &str,
        content_type: &str,
        artist: &str,
        album: &str,
        title: &str,
        fp: Option<&str>,
    ) -> i64 {
        db.insert_track(&TrackInsert {
            path: path.into(),
            content_type: content_type.into(),
            title: Some(title.into()),
            artist: Some(artist.into()),
            album: Some(album.into()),
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
    fn an_emit_that_lost_the_race_is_dropped() {
        let emits = Emits::default();
        let first = emits.stamp();
        let second = emits.stamp();
        let sent = Mutex::new(Vec::new());
        // The newer report reaches the renderer first: the older one is what a
        // slower refresh would otherwise deliver last.
        emits.send(second, || sent.lock().push(second));
        emits.send(first, || sent.lock().push(first));
        assert_eq!(*sent.lock(), vec![second]);
    }

    #[test]
    fn concurrent_refreshes_emit_in_store_order() {
        let emits = Emits::default();
        let sent = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let (emits, sent) = (&emits, &sent);
                scope.spawn(move || {
                    for _ in 0..50 {
                        let seq = emits.stamp();
                        emits.send(seq, || sent.lock().push(seq));
                    }
                });
            }
        });
        let sent = sent.lock();
        assert!(sent.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(sent.last().copied(), Some(*emits.next.lock()));
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
    fn a_track_that_cannot_be_decoded_is_unreadable_not_waiting() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", None);
        insert(&db, "/music/b.mp3", "music", "Y", "Two", None);
        assert!(db
            .set_analysis_failed(a, "fingerprint: probe: unsupported", 7, Some(1))
            .unwrap());
        let report = build(&db, &music_root()).unwrap();
        assert_eq!(report.unhashed, 1);
        assert_eq!(report.unreadable.len(), 1);
        let u = &report.unreadable[0];
        assert_eq!(u.track.id, a);
        assert_eq!(u.path, "/music/a.mp3");
        assert_eq!(u.content_type, "music");
        assert_eq!(u.error, "fingerprint: probe: unsupported");
        assert_eq!(u.failed_at, 7);
    }

    #[test]
    fn a_tag_length_the_audio_contradicts_is_a_bad_duration() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", None);
        let b = insert(&db, "/music/b.mp3", "music", "Y", "Two", None);
        let tag = db.get_track(a).unwrap().unwrap().duration;
        let ms = (tag * 1000.0) as i64;
        assert!(db
            .set_measured_duration(a, ms + 60_000, 7, Some(1))
            .unwrap());
        assert!(db.set_measured_duration(b, ms, 7, Some(1)).unwrap());
        let report = build(&db, &music_root()).unwrap();
        assert_eq!(report.bad_durations.len(), 1);
        let bad = &report.bad_durations[0];
        assert_eq!(bad.track.id, a);
        assert_eq!(bad.path, "/music/a.mp3");
        assert_eq!(bad.tag_duration, Some(tag));
        assert_eq!(bad.measured_duration, Some(tag + 60.0));
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
    fn the_same_title_on_different_albums_is_a_possible_duplicate() {
        let db = Db::open_in_memory().unwrap();
        let a = insert_on(
            &db,
            "/music/a.mp3",
            "music",
            "X",
            "Album",
            "Song",
            Some("v1:1"),
        );
        let b = insert_on(
            &db,
            "/music/b.mp3",
            "music",
            "X",
            "Best Of",
            "Song",
            Some("v1:2"),
        );
        let c = insert_on(&db, "/music/c.mp3", "music", "X", "", "Song", None);
        let report = build(&db, &music_root()).unwrap();
        assert_eq!(report.possible.len(), 1);
        assert_eq!(ids(&report.possible[0]), vec![a, b, c]);
        assert_eq!(report.possible[0].key, "x\u{1f}song");
    }

    fn squashed_near(a: &str, b: &str) -> bool {
        near(&Squashed::new(a), &Squashed::new(b))
    }

    #[test]
    fn near_is_one_edit_in_one_long_word_or_a_moved_word_break() {
        assert!(squashed_near("african herbman", "african herbsman"));
        assert!(squashed_near("hot in here", "hot in herre"));
        assert!(squashed_near("kraftwerk", "kraftwrek"));
        assert!(squashed_near("dope man", "dopeman"));
        assert!(squashed_near("o le o le saunotaan", "ole ole saunotaan"));
        assert!(squashed_near(
            "lainelautaileva lehmänmaha",
            "lainelautailevan lehmän maha"
        ));

        assert!(!squashed_near("kötinä ii", "kötinä iii"));
        assert!(!squashed_near("one love club mix", "one love club remix"));
        assert!(!squashed_near("älä mee", "älä tee"));
        assert!(!squashed_near("humppatauti", "humppatähti"));
        assert!(!squashed_near("fussin and fightin", "fussing and fighting"));
        assert!(!squashed_near("a b", "ab"));
        assert!(!squashed_near("believe", "i believe"));
        assert!(!squashed_near("song part", "song part i"));
        assert!(!squashed_near("gin juice", "gin n juice"));
    }

    #[test]
    fn near_never_crosses_a_number() {
        assert!(!squashed_near("symphony 15", "symphony 16"));
        assert!(!squashed_near("live 1999", "live 1989"));
        assert!(!squashed_near("part 1", "part1 1"));
        assert!(squashed_near("trench town rock 2", "trenchtown rock 2"));
    }

    #[test]
    fn a_typo_in_the_title_or_the_artist_is_a_possible_duplicate() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(
            &db,
            "/music/a.mp3",
            "music",
            "Eric Clapton",
            "Possession",
            Some("v1:1"),
        );
        let b = insert(
            &db,
            "/music/b.mp3",
            "music",
            "Eric Clapton",
            "Possesion",
            Some("v1:2"),
        );
        let c = insert(
            &db,
            "/music/c.mp3",
            "music",
            "Eric Clapten",
            "Possesion",
            Some("v1:3"),
        );
        insert(
            &db,
            "/music/d.mp3",
            "music",
            "Derek",
            "Possession",
            Some("v1:4"),
        );
        let report = build(&db, &music_root()).unwrap();
        assert_eq!(report.possible.len(), 1);
        assert_eq!(ids(&report.possible[0]), vec![a, b, c]);
        assert_eq!(report.possible[0].key, "eric clapten\u{1f}possesion");
    }

    #[test]
    fn a_typo_of_one_recording_is_left_to_the_exact_list() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, "/music/a.mp3", "music", "X", "Dopeman", Some("v1:1"));
        insert(&db, "/music/b.mp3", "music", "X", "Dope Man", Some("v1:1"));
        let report = build(&db, &music_root()).unwrap();
        assert_eq!(report.exact.len(), 1);
        assert!(report.possible.is_empty());
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
        dismiss(
            &db,
            FindingKind::Possible,
            "nobody\u{1f}\u{1f}nothing",
            "1,2",
        );

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

    /// Two tracks, each with a tag length a minute short of its audio.
    fn two_bad_durations(db: &Db) -> (i64, i64) {
        let a = insert(db, "/music/a.mp3", "music", "X", "One", None);
        let b = insert(db, "/music/b.mp3", "music", "Y", "Two", None);
        for id in [a, b] {
            assert!(db.set_measured_duration(id, 260_000, 7, Some(1)).unwrap());
        }
        (a, b)
    }

    fn dismiss_durations(db: &Db) {
        let report = build(db, &music_root()).unwrap();
        let value = duration_signature(&report.bad_durations).unwrap();
        dismiss(db, FindingKind::Duration, "", &value);
    }

    /// The badge is for news. A file the operator repaired is not news, so the
    /// rest of a dismissed list stays dismissed; a file that was not on it is.
    #[test]
    fn dismissed_bad_durations_light_again_only_for_a_new_one() {
        let db = Db::open_in_memory().unwrap();
        let (a, _) = two_bad_durations(&db);
        assert!(!build(&db, &music_root()).unwrap().bad_durations_dismissed);
        dismiss_durations(&db);
        assert!(build(&db, &music_root()).unwrap().bad_durations_dismissed);

        // `a` is repaired: its measured length now agrees with its tags.
        assert!(db.set_measured_duration(a, 200_000, 8, Some(1)).unwrap());
        let report = build(&db, &music_root()).unwrap();
        assert_eq!(report.bad_durations.len(), 1);
        assert!(report.bad_durations_dismissed, "one fewer is not news");

        let c = insert(&db, "/music/c.mp3", "music", "Z", "Three", None);
        assert!(db.set_measured_duration(c, 260_000, 9, Some(1)).unwrap());
        assert!(!build(&db, &music_root()).unwrap().bad_durations_dismissed);
    }

    /// The same track with a different length is a different finding: the file
    /// was replaced, and what replaced it is wrong in its own way.
    #[test]
    fn a_dismissed_bad_duration_lights_again_when_its_lengths_move() {
        let db = Db::open_in_memory().unwrap();
        let (a, _) = two_bad_durations(&db);
        dismiss_durations(&db);

        assert!(db.set_measured_duration(a, 90_000, 8, Some(1)).unwrap());

        assert!(!build(&db, &music_root()).unwrap().bad_durations_dismissed);
    }

    #[test]
    fn a_duration_dismissal_is_forgotten_once_nothing_is_listed() {
        let db = Db::open_in_memory().unwrap();
        let (a, b) = two_bad_durations(&db);
        dismiss_durations(&db);

        for id in [a, b] {
            assert!(db.set_measured_duration(id, 200_000, 8, Some(1)).unwrap());
        }
        let report = build(&db, &music_root()).unwrap();

        assert!(report.bad_durations.is_empty());
        assert!(!report.bad_durations_dismissed);
        assert!(db.dismissals().unwrap().is_empty());
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

    fn hidden_ids(db: &Db) -> Vec<i64> {
        let report = build(db, &music_root()).unwrap();
        report.hidden.iter().map(|t| t.id).collect()
    }

    fn searchable_ids(db: &Db) -> Vec<i64> {
        let found = db.search("", None, None, None).unwrap();
        found.iter().map(|t| t.id).collect()
    }

    #[test]
    fn a_hidden_track_leaves_the_library_and_its_duplicate_group() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", Some("v1:aa"));
        let b = insert(&db, "/music/comp/a.mp3", "music", "X", "One", Some("v1:aa"));
        assert_eq!(build(&db, &music_root()).unwrap().exact.len(), 1);

        assert_eq!(db.hide_tracks(&[b, 999], 50).unwrap(), vec![b]);

        let report = build(&db, &music_root()).unwrap();
        assert_eq!(hidden_ids(&db), vec![b]);
        assert!(report.exact.is_empty());
        assert!(report.possible.is_empty());
        assert!(report.missing.is_empty(), "hidden is not purgeable");
        assert_eq!(searchable_ids(&db), vec![a]);
        assert_eq!(
            db.search("one", Some("music"), None, None).unwrap().len(),
            1
        );
        assert_eq!(db.get_stats().unwrap().total_tracks, 1);
        let picked = db
            .get_random_tracks("music", 10, &SelectionFilter::default())
            .unwrap();
        assert_eq!(picked.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a]);
        assert!(db.get_track(b).unwrap().is_some(), "the row is kept");
    }

    #[test]
    fn unhiding_puts_a_track_back() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", None);
        db.hide_tracks(&[a], 50).unwrap();
        assert_eq!(db.hide_tracks(&[a], 60).unwrap(), Vec::<i64>::new());

        assert_eq!(db.unhide_tracks(&[a, 999]).unwrap(), vec![a]);

        assert!(hidden_ids(&db).is_empty());
        assert_eq!(searchable_ids(&db), vec![a]);
    }

    /// The scan still owns a hidden row's path: the file is neither inserted
    /// again nor un-hidden by being read again.
    #[test]
    fn a_rescan_keeps_a_hidden_track_hidden() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", Some("v1:aa"));
        db.hide_tracks(&[a], 50).unwrap();

        let again = insert(&db, "/music/a.mp3", "music", "X", "One", Some("v1:aa"));

        assert_eq!(again, a);
        assert_eq!(db.track_index().unwrap().len(), 1);
        assert_eq!(hidden_ids(&db), vec![a]);
    }

    #[test]
    fn a_hidden_track_whose_file_is_gone_is_missing_until_it_returns() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", Some("v1:aa"));
        db.hide_tracks(&[a], 50).unwrap();

        mark_missing(&db, &[a], 100);

        let report = build(&db, &music_root()).unwrap();
        assert!(report.hidden.is_empty());
        assert_eq!(report.missing.len(), 1);

        db.reconcile(&Reconcile {
            new_files: vec![TrackInsert {
                path: "/music/moved/a.mp3".into(),
                content_type: "music".into(),
                mtime: Some(2),
                fingerprint: Some("v1:aa".into()),
                ..Default::default()
            }],
            now_ms: 200,
            ..Default::default()
        })
        .unwrap();

        assert_eq!(hidden_ids(&db), vec![a], "reattached, still hidden");
        assert!(searchable_ids(&db).is_empty());
    }

    #[test]
    fn a_missing_track_cannot_be_hidden() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", None);
        mark_missing(&db, &[a], 100);

        assert!(db.hide_tracks(&[a], 150).unwrap().is_empty());
    }

    /// A duplicate starts with its twin's operator state, but hiding one copy
    /// says nothing about a file that arrives later.
    #[test]
    fn a_new_copy_of_hidden_audio_is_not_hidden() {
        let db = Db::open_in_memory().unwrap();
        let a = insert(&db, "/music/a.mp3", "music", "X", "One", Some("v1:aa"));
        db.hide_tracks(&[a], 50).unwrap();

        db.reconcile(&Reconcile {
            new_files: vec![TrackInsert {
                path: "/music/other/a.mp3".into(),
                content_type: "music".into(),
                title: Some("One".into()),
                artist: Some("X".into()),
                mtime: Some(2),
                fingerprint: Some("v1:aa".into()),
                ..Default::default()
            }],
            now_ms: 200,
            ..Default::default()
        })
        .unwrap();

        assert_eq!(hidden_ids(&db), vec![a]);
        assert_eq!(searchable_ids(&db).len(), 1);
    }
}

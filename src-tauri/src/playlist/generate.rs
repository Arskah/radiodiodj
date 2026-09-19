use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use strum::AsRefStr;

use crate::library::db::{artist_key, Db, SelectionFilter, Track};
use crate::persist::config::TuningConfig;

#[derive(Serialize, Deserialize, AsRefStr, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ContentType {
    Music,
    Jingle,
    Commercial,
}

/// Interleave cadence for a generated block: how often jingles/commercials are
/// woven into the music, and how the commercial pick-from-bottom bucket is
/// sized. Supplied per call from the stored config; clamped on write (see
/// `persist::config::set_tuning`), so the values here are always in range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interleave {
    pub jingle_every: i64,
    pub commercial_every: i64,
    pub commercial_bucket_multiplier: i64,
    pub commercial_bucket_min: i64,
}

impl Interleave {
    /// Read the cadence out of the stored tuning. Values are clamped on write
    /// (see `persist::config::set_tuning`), so they are always in range here.
    pub fn from_config(t: &TuningConfig) -> Self {
        Self {
            jingle_every: t.interleave.jingle_every,
            commercial_every: t.interleave.commercial_every,
            commercial_bucket_multiplier: t.interleave.commercial_bucket_multiplier,
            commercial_bucket_min: t.interleave.commercial_bucket_min,
        }
    }
}

impl Default for Interleave {
    fn default() -> Self {
        Self {
            jingle_every: 4,
            commercial_every: 8,
            commercial_bucket_multiplier: 3,
            commercial_bucket_min: 10,
        }
    }
}

/// How many jingles/commercials to insert into `count` slots at the given
/// cadence. An `every` of 0 disables the content type (no insertions, no
/// divide-by-zero).
fn cadence_count(count: i64, every: i64) -> i64 {
    if every > 0 {
        count / every
    } else {
        0
    }
}

fn commercial_bucket_size(count: i64, il: &Interleave) -> i64 {
    (count * il.commercial_bucket_multiplier).max(il.commercial_bucket_min)
}

/// No-repeat windows for music selection, in minutes of wall clock measured
/// against `play_log.aired_at`. `0` disables a rule, the convention
/// [`cadence_count`] already uses. Clamped on write (see
/// `persist::config::set_tuning`), so the values here are always in range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rotation {
    pub title_window_min: i64,
    pub artist_window_min: i64,
}

impl Rotation {
    pub fn from_config(t: &TuningConfig) -> Self {
        Self {
            title_window_min: t.rotation.title_window_min,
            artist_window_min: t.rotation.artist_window_min,
        }
    }

    /// The instant a window opens, or `None` when the rule is off.
    fn since(window_min: i64, now_ms: i64) -> Option<i64> {
        (window_min > 0).then(|| now_ms - window_min * 60_000)
    }
}

impl Default for Rotation {
    fn default() -> Self {
        Self {
            title_window_min: 180,
            artist_window_min: 45,
        }
    }
}

/// One rung of the relaxation ladder: which rotation rules still apply.
struct Rung {
    title: bool,
    artist: bool,
    /// What gets logged the first time a refill has to step down to this rung.
    dropped: Option<&'static str>,
}

/// Tightest first. Selection must degrade rather than stall: a fresh install
/// with 80 tracks cannot satisfy a three-hour title window.
const LADDER: [Rung; 3] = [
    Rung {
        title: true,
        artist: true,
        dropped: None,
    },
    Rung {
        title: true,
        artist: false,
        dropped: Some("no-repeat artist"),
    },
    Rung {
        title: false,
        artist: false,
        dropped: Some("no-repeat artist and no-repeat title"),
    },
];

const NO_ARTISTS: &[String] = &[];

/// An interleaved block of `count` tracks.
///
/// `queued` is everything already in the playlist. Its ids are excluded from
/// every content type, and — because a queued track has not aired yet and so
/// has no log row — its artists constrain music selection exactly as aired ones
/// do. See `docs/rotation.md`.
pub fn generate(
    db: &Db,
    count: i64,
    queued: &[&Track],
    il: &Interleave,
    rot: &Rotation,
    now_ms: i64,
) -> Result<Vec<Track>> {
    if count <= 0 {
        return Ok(vec![]);
    }

    let jingle_count = cadence_count(count, il.jingle_every);
    let commercial_count = cadence_count(count, il.commercial_every);
    let music_count = (count - jingle_count - commercial_count).max(0);

    let exclude_ids: Vec<i64> = queued.iter().map(|t| t.id).collect();

    // Exclusion happens in SQL (`NOT IN`), so each query returns exactly the
    // requested count of not-yet-queued tracks — no over-fetch or post-filter.
    let music = pick_music(db, music_count, queued, rot, now_ms)?;
    let jingles = db.get_random_tracks(
        ContentType::Jingle.as_ref(),
        jingle_count,
        &SelectionFilter::excluding(&exclude_ids),
    )?;
    let commercials = db.pick_random_from_bottom(
        ContentType::Commercial.as_ref(),
        commercial_count,
        commercial_bucket_size(commercial_count, il),
        &exclude_ids,
    )?;

    Ok(interleave_evenly(music, jingles, commercials))
}

/// The music half of a block, picked one slot at a time under the rotation
/// rules.
///
/// A slot at a time because a single query can only exclude artists it is told
/// about up front: asked for fifteen rows in one go, SQL would happily return
/// the same artist three times inside the block it is supposed to be spreading
/// out. Each pick therefore joins the blocklist for the next one.
///
/// A slot that cannot be filled under both rules steps down [`LADDER`] for
/// itself alone, so only the tail of a block is ever compromised. `exclude_ids`
/// is never relaxed: a duplicate inside the queue is a bug, not a degradation.
fn pick_music(
    db: &Db,
    count: i64,
    queued: &[&Track],
    rot: &Rotation,
    now_ms: i64,
) -> Result<Vec<Track>> {
    if count <= 0 {
        return Ok(vec![]);
    }
    let title_since = Rotation::since(rot.title_window_min, now_ms);
    let artist_since = Rotation::since(rot.artist_window_min, now_ms);

    let mut exclude_ids: Vec<i64> = queued.iter().map(|t| t.id).collect();
    // A blank key would block every untagged track in the library at once.
    let mut artist_keys: Vec<String> = queued
        .iter()
        .map(|t| artist_key(&t.artist))
        .filter(|k| !k.is_empty())
        .collect();
    artist_keys.sort();
    artist_keys.dedup();

    let mut picked: Vec<Track> = Vec::with_capacity(count as usize);
    let mut warned = [false; LADDER.len()];

    while (picked.len() as i64) < count {
        let mut filled = false;
        for (i, rung) in LADDER.iter().enumerate() {
            let filter = SelectionFilter {
                exclude_ids: &exclude_ids,
                title_since: title_since.filter(|_| rung.title),
                artist_since: artist_since.filter(|_| rung.artist),
                artist_keys: if rung.artist {
                    &artist_keys
                } else {
                    NO_ARTISTS
                },
            };
            let Some(track) = db
                .get_random_tracks(ContentType::Music.as_ref(), 1, &filter)?
                .pop()
            else {
                continue;
            };
            if let Some(dropped) = rung.dropped {
                if !warned[i] {
                    warned[i] = true;
                    log::warn!(
                        "auto-playlist: not enough music to honour the {} rule — relaxing it for this refill",
                        dropped
                    );
                }
            }
            exclude_ids.push(track.id);
            let key = artist_key(&track.artist);
            if !key.is_empty() {
                artist_keys.push(key);
            }
            picked.push(track);
            filled = true;
            break;
        }
        if !filled {
            break;
        }
    }

    Ok(picked)
}

pub fn pick_filler(db: &Db, content_type: ContentType, il: &Interleave) -> Result<Option<Track>> {
    match content_type {
        ContentType::Jingle => Ok(db
            .get_random_tracks(ContentType::Jingle.as_ref(), 1, &SelectionFilter::default())?
            .pop()),
        ContentType::Commercial => Ok(db
            .pick_random_from_bottom(
                ContentType::Commercial.as_ref(),
                1,
                commercial_bucket_size(1, il),
                &[],
            )?
            .pop()),
        ContentType::Music => Ok(None),
    }
}

pub fn interleave_evenly(
    music: Vec<Track>,
    jingles: Vec<Track>,
    commercials: Vec<Track>,
) -> Vec<Track> {
    let total = music.len() + jingles.len() + commercials.len();
    if total == 0 {
        return vec![];
    }

    let j_slots = pick_even_slots(total, jingles.len());
    let remaining: Vec<usize> = (0..total).filter(|i| !j_slots.contains(i)).collect();
    let c_indices = pick_even_slots(remaining.len(), commercials.len());
    let c_slots: HashSet<usize> = c_indices.iter().map(|&idx| remaining[idx]).collect();

    let mut music_iter = music.into_iter();
    let mut jingle_iter = jingles.into_iter();
    let mut commercial_iter = commercials.into_iter();
    let mut out = Vec::with_capacity(total);
    for i in 0..total {
        let next = if j_slots.contains(&i) {
            jingle_iter.next()
        } else if c_slots.contains(&i) {
            commercial_iter.next()
        } else {
            music_iter.next()
        };
        if let Some(t) = next {
            out.push(t);
        }
    }
    out
}

fn pick_even_slots(total: usize, count: usize) -> HashSet<usize> {
    let mut slots = HashSet::new();
    if count == 0 || total == 0 {
        return slots;
    }
    let step = total as f64 / count as f64;
    for i in 0..count {
        slots.insert((i as f64 * step + step / 2.0).floor() as usize);
    }
    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track_with(id: i64, artist: &str) -> Track {
        Track {
            artist: artist.into(),
            ..track(id)
        }
    }

    fn track(id: i64) -> Track {
        Track {
            id,
            title: format!("t{}", id),
            artist: "a".into(),
            album: "al".into(),
            duration: 100.0,
            play_count: 0,
            genre: None,
            year: None,
            bpm: None,
            sample_rate: None,
            bitrate: None,
            format: None,
            cue_points: Default::default(),
            edited_fields: 0,
        }
    }

    use crate::library::db::TrackInsert;

    /// Music-only cadence, so a generated block is nothing but the material the
    /// rotation rules govern.
    const MUSIC_ONLY: Interleave = Interleave {
        jingle_every: 0,
        commercial_every: 0,
        commercial_bucket_multiplier: 3,
        commercial_bucket_min: 10,
    };

    const NOW: i64 = 10_000_000;

    /// A library of `(title, artist)` music, inserted in order so ids run 1..n.
    fn library(tracks: &[(&str, &str)]) -> Db {
        let db = Db::open_in_memory().unwrap();
        for (i, (title, artist)) in tracks.iter().enumerate() {
            db.insert_track(&TrackInsert {
                path: format!("/m{i}.mp3"),
                content_type: "music".into(),
                title: Some((*title).into()),
                artist: Some((*artist).into()),
                duration: Some(100.0),
                ..Default::default()
            })
            .unwrap();
        }
        db
    }

    fn block(db: &Db, count: i64, queued: &[&Track], rot: &Rotation) -> Vec<Track> {
        generate(db, count, queued, &MUSIC_ONLY, rot, NOW).unwrap()
    }

    fn ids(tracks: &[Track]) -> Vec<i64> {
        tracks.iter().map(|t| t.id).collect()
    }

    fn artists(tracks: &[Track]) -> Vec<String> {
        tracks.iter().map(|t| t.artist.clone()).collect()
    }

    /// One query asked for the whole block would happily hand back the same
    /// artist three times, so each pick has to constrain the next.
    #[test]
    fn one_block_never_repeats_an_artist() {
        let db = library(&[("1", "A"), ("2", "A"), ("3", "B"), ("4", "C")]);
        let picked = block(&db, 3, &[], &Rotation::default());
        let mut got = artists(&picked);
        got.sort();
        assert_eq!(got, vec!["A", "B", "C"]);
    }

    #[test]
    fn an_artist_already_queued_is_not_selected() {
        let db = library(&[("1", "A"), ("2", "B")]);
        let queued = track_with(1, "A");
        let picked = block(&db, 1, &[&queued], &Rotation::default());
        assert_eq!(artists(&picked), vec!["B"]);
    }

    #[test]
    fn an_airing_inside_the_title_window_is_not_reselected() {
        let db = library(&[("1", "A"), ("2", "B")]);
        db.record_airing(1, NOW - 60_000).unwrap();
        let picked = block(&db, 1, &[], &Rotation::default());
        assert_eq!(ids(&picked), vec![2]);
    }

    #[test]
    fn a_zero_window_disables_the_rule() {
        let db = library(&[("1", "A"), ("2", "B")]);
        db.record_airing(1, NOW - 1_000).unwrap();

        let picked = block(&db, 1, &[], &Rotation::default());
        assert_eq!(ids(&picked), vec![2], "the airing is inside both windows");

        let off = Rotation {
            title_window_min: 0,
            artist_window_min: 0,
        };
        let mut seen: HashSet<i64> = HashSet::new();
        for _ in 0..50 {
            seen.extend(ids(&block(&db, 1, &[], &off)));
        }
        assert_eq!(seen.len(), 2, "both are eligible again: {seen:?}");
    }

    /// The artist rule goes first, so a library with too few artists still
    /// honours the title rule.
    #[test]
    fn the_ladder_drops_the_artist_rule_before_the_title_rule() {
        let db = library(&[("1", "A"), ("2", "A")]);
        db.record_airing(1, NOW - 60_000).unwrap();

        // Only the un-aired track satisfies the title rule, so the second slot
        // needs the title rule gone as well as the artist rule.
        let picked = block(&db, 2, &[], &Rotation::default());
        assert_eq!(ids(&picked), vec![2, 1]);
    }

    /// Relaxation never touches the queue exclusion: a block short of material
    /// comes back short rather than duplicated.
    #[test]
    fn a_block_never_repeats_a_track_however_far_it_relaxes() {
        let db = library(&[("1", "A")]);
        let picked = block(&db, 5, &[], &Rotation::default());
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn jingles_and_commercials_ignore_the_rotation_rules() {
        let db = Db::open_in_memory().unwrap();
        for (i, kind) in ["jingle", "commercial"].iter().enumerate() {
            db.insert_track(&TrackInsert {
                path: format!("/{kind}.mp3"),
                content_type: (*kind).into(),
                title: Some("station".into()),
                artist: Some("Station".into()),
                duration: Some(5.0),
                ..Default::default()
            })
            .unwrap();
            db.record_airing(i as i64 + 1, NOW - 1_000).unwrap();
        }
        let il = Interleave {
            jingle_every: 1,
            commercial_every: 2,
            ..Interleave::default()
        };
        let picked = generate(&db, 2, &[], &il, &Rotation::default(), NOW).unwrap();
        assert_eq!(picked.len(), 2, "both just aired and both came back");
    }

    #[test]
    fn empty_inputs_yield_empty_output() {
        assert!(interleave_evenly(vec![], vec![], vec![]).is_empty());
    }

    #[test]
    fn music_only_passes_through() {
        let m = vec![track(1), track(2), track(3)];
        let r = interleave_evenly(m, vec![], vec![]);
        assert_eq!(r.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn jingles_and_commercials_spread_across_total() {
        let music: Vec<Track> = (1..=12).map(track).collect();
        let jingles: Vec<Track> = (101..=103).map(track).collect();
        let commercials: Vec<Track> = (201..=202).map(track).collect();
        let r = interleave_evenly(music, jingles, commercials);
        assert_eq!(r.len(), 17);
        let jingle_positions: Vec<usize> = r
            .iter()
            .enumerate()
            .filter_map(|(i, t)| (t.id >= 101 && t.id <= 103).then_some(i))
            .collect();
        assert_eq!(jingle_positions.len(), 3);
        // Jingle slots should not be adjacent (spread is the contract).
        for w in jingle_positions.windows(2) {
            assert!(w[1] - w[0] >= 2, "jingles too close: {:?}", w);
        }
    }

    #[test]
    fn music_count_is_clamped_for_small_totals() {
        // count=3 → no jingles, no commercials, 3 music
        let il = Interleave::default();
        let total = 3i64;
        let jingle_count = total / il.jingle_every;
        let commercial_count = total / il.commercial_every;
        assert_eq!(jingle_count, 0);
        assert_eq!(commercial_count, 0);
    }

    #[test]
    fn commercial_bucket_size_respects_multiplier_and_min() {
        let il = Interleave::default(); // x3, min 10
        assert_eq!(commercial_bucket_size(1, &il), 10); // min wins
        assert_eq!(commercial_bucket_size(5, &il), 15); // multiplier wins
        let custom = Interleave {
            commercial_bucket_multiplier: 2,
            commercial_bucket_min: 4,
            ..Interleave::default()
        };
        assert_eq!(commercial_bucket_size(1, &custom), 4);
        assert_eq!(commercial_bucket_size(10, &custom), 20);
    }

    #[test]
    fn cadence_count_zero_disables_without_dividing() {
        assert_eq!(cadence_count(100, 0), 0); // disabled: none inserted
        assert_eq!(cadence_count(100, 4), 25); // normal cadence
        assert_eq!(cadence_count(3, 4), 0); // fewer slots than cadence
    }
}

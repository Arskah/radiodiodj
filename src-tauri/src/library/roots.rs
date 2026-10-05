//! Library paths as the library stores them: a root the database knows by id
//! and content type, and the folder this machine reaches it at, which only
//! `config.json` knows. A track is stored as a root id plus a path relative to
//! that root, so a share that moves to another mount point — or is mounted
//! somewhere else on another machine — is one setting, not a lost library.
//!
//! Everything outside [`Db`] keeps handling absolute paths. The translation is
//! [`Roots::resolve`] on the way out and [`Roots::relativize`] on the way in.
//! See `docs/library.md#library-paths-and-content-types`.

use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::db::Db;
use crate::persist::config::Config;

/// The root id of a row that sits under no library path. Its `path` is
/// absolute.
pub const NO_ROOT: i64 = 0;

/// The content types a library path can feed, in the order paths are listed.
/// The order is a rule: a file under two paths belongs to the first.
pub const CONTENT_TYPES: [&str; 3] = ["music", "commercial", "jingle"];

/// One library path.
#[derive(Clone, Debug, PartialEq)]
pub struct Root {
    pub id: i64,
    pub content_type: &'static str,
    /// Where this machine reaches the root, when it has been told.
    pub mount: Option<String>,
}

impl Root {
    /// The folder the root's tracks resolve under. A root this machine has not
    /// located gets a name no folder can have, so everything that already
    /// handles an unreachable library path — the scan keeps its tracks, the
    /// check reports it, a deck load fails — handles this without knowing.
    pub fn folder(&self) -> String {
        self.mount
            .clone()
            .unwrap_or_else(|| format!("<library path {}, not located>", self.id))
    }
}

/// Every library path, in listing order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Roots(Vec<Root>);

impl Roots {
    /// `rows` as the database holds them, with each root's local folder.
    pub fn new(rows: Vec<(i64, String)>, mounts: &BTreeMap<i64, String>) -> Self {
        let mut roots: Vec<Root> = rows
            .into_iter()
            .filter_map(|(id, content_type)| {
                Some(Root {
                    id,
                    content_type: static_content_type(&content_type)?,
                    mount: mounts.get(&id).cloned(),
                })
            })
            .collect();
        roots.sort_by_key(|r| (type_rank(r.content_type), r.id));
        Self(roots)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Root> {
        self.0.iter()
    }

    pub fn get(&self, id: i64) -> Option<&Root> {
        self.0.iter().find(|r| r.id == id)
    }

    /// The absolute path of a stored `(root_id, path)`.
    pub fn resolve(&self, root_id: i64, path: String) -> String {
        if root_id == NO_ROOT {
            return path;
        }
        let folder = match self.get(root_id) {
            Some(root) => root.folder(),
            // A row whose root is gone from the table. Nothing in the app
            // leaves one behind, so this is a database edited by hand.
            None => format!("<library path {root_id}, removed>"),
        };
        let mut absolute = PathBuf::from(folder);
        absolute.extend(path.split('/'));
        absolute.to_string_lossy().into_owned()
    }

    /// The stored form of an absolute path: the first root it sits under, by
    /// path component, and the path below it with `/` between components
    /// whatever this platform uses. A path under no root is kept whole.
    pub fn relativize(&self, absolute: &str) -> (i64, String) {
        let path = Path::new(absolute);
        for root in &self.0 {
            let Ok(below) = path.strip_prefix(root.folder()) else {
                continue;
            };
            let parts: Vec<_> = below
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect();
            if !parts.is_empty() {
                return (root.id, parts.join("/"));
            }
        }
        (NO_ROOT, absolute.to_owned())
    }
}

fn static_content_type(content_type: &str) -> Option<&'static str> {
    CONTENT_TYPES.into_iter().find(|t| *t == content_type)
}

fn type_rank(content_type: &str) -> usize {
    CONTENT_TYPES
        .iter()
        .position(|t| *t == content_type)
        .unwrap_or(CONTENT_TYPES.len())
}

/// A library path as the settings list shows it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPath {
    pub id: i64,
    /// `None` while this machine has not been told where the path is.
    pub path: Option<String>,
}

/// The settings list: every library path, by content type.
pub fn list(db: &Db) -> BTreeMap<&'static str, Vec<LibraryPath>> {
    let roots = db.roots();
    CONTENT_TYPES
        .into_iter()
        .map(|content_type| {
            let paths = roots
                .iter()
                .filter(|r| r.content_type == content_type)
                .map(|r| LibraryPath {
                    id: r.id,
                    path: r.mount.clone(),
                })
                .collect();
            (content_type, paths)
        })
        .collect()
}

/// Bring the database and `config.json` into step at launch.
///
/// Library paths written by a build that kept them in the config alone — or
/// seeded there by hand — become roots, each taking a root of its type that has
/// no folder yet before a new one is made, so a launch interrupted between the
/// two files does not leave a second set behind. Tracks stored under an
/// absolute path then move under the root that holds them.
pub fn adopt(db: &Db, config: &Config) -> Result<()> {
    let legacy = config.legacy_paths();
    if !legacy.is_empty() {
        let mut mounts = config.mounts();
        for (content_type, dir) in legacy {
            if mounts.values().any(|m| *m == dir) {
                continue;
            }
            let free = db
                .roots()
                .iter()
                .find(|r| r.content_type == content_type && !mounts.contains_key(&r.id))
                .map(|r| r.id);
            let id = match free {
                Some(id) => id,
                None => db.add_root(content_type)?,
            };
            mounts.insert(id, dir);
        }
        config.adopt_mounts(mounts)?;
    }
    let moved = db.set_mounts(&config.mounts())?;
    if moved > 0 {
        log::info!("library: {moved} track(s) moved under their library path");
    }
    Ok(())
}

/// Add `dir` as a library path of `content_type`. False when that type already
/// has it, or the type is not one.
pub fn add(db: &Db, config: &Config, content_type: &str, dir: &str) -> Result<bool> {
    let Some(content_type) = static_content_type(content_type) else {
        return Ok(false);
    };
    let dir = Config::canonical_dir(dir);
    let held = db
        .roots()
        .iter()
        .any(|r| r.content_type == content_type && r.mount.as_deref() == Some(dir.as_str()));
    if held {
        return Ok(false);
    }
    let id = db.add_root(content_type)?;
    config.set_mount(id, &dir)?;
    db.set_mounts(&config.mounts())?;
    Ok(true)
}

/// Tell this machine where library path `id` is. Its tracks follow: nothing in
/// the library names the old folder. False when there is no such path, or
/// another one is already at `dir`.
pub fn locate(db: &Db, config: &Config, id: i64, dir: &str) -> Result<bool> {
    let dir = Config::canonical_dir(dir);
    let roots = db.roots();
    let taken = roots
        .iter()
        .any(|r| r.id != id && r.mount.as_deref() == Some(dir.as_str()));
    if roots.get(id).is_none() || taken {
        return Ok(false);
    }
    config.set_mount(id, &dir)?;
    db.set_mounts(&config.mounts())?;
    Ok(true)
}

/// Remove library path `id`. Its tracks stay, under the absolute path they had,
/// which is what lets the next scan mark them missing and a path added back at
/// the same folder take them up again. False when there is no such path.
pub fn remove(db: &Db, config: &Config, id: i64) -> Result<bool> {
    if !db.remove_root(id)? {
        return Ok(false);
    }
    config.remove_mount(id)?;
    db.set_mounts(&config.mounts())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_measure::test_audio::write_wav;
    use crate::library::db::IndexRow;
    use crate::library::listing::{self, ScanRoot};
    use crate::library::scanner::{scan_all, Missing, ScanOutcome};
    use tempfile::TempDir;

    fn roots(mounts: &[(i64, &str, Option<&str>)]) -> Roots {
        let rows = mounts
            .iter()
            .map(|(id, content_type, _)| (*id, (*content_type).to_owned()))
            .collect();
        let located = mounts
            .iter()
            .filter_map(|(id, _, mount)| Some((*id, (*mount)?.to_owned())))
            .collect();
        Roots::new(rows, &located)
    }

    fn abs(parts: &[&str]) -> String {
        let mut path = PathBuf::from(std::path::MAIN_SEPARATOR_STR);
        path.extend(parts);
        path.to_string_lossy().into_owned()
    }

    /// A library with one file in a music folder, as a build that kept its
    /// paths in the config left it: the row absolute, the folder in
    /// `musicPaths`.
    struct Station {
        dir: TempDir,
        db: Db,
        config: Config,
    }

    impl Station {
        fn legacy() -> Self {
            let dir = TempDir::new().unwrap();
            let music = dir.path().join("music");
            write_wav(&music.join("a.wav"), 1, 1);
            let db = Db::open_in_memory().unwrap();
            let before = [ScanRoot {
                content_type: "music",
                path: Config::canonical_dir(&music.to_string_lossy()),
            }];
            scan_all(&db, &before, Missing::Mark, &|| false, |_, _| {}).unwrap();
            let seeded = serde_json::json!({ "musicPaths": [music] });
            std::fs::write(dir.path().join("config.json"), seeded.to_string()).unwrap();
            let config = Config::open(dir.path()).unwrap();
            Self { dir, db, config }
        }

        fn adopted() -> Self {
            let station = Self::legacy();
            adopt(&station.db, &station.config).unwrap();
            station
        }

        fn scan(&self) -> ScanOutcome {
            let roots = listing::configured_roots(&self.db);
            scan_all(&self.db, &roots, Missing::Mark, &|| false, |_, _| {}).unwrap()
        }

        fn only_row(&self) -> IndexRow {
            let mut rows = self.db.track_index().unwrap();
            assert_eq!(rows.len(), 1, "one file, one row");
            rows.remove(0)
        }

        fn music_id(&self) -> i64 {
            list(&self.db)["music"][0].id
        }

        fn folder(&self, name: &str) -> String {
            let path = self.dir.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            Config::canonical_dir(&path.to_string_lossy())
        }
    }

    #[test]
    fn paths_kept_in_the_config_become_roots_and_their_tracks_move_under_them() {
        let station = Station::legacy();
        let before = station.only_row();

        adopt(&station.db, &station.config).unwrap();

        let paths = list(&station.db);
        assert_eq!(paths["music"].len(), 1);
        assert_eq!(
            paths["music"][0].path.as_deref(),
            Some(station.folder("music").as_str())
        );
        assert!(station.config.legacy_paths().is_empty());
        let after = station.only_row();
        assert_eq!((after.id, &after.path), (before.id, &before.path));
        assert_eq!(station.scan().added, 0);
        assert_eq!(station.only_row().missing_since, None);
    }

    #[test]
    fn taking_up_the_config_twice_makes_no_second_root() {
        let station = Station::adopted();

        adopt(&station.db, &station.config).unwrap();

        assert_eq!(list(&station.db)["music"].len(), 1);
    }

    #[test]
    fn a_launch_interrupted_between_the_two_files_reuses_the_root_it_made() {
        let station = Station::legacy();
        // The database took the root; the config never heard of it.
        let orphan = station.db.add_root("music").unwrap();

        adopt(&station.db, &station.config).unwrap();

        assert_eq!(station.music_id(), orphan);
        assert_eq!(list(&station.db)["music"].len(), 1);
    }

    #[test]
    fn a_folder_that_moves_takes_its_tracks_with_it() {
        let station = Station::adopted();
        let id = station.only_row().id;
        let (old, new) = (
            station.dir.path().join("music"),
            station.dir.path().join("moved"),
        );
        std::fs::rename(&old, &new).unwrap();
        let new = Config::canonical_dir(&new.to_string_lossy());

        assert!(locate(&station.db, &station.config, station.music_id(), &new).unwrap());

        let row = station.only_row();
        assert_eq!(row.id, id);
        assert!(Path::new(&row.path).starts_with(&new), "{}", row.path);
        let outcome = station.scan();
        assert_eq!(
            (outcome.added, outcome.missing, outcome.reattached),
            (0, 0, 0)
        );
        assert_eq!(station.only_row().missing_since, None);
    }

    #[test]
    fn a_path_this_machine_cannot_locate_keeps_its_tracks_through_a_scan() {
        let station = Station::adopted();
        let id = station.only_row().id;
        // The config is lost; the library is not.
        station.config.remove_mount(station.music_id()).unwrap();
        station.db.set_mounts(&station.config.mounts()).unwrap();

        assert_eq!(list(&station.db)["music"][0].path, None);
        assert_eq!(station.scan().missing, 0);
        let row = station.only_row();
        assert_eq!(row.missing_since, None);
        assert!(!Path::new(&row.path).exists());

        let music = station.folder("music");
        assert!(locate(&station.db, &station.config, station.music_id(), &music).unwrap());
        let row = station.only_row();
        assert_eq!(row.id, id);
        assert!(Path::new(&row.path).exists(), "{}", row.path);
    }

    #[test]
    fn a_removed_path_leaves_its_tracks_to_go_missing_and_takes_them_back_when_readded() {
        let station = Station::adopted();
        let before = station.only_row();

        assert!(remove(&station.db, &station.config, station.music_id()).unwrap());

        assert!(list(&station.db)["music"].is_empty());
        assert!(station.config.mounts().is_empty());
        assert_eq!(station.only_row().path, before.path);
        assert_eq!(station.scan().missing, 1);

        let music = station.folder("music");
        assert!(add(&station.db, &station.config, "music", &music).unwrap());
        station.scan();
        let after = station.only_row();
        assert_eq!((after.id, after.missing_since), (before.id, None));
    }

    #[test]
    fn a_root_added_above_another_takes_its_files_without_doubling_them() {
        let station = Station::adopted();
        let id = station.only_row().id;
        let parent = Config::canonical_dir(&station.dir.path().to_string_lossy());
        // Commercial lists before jingle, so make the inner root the later one.
        remove(&station.db, &station.config, station.music_id()).unwrap();
        let music = station.folder("music");
        assert!(add(&station.db, &station.config, "jingle", &music).unwrap());
        station.scan();
        assert_eq!(station.only_row().content_type, "jingle");

        assert!(add(&station.db, &station.config, "music", &parent).unwrap());
        // The reclassified file is re-read, which the scan counts as added;
        // what it must not be is a second row.
        assert_eq!(station.scan().missing, 0);

        let row = station.only_row();
        assert_eq!((row.id, row.content_type.as_str()), (id, "music"));
    }

    /// Only a config edited by hand gets here: [`locate`] refuses it.
    #[test]
    fn two_paths_pointed_at_one_folder_leave_one_present_row_per_file() {
        let station = Station::adopted();
        let other = station.folder("other");
        write_wav(&Path::new(&other).join("a.wav"), 2, 1);
        add(&station.db, &station.config, "jingle", &other).unwrap();
        station.scan();
        let jingles = list(&station.db)["jingle"][0].id;

        station
            .config
            .set_mount(jingles, &station.folder("music"))
            .unwrap();
        station.db.set_mounts(&station.config.mounts()).unwrap();

        let rows = station.db.track_index().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].path, rows[1].path);
        let present: Vec<_> = rows.iter().filter(|r| r.missing_since.is_none()).collect();
        assert_eq!(present.len(), 1);
        assert_eq!(present[0].content_type, "music");
    }

    #[test]
    fn a_folder_is_one_path_of_its_type_and_no_two_paths_share_one() {
        let station = Station::adopted();
        let music = station.folder("music");
        let other = station.folder("other");

        assert!(!add(&station.db, &station.config, "music", &music).unwrap());
        assert!(!add(&station.db, &station.config, "bogus", &other).unwrap());
        assert!(add(&station.db, &station.config, "jingle", &other).unwrap());
        let jingles = list(&station.db)["jingle"][0].id;
        assert!(!locate(&station.db, &station.config, jingles, &music).unwrap());
        assert!(!locate(&station.db, &station.config, 999, &other).unwrap());
        assert!(!remove(&station.db, &station.config, 999).unwrap());
    }

    #[test]
    fn a_path_under_a_root_is_stored_relative_and_reads_back_whole() {
        let music = abs(&["mnt", "radio", "music"]);
        let roots = roots(&[(1, "music", Some(&music))]);
        let file = abs(&["mnt", "radio", "music", "Artist", "Song.mp3"]);

        let stored = roots.relativize(&file);

        assert_eq!(stored, (1, "Artist/Song.mp3".to_owned()));
        assert_eq!(roots.resolve(stored.0, stored.1), file);
    }

    #[test]
    fn a_path_under_no_root_is_kept_whole() {
        let roots = roots(&[(1, "music", Some(&abs(&["music"])))]);
        let file = abs(&["music2", "a.mp3"]);

        assert_eq!(roots.relativize(&file), (NO_ROOT, file.clone()));
        assert_eq!(roots.resolve(NO_ROOT, file.clone()), file);
    }

    #[test]
    fn a_root_is_not_a_file_under_itself() {
        let music = abs(&["music"]);
        let roots = roots(&[(1, "music", Some(&music))]);

        assert_eq!(roots.relativize(&music), (NO_ROOT, music));
    }

    #[test]
    fn a_file_under_two_roots_belongs_to_the_first_in_listing_order() {
        let roots = roots(&[
            (1, "jingle", Some(&abs(&["radio", "idents"]))),
            (2, "music", Some(&abs(&["radio"]))),
        ]);

        let stored = roots.relativize(&abs(&["radio", "idents", "a.mp3"]));

        assert_eq!(stored, (2, "idents/a.mp3".to_owned()));
    }

    #[test]
    fn moving_the_mount_moves_every_path_under_it() {
        let before = roots(&[(1, "music", Some(&abs(&["Volumes", "radio"])))]);
        let after = roots(&[(1, "music", Some(&abs(&["mnt", "station"])))]);
        let (root, path) = before.relativize(&abs(&["Volumes", "radio", "a", "b.mp3"]));

        assert_eq!(
            after.resolve(root, path),
            abs(&["mnt", "station", "a", "b.mp3"])
        );
    }

    #[test]
    fn a_root_with_no_folder_resolves_somewhere_that_cannot_exist_and_back() {
        let roots = roots(&[(7, "music", None)]);

        let resolved = roots.resolve(7, "a/b.mp3".to_owned());

        assert!(!Path::new(&resolved).exists());
        assert!(resolved.starts_with("<library path 7, not located>"));
        assert_eq!(roots.relativize(&resolved), (7, "a/b.mp3".to_owned()));
    }
}

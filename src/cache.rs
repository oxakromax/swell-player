//! A SQLite cache, so the library and pages can be drawn before TIDAL
//! answers and unchanged data is not downloaded again.
//!
//! - Collections (favourite tracks...) with their order, validated against
//!   TIDAL with one request at startup.
//! - API answers for pages (album, artist, home...) keyed by path, served
//!   while they are fresh and shown while they are being revalidated.
//!
//! Playback manifests (signed, short-lived URLs) and tokens (in the keyring)
//! never go here. The file is a cache: deleting it only costs reloads.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use gtk::glib;
use rusqlite::{Connection, params};

use crate::model::TrackInfo;

const SCHEMA_VERSION: i32 = 2;
const FAVORITE_TRACKS: &str = "favorite_tracks";
/// Drops metadata no collection refers to any more.
const ORPHAN_TRACKS: &str =
    "DELETE FROM track WHERE id NOT IN (SELECT track_id FROM collection_track)";

pub struct CachedCollection {
    pub tracks: Vec<TrackInfo>,
    /// TIDAL's total when the collection was cached.
    pub total: usize,
    pub age: Duration,
}

#[derive(Clone)]
pub struct Cache {
    connection: Arc<Mutex<Connection>>,
    /// Set by [`Cache::forget_user`], under the connection lock: writes
    /// that come later (work still in flight at sign-out) are dropped.
    closed: Arc<AtomicBool>,
}

impl Cache {
    pub async fn open() -> Result<Self> {
        tokio::task::spawn_blocking(|| {
            let dir = glib::user_cache_dir().join("swell-player");
            std::fs::create_dir_all(&dir)?;
            Self::open_at(&dir.join("library.sqlite3"))
        })
        .await?
    }

    fn open_at(path: &Path) -> Result<Self> {
        let connection = Connection::open(path).context("could not open the library cache")?;
        // Takes effect for a new file only; lets `prune` give space back.
        connection.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != SCHEMA_VERSION {
            // A cache: on any schema change, start over rather than migrate.
            connection.execute_batch(
                "DROP TABLE IF EXISTS collection_track;
                 DROP TABLE IF EXISTS collection;
                 DROP TABLE IF EXISTS track;
                 DROP TABLE IF EXISTS response;",
            )?;
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS track (
                 id   INTEGER PRIMARY KEY,
                 data BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS response (
                 user_id    INTEGER NOT NULL,
                 key        TEXT NOT NULL,
                 data       BLOB NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 PRIMARY KEY (user_id, key)
             );
             CREATE TABLE IF NOT EXISTS collection (
                 user_id    INTEGER NOT NULL,
                 name       TEXT NOT NULL,
                 total      INTEGER NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 PRIMARY KEY (user_id, name)
             );
             CREATE TABLE IF NOT EXISTS collection_track (
                 user_id  INTEGER NOT NULL,
                 name     TEXT NOT NULL,
                 position INTEGER NOT NULL,
                 track_id INTEGER NOT NULL REFERENCES track (id),
                 PRIMARY KEY (user_id, name, position)
             );",
        )?;
        connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            closed: Arc::new(AtomicBool::new(false)),
        })
    }

    pub async fn favorite_tracks(&self, user_id: u64) -> Result<Option<CachedCollection>> {
        self.blocking(move |db| read_collection(db, user_id, FAVORITE_TRACKS))
            .await
    }

    pub async fn store_favorite_tracks(
        &self,
        user_id: u64,
        total: usize,
        tracks: Vec<TrackInfo>,
    ) -> Result<()> {
        self.write(move |db| write_collection(db, user_id, FAVORITE_TRACKS, total, &tracks))
            .await
    }

    /// TIDAL's total for the cached favourite tracks (which may count
    /// tracks TIDAL no longer lists), without reading the tracks.
    pub async fn favorite_tracks_total(&self, user_id: u64) -> Result<Option<usize>> {
        self.blocking(move |db| {
            let total = db.query_row(
                "SELECT total FROM collection WHERE user_id = ?1 AND name = ?2",
                params![user_id, FAVORITE_TRACKS],
                |row| row.get::<_, usize>(0),
            );
            match total {
                Ok(total) => Ok(Some(total)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(err) => Err(err.into()),
            }
        })
        .await
    }

    /// A cached API answer and its age.
    pub async fn response(&self, user_id: u64, key: String) -> Result<Option<(Vec<u8>, Duration)>> {
        self.blocking(move |db| {
            let row = db
                .prepare_cached(
                    "SELECT data, fetched_at FROM response WHERE user_id = ?1 AND key = ?2",
                )?
                .query_row(params![user_id, key], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, u64>(1)?))
                });
            match row {
                Ok((data, fetched_at)) => Ok(Some((data, age_of(fetched_at)))),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(err) => Err(err.into()),
            }
        })
        .await
    }

    pub async fn store_response(&self, user_id: u64, key: String, data: Vec<u8>) -> Result<()> {
        self.write(move |db| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            db.prepare_cached(
                "INSERT INTO response (user_id, key, data, fetched_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (user_id, key) DO UPDATE SET
                     data = excluded.data, fetched_at = excluded.fetched_at",
            )?
            .execute(params![user_id, key, data, now])?;
            Ok(())
        })
        .await
    }

    /// Drops cached answers whose key starts with `prefix` (after an edit).
    pub async fn forget_responses(&self, user_id: u64, prefix: String) -> Result<()> {
        self.blocking(move |db| {
            db.execute(
                "DELETE FROM response WHERE user_id = ?1 AND substr(key, 1, length(?2)) = ?2",
                params![user_id, prefix],
            )?;
            Ok(())
        })
        .await
    }

    /// Removes answers older than `max_age` and compacts the file a little.
    pub async fn prune(&self, max_age: Duration) -> Result<()> {
        self.blocking(move |db| {
            let cutoff = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_secs()
                .saturating_sub(max_age.as_secs());
            db.execute("DELETE FROM response WHERE fetched_at < ?1", [cutoff])?;
            db.pragma_update(None, "incremental_vacuum", 0)?;
            Ok(())
        })
        .await
    }

    /// Size of the cache file in bytes.
    pub fn file_size() -> u64 {
        let path = glib::user_cache_dir()
            .join("swell-player")
            .join("library.sqlite3");
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }

    /// Forgets everything cached for `user_id` (on sign-out). From then on
    /// this handle (and its clones) store nothing.
    pub async fn forget_user(&self, user_id: u64) -> Result<()> {
        let closed = self.closed.clone();
        self.blocking(move |db| {
            closed.store(true, Ordering::SeqCst);
            let tx = db.transaction()?;
            tx.execute("DELETE FROM collection_track WHERE user_id = ?1", [user_id])?;
            tx.execute("DELETE FROM collection WHERE user_id = ?1", [user_id])?;
            tx.execute("DELETE FROM response WHERE user_id = ?1", [user_id])?;
            tx.execute(ORPHAN_TRACKS, [])?;
            Ok(tx.commit()?)
        })
        .await
    }

    /// Runs a write unless [`Cache::forget_user`] ran first. The check
    /// happens under the connection lock, so the two cannot interleave.
    async fn write<F>(&self, f: F) -> Result<()>
    where
        F: FnOnce(&mut Connection) -> Result<()> + Send + 'static,
    {
        let closed = self.closed.clone();
        self.blocking(move |db| {
            if closed.load(Ordering::SeqCst) {
                return Ok(());
            }
            f(db)
        })
        .await
    }

    async fn blocking<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || f(&mut connection.lock().unwrap())).await?
    }
}

fn read_collection(
    db: &mut Connection,
    user_id: u64,
    name: &str,
) -> Result<Option<CachedCollection>> {
    let header = db.query_row(
        "SELECT total, fetched_at FROM collection WHERE user_id = ?1 AND name = ?2",
        params![user_id, name],
        |row| Ok((row.get::<_, usize>(0)?, row.get::<_, u64>(1)?)),
    );
    let (total, fetched_at) = match header {
        Ok(header) => header,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(err) => return Err(err.into()),
    };

    let mut statement = db.prepare_cached(
        "SELECT t.data
         FROM collection_track c JOIN track t ON t.id = c.track_id
         WHERE c.user_id = ?1 AND c.name = ?2
         ORDER BY c.position",
    )?;
    let tracks = statement
        .query_map(params![user_id, name], |row| row.get::<_, Vec<u8>>(0))?
        .filter_map(|data| serde_json::from_slice::<TrackInfo>(&data.ok()?).ok())
        .collect::<Vec<_>>();
    Ok(Some(CachedCollection {
        tracks,
        total,
        age: age_of(fetched_at),
    }))
}

fn age_of(fetched_at: u64) -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH + Duration::from_secs(fetched_at))
        .unwrap_or_default()
}

fn write_collection(
    db: &mut Connection,
    user_id: u64,
    name: &str,
    total: usize,
    tracks: &[TrackInfo],
) -> Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let tx = db.transaction()?;
    tx.execute(
        "DELETE FROM collection_track WHERE user_id = ?1 AND name = ?2",
        params![user_id, name],
    )?;
    {
        let mut upsert_track = tx.prepare_cached(
            "INSERT INTO track (id, data) VALUES (?1, ?2)
             ON CONFLICT (id) DO UPDATE SET data = excluded.data",
        )?;
        let mut insert_position = tx.prepare_cached(
            "INSERT INTO collection_track (user_id, name, position, track_id) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (position, track) in tracks.iter().enumerate() {
            upsert_track.execute(params![track.id, serde_json::to_vec(track)?])?;
            insert_position.execute(params![user_id, name, position, track.id])?;
        }
    }
    tx.execute(ORPHAN_TRACKS, [])?;
    tx.execute(
        "INSERT INTO collection (user_id, name, total, fetched_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (user_id, name) DO UPDATE SET total = excluded.total, fetched_at = excluded.fetched_at",
        params![user_id, name, total, now],
    )?;
    Ok(tx.commit()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: u64) -> TrackInfo {
        TrackInfo {
            id,
            title: format!("Track {id}"),
            album: "Album".into(),
            duration: 180,
            cover: Some("ab-cd".into()),
            hires: id.is_multiple_of(2),
            ..TrackInfo::default()
        }
    }

    #[test]
    fn favourites_round_trip_in_order() {
        let cache = Cache::open_at(Path::new(":memory:")).unwrap();
        let mut db = cache.connection.lock().unwrap();
        assert!(
            read_collection(&mut db, 7, FAVORITE_TRACKS)
                .unwrap()
                .is_none()
        );

        write_collection(
            &mut db,
            7,
            FAVORITE_TRACKS,
            4,
            &[track(3), track(1), track(2)],
        )
        .unwrap();
        write_collection(&mut db, 7, FAVORITE_TRACKS, 3, &[track(2), track(3)]).unwrap();

        let cached = read_collection(&mut db, 7, FAVORITE_TRACKS)
            .unwrap()
            .unwrap();
        assert_eq!(cached.total, 3);
        let ids: Vec<_> = cached.tracks.iter().map(|t| t.id).collect();
        assert_eq!(ids, [2, 3]);
        assert!(cached.tracks[0].hires);
        drop(db);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            cache
                .store_response(7, "pages/a".into(), b"x".to_vec())
                .await
                .unwrap();
            let (data, _) = cache.response(7, "pages/a".into()).await.unwrap().unwrap();
            assert_eq!(data, b"x");
            cache.forget_responses(7, "pages/".into()).await.unwrap();
            assert!(cache.response(7, "pages/a".into()).await.unwrap().is_none());
        });
        let mut db = cache.connection.lock().unwrap();
        assert!(
            read_collection(&mut db, 8, FAVORITE_TRACKS)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn nothing_is_stored_after_forgetting_the_user() {
        let cache = Cache::open_at(Path::new(":memory:")).unwrap();
        let late = cache.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            cache
                .store_response(7, "a".into(), b"x".to_vec())
                .await
                .unwrap();
            cache
                .store_favorite_tracks(7, 1, vec![track(1)])
                .await
                .unwrap();
            cache.forget_user(7).await.unwrap();
            assert!(cache.response(7, "a".into()).await.unwrap().is_none());
            assert!(cache.favorite_tracks(7).await.unwrap().is_none());

            late.store_response(7, "b".into(), b"y".to_vec())
                .await
                .unwrap();
            late.store_favorite_tracks(7, 1, vec![track(2)])
                .await
                .unwrap();
            assert!(cache.response(7, "b".into()).await.unwrap().is_none());
            assert!(cache.favorite_tracks(7).await.unwrap().is_none());
        });
    }
}

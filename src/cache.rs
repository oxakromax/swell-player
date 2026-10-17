//! A SQLite cache of library metadata, so the favourites can be drawn at
//! startup before TIDAL answers.
//!
//! Only data that is safe to reuse lives here: track metadata and the order
//! of the user's favourites. Playback manifests (signed, short-lived URLs)
//! and tokens (in the keyring) never do. The file is a cache: deleting it
//! only costs one full reload.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use gtk::glib;
use rusqlite::{Connection, params};

use crate::model::TrackInfo;

const SCHEMA_VERSION: i32 = 1;
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
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != SCHEMA_VERSION {
            // A cache: on any schema change, start over rather than migrate.
            connection.execute_batch(
                "DROP TABLE IF EXISTS collection_track;
                 DROP TABLE IF EXISTS collection;
                 DROP TABLE IF EXISTS track;",
            )?;
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS track (
                 id       INTEGER PRIMARY KEY,
                 title    TEXT NOT NULL,
                 artists  TEXT NOT NULL,
                 album    TEXT NOT NULL,
                 duration INTEGER NOT NULL,
                 cover    TEXT,
                 hires    INTEGER NOT NULL
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
        self.blocking(move |db| write_collection(db, user_id, FAVORITE_TRACKS, total, &tracks))
            .await
    }

    /// Forgets everything cached for `user_id` (on sign-out).
    pub async fn forget_user(&self, user_id: u64) -> Result<()> {
        self.blocking(move |db| {
            let tx = db.transaction()?;
            tx.execute("DELETE FROM collection_track WHERE user_id = ?1", [user_id])?;
            tx.execute("DELETE FROM collection WHERE user_id = ?1", [user_id])?;
            tx.execute(ORPHAN_TRACKS, [])?;
            Ok(tx.commit()?)
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
        "SELECT t.id, t.title, t.artists, t.album, t.duration, t.cover, t.hires
         FROM collection_track c JOIN track t ON t.id = c.track_id
         WHERE c.user_id = ?1 AND c.name = ?2
         ORDER BY c.position",
    )?;
    let tracks = statement
        .query_map(params![user_id, name], |row| {
            Ok(TrackInfo {
                id: row.get(0)?,
                title: row.get(1)?,
                artists: row.get(2)?,
                album: row.get(3)?,
                duration: row.get(4)?,
                cover: row.get(5)?,
                hires: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let age = SystemTime::now()
        .duration_since(UNIX_EPOCH + Duration::from_secs(fetched_at))
        .unwrap_or_default();
    Ok(Some(CachedCollection { tracks, total, age }))
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
            "INSERT INTO track (id, title, artists, album, duration, cover, hires)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (id) DO UPDATE SET
                 title = excluded.title, artists = excluded.artists, album = excluded.album,
                 duration = excluded.duration, cover = excluded.cover, hires = excluded.hires",
        )?;
        let mut insert_position = tx.prepare_cached(
            "INSERT INTO collection_track (user_id, name, position, track_id) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (position, track) in tracks.iter().enumerate() {
            upsert_track.execute(params![
                track.id,
                track.title,
                track.artists,
                track.album,
                track.duration,
                track.cover,
                track.hires
            ])?;
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
            artists: "Artist".into(),
            album: "Album".into(),
            duration: 180,
            cover: Some("ab-cd".into()),
            hires: id.is_multiple_of(2),
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
        assert!(
            read_collection(&mut db, 8, FAVORITE_TRACKS)
                .unwrap()
                .is_none()
        );
    }
}

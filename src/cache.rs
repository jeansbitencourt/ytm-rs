use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use anyhow::Result;
use rusqlite::{params, Connection};
use crate::innertube::Track;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS liked_tracks (
                video_id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT NOT NULL,
                duration_secs INTEGER NOT NULL,
                thumbnail_url TEXT NOT NULL,
                added_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS play_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                video_id TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT NOT NULL,
                duration_secs INTEGER NOT NULL,
                thumbnail_url TEXT NOT NULL,
                played_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS stream_cache (
                video_id TEXT PRIMARY KEY,
                stream_url TEXT NOT NULL,
                expires_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS playlists (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                count INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS playlist_tracks (
                playlist_id TEXT NOT NULL,
                video_id TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT NOT NULL,
                duration_secs INTEGER NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (playlist_id, video_id)
            );
            "#,
        )?;
        Ok(())
    }

    fn now_ts() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    pub fn add_liked_track(&self, track: &Track) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO liked_tracks (video_id, title, artist, album, duration_secs, thumbnail_url, added_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                track.video_id,
                track.title,
                track.artist,
                track.album,
                track.duration_secs,
                track.thumbnail_url,
                Self::now_ts()
            ],
        )?;
        Ok(())
    }

    pub fn remove_liked_track(&self, video_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM liked_tracks WHERE video_id = ?1",
            params![video_id],
        )?;
        Ok(())
    }

    pub fn is_track_liked(&self, video_id: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare("SELECT 1 FROM liked_tracks WHERE video_id = ?1")?;
        let exists = stmt.exists(params![video_id])?;
        Ok(exists)
    }

    pub fn get_liked_tracks(&self) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT video_id, title, artist, album, duration_secs, thumbnail_url FROM liked_tracks ORDER BY added_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let mut track = Track::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, String>(5)?,
            );
            track.is_liked = true;
            Ok(track)
        })?;

        let mut tracks = Vec::new();
        for r in rows {
            tracks.push(r?);
        }
        Ok(tracks)
    }

    pub fn add_history(&self, track: &Track) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO play_history (video_id, title, artist, album, duration_secs, thumbnail_url, played_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                track.video_id,
                track.title,
                track.artist,
                track.album,
                track.duration_secs,
                track.thumbnail_url,
                Self::now_ts()
            ],
        )?;
        Ok(())
    }

    pub fn get_history(&self, limit: usize) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT video_id, title, artist, album, duration_secs, thumbnail_url FROM play_history ORDER BY played_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let track = Track::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, String>(5)?,
            );
            Ok(track)
        })?;

        let mut tracks = Vec::new();
        for r in rows {
            tracks.push(r?);
        }
        Ok(tracks)
    }

    pub fn save_stream_url(&self, video_id: &str, stream_url: &str, ttl_secs: u64) -> Result<()> {
        let expires_at = Self::now_ts() + ttl_secs;
        self.conn.execute(
            "INSERT OR REPLACE INTO stream_cache (video_id, stream_url, expires_at) VALUES (?1, ?2, ?3)",
            params![video_id, stream_url, expires_at],
        )?;
        Ok(())
    }

    pub fn get_stream_url(&self, video_id: &str) -> Result<Option<String>> {
        let now = Self::now_ts();
        let mut stmt = self.conn.prepare(
            "SELECT stream_url FROM stream_cache WHERE video_id = ?1 AND expires_at > ?2",
        )?;
        let mut rows = stmt.query(params![video_id, now])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_liked_tracks_crud() {
        let db = Database::open_in_memory().unwrap();
        let track = Track::new("id1", "Title 1", "Artist 1", "Album 1", 200, "thumb1");

        assert!(!db.is_track_liked("id1").unwrap());
        db.add_liked_track(&track).unwrap();
        assert!(db.is_track_liked("id1").unwrap());

        let liked = db.get_liked_tracks().unwrap();
        assert_eq!(liked.len(), 1);
        assert_eq!(liked[0].title, "Title 1");

        db.remove_liked_track("id1").unwrap();
        assert!(!db.is_track_liked("id1").unwrap());
    }

    #[test]
    fn test_stream_cache() {
        let db = Database::open_in_memory().unwrap();
        db.save_stream_url("id2", "https://stream.url", 3600).unwrap();
        let cached = db.get_stream_url("id2").unwrap();
        assert_eq!(cached, Some("https://stream.url".to_string()));
    }
}

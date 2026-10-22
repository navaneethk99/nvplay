//! Bounded local watch history with durable playback checkpoints.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const LIMIT: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub source: String,
    pub title: String,
    pub position: f64,
    pub duration: f64,
    pub watched_at: u64,
    pub completed: bool,
}
impl Default for Entry {
    fn default() -> Self {
        Self {
            source: String::new(),
            title: String::new(),
            position: 0.0,
            duration: 0.0,
            watched_at: 0,
            completed: false,
        }
    }
}
pub struct History {
    path: Option<PathBuf>,
    entries: Vec<Entry>,
    dirty: bool,
    last_flush: Instant,
}
impl History {
    #[cfg(not(test))]
    pub fn load() -> Result<Self, String> {
        Self::at(crate::settings::directory()?.join("history.json"))
    }
    pub(crate) fn at(path: PathBuf) -> Result<Self, String> {
        let mut entries: Vec<Entry> = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Could not read saved watch history.".to_string())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return Err("Could not read saved watch history.".into()),
        };
        entries.retain(|e| {
            !e.source.trim().is_empty()
                && !e.source.contains('\0')
                && e.position.is_finite()
                && e.duration.is_finite()
        });
        let mut seen = std::collections::HashSet::new();
        entries.retain_mut(|entry| {
            entry.position = entry.position.max(0.0);
            entry.duration = entry.duration.max(0.0);
            if entry.completed {
                entry.position = 0.0;
            }
            if entry.title.is_empty() {
                entry.title = entry.source.clone();
            }
            seen.insert(entry.source.clone())
        });
        entries.truncate(LIMIT);
        Ok(Self {
            path: Some(path),
            entries,
            dirty: false,
            last_flush: Instant::now(),
        })
    }
    pub fn empty() -> Self {
        Self {
            path: crate::settings::directory()
                .ok()
                .map(|p| p.join("history.json")),
            entries: Vec::new(),
            dirty: false,
            last_flush: Instant::now(),
        }
    }
    #[cfg(test)]
    pub fn memory() -> Self {
        Self {
            path: None,
            ..Self::empty()
        }
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn entry(&self, source: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.source == source)
    }
    pub fn record(
        &mut self,
        source: &str,
        title: &str,
        position: f64,
        duration: f64,
        completed: bool,
    ) {
        if !position.is_finite() || !duration.is_finite() || source.is_empty() {
            return;
        }
        let position = if completed { 0.0 } else { position.max(0.0) };
        let index = self.entries.iter().position(|e| e.source == source);
        if index == Some(0)
            && self.entries.first().is_some_and(|e| {
                e.position == position
                    && e.title == title
                    && e.duration == duration
                    && e.completed == completed
            })
        {
            return;
        }
        if let Some(index) = index {
            self.entries.remove(index);
        }
        self.entries.insert(
            0,
            Entry {
                source: source.into(),
                title: title.into(),
                position,
                duration: duration.max(0.0),
                watched_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                completed,
            },
        );
        self.entries.truncate(LIMIT);
        self.dirty = true;
    }
    pub fn remove(&mut self, source: &str) {
        self.entries.retain(|e| e.source != source);
        self.dirty = true;
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.dirty = true;
    }
    pub fn flush(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(path) = &self.path {
            let bytes = serde_json::to_vec_pretty(&self.entries)
                .map_err(|_| "Could not encode watch history.".to_string())?;
            crate::settings::write_private(path, &bytes).map_err(|_| {
                "Could not save watch history. Check your app settings folder.".to_string()
            })?;
        }
        self.dirty = false;
        self.last_flush = Instant::now();
        Ok(())
    }
    pub fn flush_if_due(&mut self) -> Result<(), String> {
        if self.last_flush.elapsed() >= Duration::from_secs(5) {
            self.last_flush = Instant::now();
            self.flush()?;
        }
        Ok(())
    }
}
pub fn source_identity(source: &str) -> String {
    if source.contains("://") {
        source.into()
    } else {
        fs::canonicalize(source)
            .unwrap_or_else(|_| {
                let path = PathBuf::from(source);
                if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir().unwrap_or_default().join(path)
                }
            })
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitles::library::tests::Fixture;
    #[test]
    fn history_restores_progress_and_recent_order_without_duplicates() {
        let fixture = Fixture::new();
        let path = fixture.0.join("history.json");
        let mut history = History::at(path.clone()).unwrap();
        history.record("movie-a", "First movie", 123.5, 7200.0, false);
        history.record("movie-b", "Second movie", 20.0, 5000.0, false);
        history.record("movie-a", "First movie", 456.75, 7200.0, false);
        history.flush().unwrap();
        drop(history);
        let history = History::at(path).unwrap();
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].source, "movie-a");
        assert_eq!(history.entries[0].position, 456.75);
        assert_eq!(history.entries[0].duration, 7200.0);
        assert_eq!(history.entries[1].title, "Second movie");
    }
    #[test]
    fn finished_movies_restart_and_history_is_bounded_removable_and_clearable() {
        let mut history = History::memory();
        for i in 0..105 {
            history.record(&format!("movie-{i}"), "Movie", 10.0, 50.0, false);
        }
        assert_eq!(history.entries.len(), LIMIT);
        assert!(history.entry("movie-0").is_none());
        history.record("movie-104", "Movie", 50.0, 50.0, true);
        assert!(history.entries[0].completed);
        assert_eq!(history.entries[0].position, 0.0);
        history.remove("movie-104");
        assert!(history.entry("movie-104").is_none());
        history.clear();
        assert!(history.entries.is_empty());
    }
    #[test]
    fn paused_samples_do_not_rewrite_history_and_invalid_timestamps_are_ignored() {
        let mut history = History::memory();
        history.record("video", "Title", 10.0, 30.0, false);
        history.flush().unwrap();
        history.record("video", "Title", 10.0, 30.0, false);
        assert!(!history.dirty);
        history.record("bad", "Title", f64::NAN, 30.0, false);
        history.record("bad", "Title", 10.0, f64::INFINITY, false);
        assert!(history.entry("bad").is_none());
    }
}

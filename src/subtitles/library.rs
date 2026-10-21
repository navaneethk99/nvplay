//! Persistent subtitle associations and global appearance preferences.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub font: String,
    pub size: f32,
    pub color: [u8; 4],
    pub outline: f32,
    pub outline_color: [u8; 4],
    pub shadow: f32,
    pub shadow_color: [u8; 4],
    pub bold: bool,
    pub italic: bool,
    pub spacing: f32,
    pub position: f32,
    pub margin: i32,
    pub alignment: String,
    pub background_box: bool,
    pub override_styles: bool,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            font: "sans-serif".into(),
            size: 38.0,
            color: [255; 4],
            outline: 1.65,
            outline_color: [0, 0, 0, 255],
            shadow: 0.0,
            shadow_color: [0, 0, 0, 175],
            bold: false,
            italic: false,
            spacing: 0.0,
            position: 100.0,
            margin: 34,
            alignment: "center".into(),
            background_box: false,
            override_styles: true,
        }
    }
}
impl Style {
    fn normalize(&mut self) {
        self.font = self
            .font
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(128)
            .collect();
        if self.font.is_empty() {
            self.font = "sans-serif".into();
        }
        self.size = self.size.clamp(8.0, 120.0);
        self.outline = self.outline.clamp(0.0, 10.0);
        self.shadow = self.shadow.clamp(0.0, 10.0);
        self.spacing = self.spacing.clamp(-10.0, 10.0);
        self.position = self.position.clamp(0.0, 100.0);
        self.margin = self.margin.clamp(0, 150);
        if !matches!(self.alignment.as_str(), "left" | "center" | "right") {
            self.alignment = "center".into();
        }
    }
    pub fn properties(&self) -> Vec<(&'static str, String)> {
        let color = |c: [u8; 4]| format!("#{:02X}{:02X}{:02X}{:02X}", c[3], c[0], c[1], c[2]);
        let yes_no = |b| if b { "yes".into() } else { "no".into() };
        vec![
            (
                "sub-font",
                if self.font.trim().is_empty() {
                    "sans-serif".into()
                } else {
                    self.font.trim().into()
                },
            ),
            ("sub-font-size", self.size.to_string()),
            ("sub-color", color(self.color)),
            ("sub-border-size", self.outline.to_string()),
            ("sub-border-color", color(self.outline_color)),
            ("sub-shadow-offset", self.shadow.to_string()),
            ("sub-back-color", color(self.shadow_color)),
            ("sub-bold", yes_no(self.bold)),
            ("sub-italic", yes_no(self.italic)),
            ("sub-spacing", self.spacing.to_string()),
            ("sub-pos", self.position.to_string()),
            ("sub-margin-y", self.margin.to_string()),
            ("sub-align-x", self.alignment.clone()),
            (
                "sub-ass-override",
                if self.override_styles { "force" } else { "no" }.into(),
            ),
            (
                "sub-border-style",
                if self.background_box {
                    "background-box"
                } else {
                    "outline-and-shadow"
                }
                .into(),
            ),
        ]
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    file: String,
    title: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    style: Style,
    videos: BTreeMap<String, Record>,
}

pub struct Library {
    root: Option<PathBuf>,
    state: State,
}
impl Library {
    #[cfg(not(test))]
    pub fn load() -> Result<Self, String> {
        Self::at(super::settings::directory()?)
    }
    pub(crate) fn at(root: PathBuf) -> Result<Self, String> {
        let path = root.join("subtitles.json");
        let mut state: State = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Could not read saved subtitle preferences.".to_string())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(_) => return Err("Could not read saved subtitle preferences.".into()),
        };
        state.style.normalize();
        Ok(Self {
            root: Some(root),
            state,
        })
    }
    pub fn empty() -> Self {
        Self {
            root: super::settings::directory().ok(),
            state: State::default(),
        }
    }
    #[cfg(test)]
    pub fn memory() -> Self {
        Self {
            root: None,
            state: State::default(),
        }
    }
    pub fn style(&self) -> &Style {
        &self.state.style
    }
    pub fn set_style(&mut self, style: Style) {
        self.state.style = style;
    }
    pub fn save(&self) -> Result<(), String> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&self.state)
            .map_err(|_| "Could not encode subtitle preferences.".to_string())?;
        super::settings::write_private(&root.join("subtitles.json"), &bytes).map_err(|_| {
            "Could not save subtitle preferences. Check your app settings folder.".into()
        })
    }
    pub fn remembered(&self, source: &str) -> Option<(PathBuf, String)> {
        let root = self.root.as_ref()?;
        let record = self.state.videos.get(&source_key(source))?;
        // Only accept filenames produced by this cache, never paths from edited JSON.
        if Path::new(&record.file).components().count() != 1
            || !record.file.starts_with("subtitle-")
        {
            return None;
        }
        let path = root.join("subtitles").join(&record.file);
        Some((path, record.title.clone()))
    }
    pub fn forget(&mut self, source: &str) -> Result<(), String> {
        self.state.videos.remove(&source_key(source));
        self.save()
    }
    pub fn remember(&mut self, source: &str, subtitle: &Path, title: &str) -> Result<(), String> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        let mut bytes = Vec::new();
        fs::File::open(subtitle)
            .and_then(|file| file.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes))
            .map_err(|_| {
                "Subtitle added, but its file could not be saved for next time.".to_string()
            })?;
        if bytes.is_empty() || bytes.len() > 32 * 1024 * 1024 {
            return Err(
                "Subtitle added, but its file is empty or exceeds the 32 MB cache limit.".into(),
            );
        }
        let extension = subtitle
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("srt")
            .to_ascii_lowercase();
        if !matches!(
            extension.as_str(),
            "srt" | "ass" | "ssa" | "vtt" | "sub" | "idx"
        ) {
            return Err("Subtitle added, but this subtitle format cannot be remembered.".into());
        }
        // VobSub consists of an index and a bitmap file; retain both together.
        let companion = match extension.as_str() {
            "idx" => Some(subtitle.with_extension("sub")),
            "sub" if subtitle.with_extension("idx").is_file() => {
                Some(subtitle.with_extension("idx"))
            }
            _ => None,
        };
        let companion_bytes = companion.as_ref().map(fs::read).transpose().map_err(|_| {
            "Subtitle added, but its companion file could not be cached.".to_string()
        })?;
        let mut hash = Sha256::new();
        hash.update(&bytes);
        if let Some(data) = &companion_bytes {
            hash.update(data);
        }
        let stem = format!("subtitle-{:x}", hash.finalize());
        let cache = root.join("subtitles");
        super::settings::write_private(&cache.join(format!("{stem}.{extension}")), &bytes)
            .map_err(|_| {
                "Subtitle added, but the subtitle cache could not be written.".to_string()
            })?;
        if let (Some(companion), Some(data)) = (&companion, &companion_bytes) {
            let ext = companion.extension().unwrap().to_string_lossy();
            super::settings::write_private(&cache.join(format!("{stem}.{ext}")), data).map_err(
                |_| "Subtitle added, but its companion file could not be saved.".to_string(),
            )?;
        }
        let extension = if companion.is_some() {
            "idx"
        } else {
            &extension
        };
        self.state.videos.insert(
            source_key(source),
            Record {
                file: format!("{stem}.{extension}"),
                title: title.into(),
            },
        );
        self.save()
    }
}
fn source_key(source: &str) -> String {
    let normalized = if source.contains("://") {
        source.to_owned()
    } else {
        fs::canonicalize(source)
            .unwrap_or_else(|_| PathBuf::from(source))
            .to_string_lossy()
            .into_owned()
    };
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    pub(crate) struct Fixture(pub PathBuf);
    impl Fixture {
        pub fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "nvplay-library-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        pub fn library(&self) -> Library {
            Library::at(self.0.clone()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reopening_video_restores_cached_subtitle_after_original_is_deleted() {
        let fixture = Fixture::new();
        let video = fixture.0.join("movie.mkv");
        fs::write(&video, b"video").unwrap();
        let subtitle = fixture.0.join("English.srt");
        fs::write(&subtitle, b"subtitle contents").unwrap();
        let mut library = fixture.library();
        library
            .remember(video.to_str().unwrap(), &subtitle, "English")
            .unwrap();
        // Local paths that refer to the same file share an association.
        let alias = fixture.0.join(".").join("movie.mkv");
        assert!(library.remembered(alias.to_str().unwrap()).is_some());
        let cached = library.remembered(video.to_str().unwrap()).unwrap().0;
        assert_ne!(cached, subtitle);
        fs::remove_file(subtitle).unwrap();
        drop(library);
        let library = fixture.library();
        let (path, title) = library.remembered(video.to_str().unwrap()).unwrap();
        assert_eq!(path, cached);
        assert_eq!(title, "English");
        assert_eq!(fs::read(path).unwrap(), b"subtitle contents");
        assert!(library.remembered("different-video.mkv").is_none());
    }
    #[test]
    fn streams_keep_separate_last_subtitle_records_and_forget_is_persistent() {
        let fixture = Fixture::new();
        let subtitle = fixture.0.join("subtitle.ass");
        fs::write(&subtitle, b"ASS contents").unwrap();
        let mut library = fixture.library();
        let first = "https://cinejoy.pk/watch/movie/10";
        let second = "https://cinejoy.pk/watch/movie/20";
        library.remember(first, &subtitle, "First release").unwrap();
        library.remember(second, &subtitle, "Second movie").unwrap();
        library.remember(first, &subtitle, "New release").unwrap();
        assert!(
            library
                .remember(first, &fixture.0.join("missing.srt"), "Broken")
                .is_err()
        );
        let mut library = fixture.library();
        assert_eq!(library.remembered(first).unwrap().1, "New release");
        assert_eq!(library.remembered(second).unwrap().1, "Second movie");
        library.forget(first).unwrap();
        assert!(fixture.library().remembered(first).is_none());
        assert!(fixture.library().remembered(second).is_some());
    }
    #[test]
    fn appearance_survives_restart_and_color_alpha_matches_mpv() {
        let fixture = Fixture::new();
        let mut library = fixture.library();
        let style = Style {
            font: "Times New Roman".into(),
            size: 56.0,
            color: [255, 32, 16, 128],
            outline: 4.0,
            outline_color: [12, 34, 56, 255],
            bold: true,
            position: 82.0,
            ..Default::default()
        };
        library.set_style(style.clone());
        library.save().unwrap();
        assert_eq!(fixture.library().style(), &style);
        let properties: BTreeMap<_, _> = style.properties().into_iter().collect();
        assert_eq!(properties["sub-color"], "#80FF2010");
        assert_eq!(properties["sub-border-color"], "#FF0C2238");
        assert_eq!(properties["sub-border-size"], "4");
    }
    #[test]
    fn unsafe_cache_paths_and_invalid_preferences_are_handled() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("subtitles.json"), serde_json::json!({
            "style": {"size": -200.0, "outline": 90.0, "alignment":"invalid"},
            "videos": { source_key("video"): {"file":"subtitle-../../outside.srt", "title":"bad"} }
        }).to_string()).unwrap();
        let library = fixture.library();
        assert_eq!(library.style().size, 8.0);
        assert_eq!(library.style().outline, 10.0);
        assert_eq!(library.style().alignment, "center");
        assert!(library.remembered("video").is_none());
        fs::write(fixture.0.join("subtitles.json"), b"broken JSON").unwrap();
        assert!(Library::at(fixture.0.clone()).is_err());
    }
    #[test]
    fn bitmap_subtitle_pair_is_preserved() {
        let fixture = Fixture::new();
        let index = fixture.0.join("bitmap.idx");
        fs::write(&index, b"index").unwrap();
        fs::write(index.with_extension("sub"), b"bitmap").unwrap();
        let mut library = fixture.library();
        library.remember("video", &index, "Bitmap").unwrap();
        let path = fixture.library().remembered("video").unwrap().0;
        assert_eq!(path.extension().unwrap(), "idx");
        assert_eq!(fs::read(&path).unwrap(), b"index");
        assert_eq!(fs::read(path.with_extension("sub")).unwrap(), b"bitmap");
    }
}

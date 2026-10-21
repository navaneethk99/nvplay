//! Local OpenSubtitles credentials, outside the repository.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) fn directory() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    base.map(|base| base.join("nvplay"))
        .ok_or_else(|| "Cannot locate your settings folder to save the API key.".into())
}

fn path() -> Result<PathBuf, String> {
    Ok(directory()?.join("opensubtitles.key"))
}

pub fn load() -> Result<String, String> {
    if let Ok(key) = std::env::var("NVPLAY_OPENSUBTITLES_API_KEY")
        && !key.trim().is_empty()
    {
        return Ok(key.trim().into());
    }
    read(&path()?)
}
fn read(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(key) => Ok(key.trim().into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(_) => Err("Could not read the saved OpenSubtitles API key.".into()),
    }
}
pub fn save(key: &str) -> Result<(), String> {
    write(&path()?, key)
}
pub fn remove() -> Result<(), String> {
    remove_file(&path()?)
}
fn remove_file(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Could not remove the saved API key.".into()),
    }
}
fn write(path: &Path, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Enter an API key before saving.".into());
    }
    write_private(path, key.as_bytes())
        .map_err(|_| "Could not save the API key. Your previous saved key has been kept.".into())
}

/// Replace a settings/cache file only after a complete private staging write.
pub(super) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing settings folder"))?;
    let mut directory = fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(parent)?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (temporary, mut file) = loop {
        let temporary = parent.join(format!(
            ".nvplay-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_key_survives_reload_can_be_replaced_and_forgotten() {
        let directory =
            std::env::temp_dir().join(format!("nvplay-key-test-{}", std::process::id()));
        let path = directory.join("opensubtitles.key");
        assert_eq!(read(&path).unwrap(), "");
        write(&path, " test-key ").unwrap();
        assert_eq!(read(&path).unwrap(), "test-key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(write(&path, " ").is_err());
        assert_eq!(read(&path).unwrap(), "test-key");
        write(&path, "replacement").unwrap();
        assert_eq!(read(&path).unwrap(), "replacement");
        remove_file(&path).unwrap();
        remove_file(&path).unwrap();
        assert_eq!(read(&path).unwrap(), "");
        fs::remove_dir(directory).unwrap();
    }
}

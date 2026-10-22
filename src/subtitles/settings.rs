//! Local OpenSubtitles credentials, outside the repository.
pub(crate) use crate::settings::{directory, write_private};
use std::{
    fs,
    path::{Path, PathBuf},
};

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

use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const INPUTS: &[&str] = &["src", "scripts", "Cargo.toml", "Cargo.lock", "build.rs"];

fn files(path: &Path, output: &mut Vec<PathBuf>) -> io::Result<()> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            files(&entry?.path(), output)?;
        }
    } else if path.is_file() {
        output.push(path.to_owned());
    } else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Missing update source: {}", path.display()),
        ));
    }
    Ok(())
}
pub fn fingerprint(root: &Path) -> io::Result<String> {
    let mut paths = Vec::new();
    for input in INPUTS {
        files(&root.join(input), &mut paths)?;
    }
    paths.sort();
    let mut hash = Sha256::new();
    for path in paths {
        let name = path.strip_prefix(root).unwrap().to_string_lossy();
        let content = fs::read(&path)?;
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((content.len() as u64).to_le_bytes());
        hash.update(content);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_version_detects_content_and_file_changes() {
        let root = std::env::temp_dir().join(format!("nvplay-source-test-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("scripts")).unwrap();
        for path in &INPUTS[2..] {
            fs::write(root.join(path), "fixture").unwrap();
        }
        fs::write(root.join("src/main.rs"), "first").unwrap();
        let first = fingerprint(&root).unwrap();
        assert_eq!(first, fingerprint(&root).unwrap());
        fs::write(root.join("src/main.rs"), "other").unwrap();
        let second = fingerprint(&root).unwrap();
        assert_ne!(first, second);
        fs::write(root.join("src/new.rs"), "new").unwrap();
        assert_ne!(second, fingerprint(&root).unwrap());
        fs::remove_file(root.join("src/new.rs")).unwrap();
        assert_eq!(second, fingerprint(&root).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}

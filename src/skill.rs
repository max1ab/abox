use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const CONTENT: &str = include_str!("../skills/abox/SKILL.md");

pub fn export(directory: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(directory)
        .map_err(|e| format!("cannot create skill directory {}: {e}", directory.display()))?;
    let path = directory.join("SKILL.md");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| {
            format!(
                "cannot create {} (existing files are never overwritten): {e}",
                path.display()
            )
        })?;
    file.write_all(CONTENT.as_bytes()).map_err(|e| {
        format!(
            "cannot finish writing {}: {e}; check the exported file before using it",
            path.display()
        )
    })?;
    Ok(path)
}

/// Setup is repeatable, but never replaces a different user-managed skill.
pub fn check_install(directory: &Path) -> Result<(), String> {
    let path = directory.join("SKILL.md");
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
        Ok(m) if !m.is_file() || m.file_type().is_symlink() => {
            Err("skill is not a regular file".into())
        }
        Ok(_) => {
            let existing = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            if existing == CONTENT {
                Ok(())
            } else {
                Err(format!(
                    "{} already contains a different skill; choose --skill-dir or review it before replacing",
                    path.display()
                ))
            }
        }
    }
}
pub fn install(directory: &Path) -> Result<PathBuf, String> {
    check_install(directory)?;
    let path = directory.join("SKILL.md");
    if path.exists() {
        Ok(path)
    } else {
        export(directory)
    }
}

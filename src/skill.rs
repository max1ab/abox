use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const CONTENT: &str = include_str!("../skills/agent-mail/SKILL.md");

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

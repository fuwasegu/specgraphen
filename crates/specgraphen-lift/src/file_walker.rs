use std::path::{Path, PathBuf};

use anyhow::Result;

pub fn collect_java_files(
    root: &Path,
    patterns: &[String],
    excludes: &[String],
) -> Result<Vec<PathBuf>> {
    // glob matches `..` literally against directory entries, so a root
    // containing parent components would silently match nothing.
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut files = Vec::new();
    for pattern in patterns {
        let full_pattern = root.join(pattern).to_string_lossy().to_string();
        for path in glob::glob(&full_pattern)?.flatten() {
            if path.is_file() {
                let excluded = excludes.iter().any(|ex| {
                    let ex_pattern = root.join(ex).to_string_lossy().to_string();
                    glob::Pattern::new(&ex_pattern)
                        .map(|p| p.matches_path(&path))
                        .unwrap_or(false)
                });
                if !excluded {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

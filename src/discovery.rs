use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

pub fn discover_test_files(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    if path.is_dir() {
        let mut files = Vec::new();
        collect_test_files(path, &mut files)?;
        files.sort();
        if files.is_empty() {
            bail!("no .test files found in {}", path.display());
        }
        return Ok(files);
    }

    bail!("path does not exist: {}", path.display());
}

fn collect_test_files(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_test_files(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "test") {
            files.push(path);
        }
    }
    Ok(())
}

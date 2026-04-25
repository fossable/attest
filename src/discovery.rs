use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

/// Shell-related file extensions that are always scanned for test functions.
const SHELL_EXTENSIONS: &[&str] = &["test", "sh", "bash"];

pub fn discover_test_files(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    if path.is_dir() {
        let mut files = Vec::new();
        collect_script_files(path, &mut files)?;
        files.sort();
        if files.is_empty() {
            bail!("no script files found in {}", path.display());
        }
        return Ok(files);
    }

    bail!("path does not exist: {}", path.display());
}

fn collect_script_files(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_script_files(&path, files)?;
        } else if is_shell_script(&path) {
            files.push(path);
        }
    }
    Ok(())
}

/// A file is considered a shell script if it has a known shell extension or a
/// shell shebang on its first line.
fn is_shell_script(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && SHELL_EXTENSIONS.contains(&ext)
    {
        return true;
    }

    has_shell_shebang(path).unwrap_or(false)
}

fn has_shell_shebang(path: &Path) -> std::io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; 256];
    let n = file.read(&mut buf)?;
    let head = std::str::from_utf8(&buf[..n]).unwrap_or("");
    let first_line = head.lines().next().unwrap_or("");
    Ok(first_line.starts_with("#!") && is_shell_interpreter(first_line))
}

pub(crate) fn is_shell_interpreter(shebang: &str) -> bool {
    let shebang = shebang.trim_start_matches("#!");
    // Handle "#!/usr/bin/env bash" style
    let parts: Vec<&str> = shebang.split_whitespace().collect();
    let interpreter = if parts.first().is_some_and(|p| p.ends_with("/env")) {
        parts.get(1).copied().unwrap_or("")
    } else {
        parts.first().copied().unwrap_or("")
    };
    let basename = interpreter.rsplit('/').next().unwrap_or(interpreter);
    matches!(basename, "sh" | "bash" | "zsh" | "dash" | "ash" | "ksh" | "attest")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn discover_single_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("example.sh");
        fs::write(&file, "#!/bin/bash\necho hello\n").unwrap();

        let result = discover_test_files(&file).unwrap();
        assert_eq!(result, vec![file]);
    }

    #[test]
    fn discover_directory_finds_shell_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.sh"), "#!/bin/bash\n").unwrap();
        fs::write(tmp.path().join("b.test"), "#!/bin/bash\n").unwrap();
        fs::write(tmp.path().join("c.txt"), "not a script\n").unwrap();

        let result = discover_test_files(tmp.path()).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|p| p.ends_with("a.sh")));
        assert!(result.iter().any(|p| p.ends_with("b.test")));
    }

    #[test]
    fn discover_directory_recursive() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(tmp.path().join("top.sh"), "#!/bin/bash\n").unwrap();
        fs::write(sub.join("nested.bash"), "#!/bin/bash\n").unwrap();

        let result = discover_test_files(tmp.path()).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn discover_empty_directory_errors() {
        let tmp = TempDir::new().unwrap();
        let result = discover_test_files(tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn discover_nonexistent_path_errors() {
        let result = discover_test_files(Path::new("/nonexistent/path/xyz"));
        assert!(result.is_err());
    }

    #[test]
    fn discover_results_are_sorted() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("z.sh"), "#!/bin/bash\n").unwrap();
        fs::write(tmp.path().join("a.sh"), "#!/bin/bash\n").unwrap();
        fs::write(tmp.path().join("m.sh"), "#!/bin/bash\n").unwrap();

        let result = discover_test_files(tmp.path()).unwrap();
        let sorted: Vec<_> = {
            let mut v = result.clone();
            v.sort();
            v
        };
        assert_eq!(result, sorted);
    }

    #[test]
    fn shell_script_by_extension() {
        let tmp = TempDir::new().unwrap();
        for ext in &["sh", "bash", "test"] {
            let file = tmp.path().join(format!("file.{ext}"));
            fs::write(&file, "no shebang\n").unwrap();
            assert!(is_shell_script(&file), "expected {ext} to be recognized");
        }
    }

    #[test]
    fn shell_script_detected_by_shebang() {
        let tmp = TempDir::new().unwrap();

        let bash_file = tmp.path().join("direct");
        fs::write(&bash_file, "#!/bin/bash\necho hi\n").unwrap();
        assert!(is_shell_script(&bash_file));

        let env_file = tmp.path().join("env_style");
        fs::write(&env_file, "#!/usr/bin/env bash\necho hi\n").unwrap();
        assert!(is_shell_script(&env_file));

        let python_file = tmp.path().join("not_shell");
        fs::write(&python_file, "#!/usr/bin/python3\nprint('hi')\n").unwrap();
        assert!(!is_shell_script(&python_file));
    }

    #[test]
    fn interpreter_detection() {
        assert!(is_shell_interpreter("#!/bin/sh"));
        assert!(is_shell_interpreter("#!/bin/bash"));
        assert!(is_shell_interpreter("#!/usr/bin/env bash"));
        assert!(is_shell_interpreter("#!/usr/bin/env zsh"));
        assert!(is_shell_interpreter("#!/bin/dash"));
        assert!(is_shell_interpreter("#!/bin/ash"));
        assert!(is_shell_interpreter("#!/usr/bin/env ksh"));
        assert!(is_shell_interpreter("#!/usr/bin/attest"));
        assert!(!is_shell_interpreter("#!/usr/bin/python3"));
        assert!(!is_shell_interpreter("#!/usr/bin/env ruby"));
        assert!(!is_shell_interpreter("#!/usr/bin/env node"));
    }
}

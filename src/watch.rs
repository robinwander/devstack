use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use blake3::Hasher;
use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::overrides::OverrideBuilder;

pub fn compute_watch_hash(
    root: &Path,
    watch: Option<&[String]>,
    ignore: &[String],
    extra_files: &[PathBuf],
    fingerprint: &[u8],
) -> Result<String> {
    let overrides = build_overrides(root, watch)?;
    let ignore_matcher = build_ignore_matcher(root, ignore)?;

    let mut builder = WalkBuilder::new(root);
    builder.standard_filters(true);
    builder.hidden(false);
    builder.parents(true);
    builder.add_custom_ignore_filename(".devstackignore");
    if let Some(overrides) = overrides {
        builder.overrides(overrides);
    }

    let mut files = BTreeSet::new();
    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if is_dir {
            continue;
        }
        if let Some(ignore) = &ignore_matcher
            && ignore.matched_path_or_any_parents(path, is_dir).is_ignore()
        {
            continue;
        }
        files.insert(path.to_path_buf());
    }

    for extra in extra_files {
        files.insert(extra.to_path_buf());
    }

    let mut hasher = Hasher::new();
    hasher.update(b"devstack-watch-v1");
    hasher.update(fingerprint);

    for path in files {
        let display = path.strip_prefix(root).unwrap_or(&path);
        hash_path_metadata(&mut hasher, display, &path)?;
    }

    Ok(hasher.finalize().to_hex().to_string())
}

fn build_overrides(
    root: &Path,
    watch: Option<&[String]>,
) -> Result<Option<ignore::overrides::Override>> {
    let watch = watch
        .map(|patterns| {
            patterns
                .iter()
                .filter(|p| !p.trim().is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|patterns| !patterns.is_empty());
    let mut builder = OverrideBuilder::new(root);
    let mut any = false;

    if let Some(patterns) = watch {
        for pattern in patterns {
            builder
                .add(pattern)
                .with_context(|| format!("invalid watch pattern '{pattern}'"))?;
            any = true;
        }
    }

    if !any {
        return Ok(None);
    }
    let overrides = builder.build().context("build watch overrides")?;
    Ok(Some(overrides))
}

fn build_ignore_matcher(root: &Path, ignore: &[String]) -> Result<Option<Gitignore>> {
    let mut builder = GitignoreBuilder::new(root);
    let mut any = false;
    for pattern in ignore.iter().filter(|p| !p.trim().is_empty()) {
        builder
            .add_line(None, pattern)
            .with_context(|| format!("invalid ignore pattern '{pattern}'"))?;
        any = true;
    }
    if !any {
        return Ok(None);
    }
    let ignore = builder.build().context("build ignore matcher")?;
    Ok(Some(ignore))
}

fn hash_path_metadata(hasher: &mut Hasher, display: &Path, path: &Path) -> Result<()> {
    hasher.update(display.to_string_lossy().as_bytes());
    match std::fs::metadata(path) {
        Ok(meta) => {
            hasher.update(&meta.len().to_le_bytes());
            if let Ok(modified) = meta.modified()
                && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
            {
                hasher.update(&duration.as_secs().to_le_bytes());
                hasher.update(&duration.subsec_nanos().to_le_bytes());
            }
            hash_file_contents(hasher, path)?;
        }
        Err(err) => {
            hasher.update(b"missing");
            hasher.update(err.to_string().as_bytes());
        }
    }
    Ok(())
}

fn hash_file_contents(hasher: &mut Hasher, path: &Path) -> Result<()> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open watch file {}", path.to_string_lossy()))?;
    let mut buf = [0u8; 8192];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_file(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn watch_hash_changes_on_file_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("svc");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("file.txt");
        write_file(&file, "one");

        let hash_a = compute_watch_hash(&root, None, &[], &[], b"config").unwrap();

        write_file(&file, "two");
        let hash_b = compute_watch_hash(&root, None, &[], &[], b"config").unwrap();
        assert_ne!(hash_a, hash_b);
    }

    #[cfg(unix)]
    #[test]
    fn watch_hash_changes_when_contents_change_but_metadata_is_same() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("svc");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("file.txt");
        write_file(&file, "abcd");

        let original_modified = std::fs::metadata(&file)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap();
        let hash_a = compute_watch_hash(&root, None, &[], &[], b"config").unwrap();

        write_file(&file, "wxyz");

        let c_path = CString::new(file.as_os_str().as_bytes()).unwrap();
        let times = [
            libc::timespec {
                tv_sec: 0,
                tv_nsec: libc::UTIME_OMIT,
            },
            libc::timespec {
                tv_sec: original_modified.as_secs() as libc::time_t,
                tv_nsec: original_modified.subsec_nanos() as libc::c_long,
            },
        ];
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0);

        let hash_b = compute_watch_hash(&root, None, &[], &[], b"config").unwrap();
        assert_ne!(hash_a, hash_b);
    }

    #[test]
    fn watch_hash_ignores_devstackignore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("svc");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        write_file(&dir.path().join(".devstackignore"), "ignored.txt\n");

        let ignored = root.join("ignored.txt");
        write_file(&ignored, "one");
        let hash_a = compute_watch_hash(&root, None, &[], &[], b"config").unwrap();
        write_file(&ignored, "two");
        let hash_b = compute_watch_hash(&root, None, &[], &[], b"config").unwrap();
        assert_eq!(hash_a, hash_b);
    }

    #[test]
    fn watch_hash_respects_watch_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("svc");
        fs::create_dir_all(root.join("src")).unwrap();
        write_file(&root.join("src").join("lib.rs"), "one");
        write_file(&root.join("Cargo.toml"), "one");

        let hash_a =
            compute_watch_hash(&root, Some(&["src/**".to_string()]), &[], &[], b"config").unwrap();

        write_file(&root.join("Cargo.toml"), "two");
        let hash_b =
            compute_watch_hash(&root, Some(&["src/**".to_string()]), &[], &[], b"config").unwrap();
        assert_eq!(hash_a, hash_b);
    }

    #[test]
    fn watch_hash_respects_ignore_reinclude() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("svc");
        fs::create_dir_all(&root).unwrap();
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        write_file(&a, "one");
        write_file(&b, "one");

        let ignore = vec!["*.txt".to_string(), "!a.txt".to_string()];
        let hash_a = compute_watch_hash(&root, None, &ignore, &[], b"config").unwrap();

        write_file(&b, "two");
        let hash_b = compute_watch_hash(&root, None, &ignore, &[], b"config").unwrap();
        assert_eq!(hash_a, hash_b);

        write_file(&a, "two");
        let hash_c = compute_watch_hash(&root, None, &ignore, &[], b"config").unwrap();
        assert_ne!(hash_a, hash_c);
    }
}

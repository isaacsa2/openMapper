//! A failed installation rolls back the files already changed. Staging lives beside each
//! destination, so renames never cross file systems. This is not crash recovery: the
//! map's regular backups are still needed after a power loss or a failed rollback.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::safe::Op;

static SERIAL: AtomicU64 = AtomicU64::new(0);

struct Staged {
    dir: PathBuf,
    original: bool,
}

#[derive(Default)]
struct Staging {
    entries: Vec<Staged>,
    preserve: bool,
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.preserve {
            for entry in &self.entries {
                if let Err(e) = std::fs::remove_dir_all(&entry.dir) {
                    log::warn!("{}: {e}", entry.dir.display());
                }
            }
        }
    }
}

fn directory(parent: &Path) -> io::Result<PathBuf> {
    loop {
        let n = SERIAL.fetch_add(1, Ordering::Relaxed);
        let dir = parent.join(format!(".openmapper-save-{}-{n}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Ok(()) => return Ok(dir),
            Err(e) => return Err(e),
        }
    }
}

fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

pub fn commit(ops: &[Op]) -> anyhow::Result<()> {
    install(ops, |_, op, dir| match op {
        Op::Write(path, _) => std::fs::rename(dir.join("new"), path),
        Op::Remove(path) => remove_file(path),
    })
}

fn install(ops: &[Op], mut apply: impl FnMut(usize, &Op, &Path) -> io::Result<()>) -> anyhow::Result<()> {
    let mut staging = Staging::default();
    let mut paths = std::collections::HashSet::new();
    // No destination changes until both its replacement and its rollback copy are ready.
    for op in ops {
        let path = op.path();
        anyhow::ensure!(paths.insert(path), "duplicate save destination: {}", path.display());
        let original = match std::fs::symlink_metadata(path) {
            Ok(m) => {
                anyhow::ensure!(m.is_file(), "not a regular file: {}", path.display());
                true
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let dir = directory(parent)?;
        staging.entries.push(Staged { dir: dir.clone(), original });
        if original {
            let old = dir.join("old");
            let mut source = std::fs::File::open(path)?;
            let mut copy = std::fs::File::create(&old)?;
            io::copy(&mut source, &mut copy)?;
            copy.sync_all()?;
        }
        if let Op::Write(_, bytes) = op {
            let mut file = std::fs::File::create(dir.join("new"))?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
    }
    for (i, (op, entry)) in ops.iter().zip(&staging.entries).enumerate() {
        if let Err(error) = apply(i, op, &entry.dir) {
            let mut failed = Vec::new();
            for (done, kept) in ops[..i].iter().zip(&staging.entries[..i]).rev() {
                let result = if kept.original {
                    std::fs::rename(kept.dir.join("old"), done.path())
                } else {
                    remove_file(done.path())
                };
                if let Err(e) = result {
                    failed.push(format!("{}: {e} (original: {})", done.path().display(), kept.dir.join("old").display()));
                }
            }
            if !failed.is_empty() {
                staging.preserve = true;
                anyhow::bail!("{}: {error}; rollback failed: {}", op.path().display(), failed.join("; "));
            }
            anyhow::bail!("{}: {error}; changes rolled back", op.path().display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_late_failure_restores_replaced_removed_and_new_files() {
        let root = std::env::temp_dir().join(format!("openmapper-rollback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("replace"), b"old").unwrap();
        std::fs::write(root.join("remove"), b"keep").unwrap();
        let ops = vec![
            Op::Write(root.join("replace"), b"new".to_vec()),
            Op::Remove(root.join("remove")),
            Op::Write(root.join("created"), b"new".to_vec()),
            Op::Write(root.join("last"), b"last".to_vec()),
        ];
        for fail_at in 0..ops.len() {
            let result = install(&ops, |i, op, dir| {
                if i == fail_at {
                    return Err(io::Error::other("simulated filesystem failure"));
                }
                match op {
                    Op::Write(path, _) => std::fs::rename(dir.join("new"), path),
                    Op::Remove(path) => remove_file(path),
                }
            });
            assert!(result.is_err());
            assert_eq!(std::fs::read(root.join("replace")).unwrap(), b"old");
            assert_eq!(std::fs::read(root.join("remove")).unwrap(), b"keep");
            assert!(!root.join("created").exists());
            assert!(!root.join("last").exists());
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        }
        commit(&ops).unwrap();
        assert_eq!(std::fs::read(root.join("replace")).unwrap(), b"new");
        assert!(!root.join("remove").exists());
        assert!(root.join("created").is_file() && root.join("last").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preparation_failure_cleans_up_without_changing_files() {
        let root = std::env::temp_dir().join(format!("openmapper-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("blocked")).unwrap();
        std::fs::write(root.join("first"), b"old").unwrap();
        let ops = [Op::Write(root.join("first"), b"new".to_vec()), Op::Write(root.join("blocked"), vec![])];
        assert!(commit(&ops).is_err());
        assert_eq!(std::fs::read(root.join("first")).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_failed_rollback_keeps_the_original_for_manual_recovery() {
        let root = std::env::temp_dir().join(format!("openmapper-rollback-failed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first");
        std::fs::write(&first, b"original").unwrap();
        let ops = [Op::Remove(first.clone()), Op::Write(root.join("last"), vec![])];
        let error = install(&ops, |i, op, _| {
            if i == 0 {
                return std::fs::remove_file(op.path());
            }
            // Another process puts a directory where the original must be restored.
            std::fs::create_dir(&first)?;
            Err(io::Error::other("simulated installation failure"))
        }).unwrap_err();
        assert!(error.to_string().contains("rollback failed"));
        let originals: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.path().join("old")).filter(|p| p.is_file()).collect();
        assert_eq!(originals.len(), 1);
        assert_eq!(std::fs::read(&originals[0]).unwrap(), b"original");
        std::fs::remove_dir_all(root).unwrap();
    }
}

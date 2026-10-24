//! Hot reload: a background thread watches the object, spline and texture folders of the
//! content folder (often) and of the installation (now and then) and says when files there
//! came, went or changed, so the catalogue and the models follow without a restart.
//!
//! Changes the editor makes itself (it writes objects, textures and splines too) are not
//! reported: [`own_change`] marks them.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Folders below a root that hold what the editor shows.
const WATCHED: [&str; 3] = ["Sceneryobjects", "Splines", "Texture"];

/// How long the files must stay as they are before a change is reported (a copy of a big
/// folder is one change, not hundreds).
const SETTLE: Duration = Duration::from_millis(900);

/// A change the editor wrote itself within this time of a change is its own.
const OWN_WINDOW: Duration = Duration::from_millis(2500);

static OWN: Mutex<Option<Instant>> = Mutex::new(None);

/// The editor wrote content files just now: the watcher takes them as already known. Also
/// tells openOMSI's file cache.
pub fn own_change() {
    omsi_cfg::content_changed();
    *OWN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

fn recently_own() -> bool {
    OWN.lock().unwrap_or_else(|e| e.into_inner()).map(|t| t.elapsed() < OWN_WINDOW).unwrap_or(false)
}

pub struct Watcher {
    rx: Receiver<usize>,
    stop: Arc<AtomicBool>,
}

impl Watcher {
    /// Watch `content` (every 1.5 s) and `omsi` (every 6 s).
    pub fn start(content: PathBuf, omsi: PathBuf) -> Watcher {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let spawned = std::thread::Builder::new().name("hot-reload".into()).spawn(move || {
            let mut prints = [fingerprint(&content), fingerprint(&omsi)];
            let mut tick = 0u32;
            while !flag.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1500));
                tick = tick.wrapping_add(1);
                let roots: [(&Path, bool); 2] = [(&content, true), (&omsi, tick % 4 == 0)];
                let mut changed = false;
                for (i, (root, due)) in roots.into_iter().enumerate() {
                    if !due {
                        continue;
                    }
                    let now = fingerprint(root);
                    if now != prints[i] {
                        // (wait until it has stopped changing)
                        let mut last = now;
                        loop {
                            std::thread::sleep(SETTLE);
                            let again = fingerprint(root);
                            if again == last || flag.load(Ordering::Relaxed) {
                                last = again;
                                break;
                            }
                            last = again;
                        }
                        prints[i] = last;
                        if recently_own() {
                            log::debug!("hot reload: {} changed by the editor itself", root.display());
                        } else {
                            changed = true;
                        }
                    }
                }
                if changed && tx.send(tick as usize).is_err() {
                    break;
                }
            }
        });
        if let Err(e) = spawned {
            log::warn!("hot reload: could not start the watcher: {e}");
        }
        Watcher { rx, stop }
    }

    /// Whether files changed since the last call.
    pub fn poll(&self) -> bool {
        let mut any = false;
        while self.rx.try_recv().is_ok() {
            any = true;
        }
        any
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A number that changes when any file below the watched folders of `root` is added,
/// removed, resized or written to.
fn fingerprint(root: &Path) -> u64 {
    let mut h = 0u64;
    for name in WATCHED {
        let Some(dir) = child(root, name) else { continue };
        visit(&dir, &mut h, 0);
    }
    h
}

fn child(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.is_dir() && p.file_name().map(|f| f.to_string_lossy().eq_ignore_ascii_case(name)).unwrap_or(false))
}

fn visit(dir: &Path, acc: &mut u64, depth: u32) {
    if depth > 12 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        let p = e.path();
        if meta.is_dir() {
            visit(&p, acc, depth + 1);
            continue;
        }
        // (files being written have a temporary name of ours or a download's)
        if p.extension().map(|x| matches!(x.to_string_lossy().to_ascii_lowercase().as_str(), "part" | "tmp" | "crdownload")).unwrap_or(false) {
            continue;
        }
        let mut one = DefaultHasher::new();
        p.hash(&mut one);
        meta.len().hash(&mut one);
        meta.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_millis()).hash(&mut one);
        // (a sum: the order the folder lists in does not matter)
        *acc = acc.wrapping_add(one.finish());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_file_changes_the_fingerprint() {
        let dir = std::env::temp_dir().join(format!("openmapper-hot-{}", std::process::id()));
        let objs = dir.join("sceneryobjects").join("My");
        std::fs::create_dir_all(&objs).unwrap();
        let before = fingerprint(&dir);
        assert_eq!(before, fingerprint(&dir));
        std::fs::write(objs.join("house.sco"), "x").unwrap();
        let added = fingerprint(&dir);
        assert_ne!(before, added);
        std::fs::write(objs.join("house.sco"), "xy").unwrap();
        assert_ne!(added, fingerprint(&dir));
        std::fs::write(objs.join("house.part"), "partial").unwrap();
        assert_eq!(fingerprint(&dir), fingerprint(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

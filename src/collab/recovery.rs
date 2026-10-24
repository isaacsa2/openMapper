//! Keep local work before the host's conflicting state replaces it. Separate from map
//! saves and undo: these bytes can be read even after leaving or restarting the editor.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use openmapper_collab::Part;
use serde::{Deserialize, Serialize};

use crate::doc::Document;

static SERIAL: AtomicU64 = AtomicU64::new(0);

// wire::State serialises only the presence of data, not the bytes themselves.
#[derive(Serialize, Deserialize)]
struct SavedPart {
    part: Part,
    data: Option<Vec<u8>>,
}

pub fn keep(doc: &Document, parts: &[Part]) -> anyhow::Result<PathBuf> {
    let dir = doc.dir.join("openMapper").join("conflicts");
    std::fs::create_dir_all(&dir)?;
    let saved: Vec<SavedPart> = parts.iter().map(|&part| SavedPart { part, data: super::sync::bytes_of(doc, part) }).collect();
    let bytes = serde_json::to_vec(&saved)?;
    loop {
        let n = SERIAL.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("{}-{}-{n}.json", crate::doc::safe::stamp(), std::process::id()));
        let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            result => result?,
        };
        if let Err(e) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(e.into());
        }
        return Ok(path);
    }
}

/// Restore only these parts, after leaving the session. Validate the whole copy before
/// changing the open document, and make the recovery one undoable edit.
pub fn restore(path: &std::path::Path, doc: &mut Document, history: &mut crate::history::History) -> anyhow::Result<()> {
    anyhow::ensure!(std::fs::metadata(path)?.len() <= 512 << 20, "recovery file too large");
    let mut saved: Vec<SavedPart> = serde_json::from_slice(&std::fs::read(path)?)?;
    saved.sort_by_key(|s| s.part != Part::Global);
    let mut candidate = super::placeholder();
    candidate.global = doc.global.clone();
    candidate.order = doc.order.clone();
    candidate.tiles = doc.tiles.clone();
    candidate.groups = doc.groups.clone();
    for s in &saved {
        let ok = match s.part {
            Part::Tile(x, y) => super::codec::apply_tile(&mut candidate, (x, y), s.data.as_deref()),
            Part::Global => s.data.as_deref().map(|b| super::codec::apply_global(&mut candidate, b)).unwrap_or(false),
            Part::Groups => s.data.as_deref().map(|b| super::codec::apply_groups(&mut candidate, b)).unwrap_or(false),
            _ => false,
        };
        anyhow::ensure!(ok, "unreadable recovery part: {:?}", s.part);
    }
    let mark = history.mark();
    if saved.iter().any(|s| s.part == Part::Groups) {
        history.push_snapshot("recover conflicting edit", crate::history::Snapshot { groups: Some(doc.groups.clone()), ..Default::default() });
    }
    history.begin("recover conflicting edit");
    for s in &saved {
        match s.part {
            Part::Tile(x, y) => history.touch_tile(doc, (x, y)),
            Part::Global => {
                history.touch_global(doc);
                // Global ground layers can resize masks on every tile.
                for &key in doc.tiles.keys() {
                    history.touch_tile(doc, key);
                }
            }
            _ => {}
        }
    }
    doc.global = candidate.global;
    doc.order = candidate.order;
    doc.tiles = candidate.tiles;
    doc.groups = candidate.groups;
    doc.global_dirty = true;
    doc.global_rev = crate::doc::next_rev();
    history.commit();
    history.merge_since(mark, "recover conflicting edit");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicting_parts_keep_their_actual_bytes_and_deletions() {
        let mut doc = crate::selection::tests::flat_map(1);
        doc.dir = std::env::temp_dir().join(format!("openmapper-recovery-{}", std::process::id()));
        doc.tiles.get_mut(&(0, 0)).unwrap().terrain.heights[0] = 42.0;
        let path = keep(&doc, &[Part::Tile(0, 0), Part::Tile(9, 9)]).unwrap();
        let saved: Vec<SavedPart> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let tile = super::super::codec::tile_from(saved[0].data.as_deref().unwrap(), vec![]).unwrap();
        assert_eq!(tile.terrain.heights[0], 42.0);
        assert!(saved[1].data.is_none());
        doc.tiles.get_mut(&(0, 0)).unwrap().terrain.heights[0] = 7.0;
        let mut history = crate::history::History::default();
        let path = keep(&doc, &[Part::Tile(0, 0)]).unwrap();
        doc.tiles.get_mut(&(0, 0)).unwrap().terrain.heights[0] = 99.0;
        restore(&path, &mut doc, &mut history).unwrap();
        assert_eq!(doc.tiles[&(0, 0)].terrain.heights[0], 7.0);
        history.undo(&mut doc);
        assert_eq!(doc.tiles[&(0, 0)].terrain.heights[0], 99.0);
        std::fs::remove_dir_all(&doc.dir).unwrap();
    }
}

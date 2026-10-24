//! A part of the map as bytes and back, in the files' own formats where there are some (the
//! tile's `.map` text, its `.terrain`, the `global.cfg` text): what the other editor reads is
//! what a save writes and a load reads.

use omsi_map::{GlobalCfg, Terrain, Tile, TERRAIN_SAMPLES};

use crate::doc::{next_rev, Document, TileDoc, TileKey};

const TILE: &[u8; 4] = b"OMT1";

/// A tile: its `.map` text, its ground, its water and its painting masks. (Its chrono
/// patches are the editor's own reading of the folders it has, not sent.)
pub fn tile_bytes(t: &TileDoc) -> Vec<u8> {
    let mut own = t.tile.clone();
    own.has_water = t.water.is_some();
    let mut out = TILE.to_vec();
    section(&mut out, crate::doc::tile_text(&own).as_bytes());
    section(&mut out, &t.terrain.to_bytes());
    match t.water {
        Some(w) => {
            out.push(1);
            w.iter().for_each(|h| out.extend_from_slice(&h.to_le_bytes()));
        }
        None => out.push(0),
    }
    out.extend_from_slice(&(t.masks.len() as u32).to_le_bytes());
    for m in &t.masks {
        match m {
            Some(m) => {
                out.push(1);
                section(&mut out, m);
            }
            None => out.push(0),
        }
    }
    out
}

/// A tile from its bytes; `chrono`: the patches the editor has for it.
pub fn tile_from(bytes: &[u8], chrono: Vec<Tile>) -> Option<TileDoc> {
    let mut r = Reader(bytes.strip_prefix(TILE)?);
    let text = std::str::from_utf8(r.section()?).ok()?;
    let tile = Tile::parse(&omsi_cfg::CfgFile::from_str("tile.map", text));
    let terrain = Terrain::parse(r.section()?).ok().filter(|t| t.samples() == TERRAIN_SAMPLES)?;
    let water = match r.byte()? {
        1 => {
            let mut w = [0f32; 4];
            for h in &mut w {
                *h = f32::from_le_bytes(r.take(4)?.try_into().ok()?);
            }
            Some(w)
        }
        _ => None,
    };
    let n = u32::from_le_bytes(r.take(4)?.try_into().ok()?) as usize;
    // (one mask per ground layer: a few dozen at most)
    if n > 256 {
        return None;
    }
    let mut masks = Vec::with_capacity(n);
    for _ in 0..n {
        masks.push(match r.byte()? {
            1 => Some(r.section()?.to_vec()),
            _ => None,
        });
    }
    Some(TileDoc { tile, terrain, masks, chrono, water, terrain_rev: next_rev(), mask_rev: next_rev(), content_rev: next_rev(), dirty: true })
}

/// The global settings and the tile list, as `global.cfg` has them.
pub fn global_bytes(doc: &Document) -> Vec<u8> {
    crate::doc::global_text(doc).into_bytes()
}

/// Put global settings sent by another editor into the document. The IDCode it hands out
/// next stays the editor's own (each has a block of its own).
pub fn apply_global(doc: &mut Document, bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else { return false };
    let mut g = GlobalCfg::parse(&omsi_cfg::CfgFile::from_str("global.cfg", text));
    g.path = doc.global.path.clone();
    g.next_id_code = doc.global.next_id_code;
    // (the tiles' files are written under these names: one that would leave the map's folder
    // gets the plain name)
    for t in &mut g.tiles {
        if !plain_file_name(&t.file) {
            t.file = crate::doc::save::tile_file((t.x, t.y));
        }
    }
    doc.order = g.raw_tiles.clone();
    doc.global = g;
    doc.global_rev = next_rev();
    doc.global_dirty = true;
    // (ground layers added, taken away or of another mask size: every tile's masks follow)
    let keys: Vec<TileKey> = doc.tiles.keys().copied().collect();
    for k in keys {
        fit_masks(doc, k);
    }
    true
}

/// A tile's painting masks made to fit the map's ground layers: one slot per layer, each of
/// its layer's size (resampled when it is square, else dropped). Whatever draws or saves the
/// masks reads them at that size.
fn fit_masks(doc: &mut Document, k: TileKey) {
    let sizes: Vec<usize> = doc.global.ground_textures.iter().map(crate::doc::mask_size).collect();
    let Some(t) = doc.tiles.get_mut(&k) else { return };
    let mut changed = t.masks.len() != sizes.len();
    t.masks.resize(sizes.len(), None);
    for (li, m) in t.masks.iter_mut().enumerate() {
        let Some(texels) = m else { continue };
        let want = sizes[li];
        if li == 0 {
            // (the base ground has no mask)
            *m = None;
            changed = true;
        } else if texels.len() != want * want {
            let from = (texels.len() as f64).sqrt().round() as usize;
            *m = (from >= 1 && from * from == texels.len()).then(|| crate::doc::resample_mask(texels, from, want));
            changed = true;
        }
    }
    if changed {
        t.touch_masks();
    }
}

pub fn groups_bytes(doc: &Document) -> Vec<u8> {
    serde_json::to_vec(&doc.groups).unwrap_or_default()
}

pub fn apply_groups(doc: &mut Document, bytes: &[u8]) -> bool {
    match serde_json::from_slice(bytes) {
        Ok(g) => {
            doc.groups = g;
            true
        }
        Err(_) => false,
    }
}

/// Put a tile sent by another editor in (None: it was taken away). Its chrono patches stay
/// the ones this editor read.
pub fn apply_tile(doc: &mut Document, k: TileKey, bytes: Option<&[u8]>) -> bool {
    match bytes {
        Some(b) => {
            let chrono = doc.tiles.get(&k).map(|t| t.chrono.clone()).unwrap_or_default();
            let Some(t) = tile_from(b, chrono) else { return false };
            doc.tiles.insert(k, t);
            fit_masks(doc, k);
        }
        None => {
            doc.tiles.remove(&k);
        }
    }
    doc.global_dirty = true;
    true
}

/// A file name that stays in the folder it is joined to: no folders, no `..`, nothing a
/// system reads as a drive or a device.
pub fn plain_file_name(f: &str) -> bool {
    let stem = f.split('.').next().unwrap_or("").trim().to_ascii_lowercase();
    let device = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul") || ((stem.starts_with("com") || stem.starts_with("lpt")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
    !f.is_empty() && f.len() <= 200 && !f.starts_with('.') && !device && f.chars().all(|c| c.is_alphanumeric() || " _-.()+".contains(c))
}

fn section(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn section(&mut self) -> Option<&'a [u8]> {
        let n = u32::from_le_bytes(self.take(4)?.try_into().ok()?) as usize;
        self.take(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::tests::{add_object, add_spline, flat_map};

    #[test]
    fn a_tile_comes_back_as_it_went() {
        let mut doc = flat_map(1);
        add_object(&mut doc, (0, 0), 7, 10.5, 20.25);
        add_spline(&mut doc, (0, 0), 8, 30.0, 40.0, 45.0, 50.0, 0, 0);
        let t = doc.tiles.get_mut(&(0, 0)).unwrap();
        t.terrain.heights[3] = 12.5;
        t.water = Some([-2.0, -2.0, -1.5, -1.0]);
        t.masks = vec![None, Some(vec![9; 256 * 256])];
        let back = tile_from(&tile_bytes(t), Vec::new()).unwrap();
        assert_eq!(back.terrain.heights, t.terrain.heights);
        assert_eq!(back.water, t.water);
        assert_eq!(back.masks, t.masks);
        assert_eq!(back.tile.objects.len(), 1);
        assert_eq!((back.tile.objects[0].id, back.tile.objects[0].pos), (7, [10.5, 20.25, 0.0]));
        let s = &back.tile.splines[0];
        assert_eq!((s.id, s.pos, s.heading, s.length), (8, [30.0, 40.0, 0.0], 45.0, 50.0));
        // (shown again: whatever draws it builds it anew)
        assert!(back.content_rev > t.content_rev && back.dirty);
    }

    #[test]
    fn broken_bytes_are_not_a_tile() {
        let doc = flat_map(1);
        let b = tile_bytes(&doc.tiles[&(0, 0)]);
        assert!(tile_from(&b[..b.len() - 1], Vec::new()).is_none());
        assert!(tile_from(b"OMT1", Vec::new()).is_none());
        assert!(tile_from(b"nothing", Vec::new()).is_none());
    }

    #[test]
    fn global_settings_come_over_but_not_the_next_idcode() {
        let mut a = flat_map(2);
        a.global.friendly_name = "Grundorf together".into();
        a.global.next_id_code = 5_000_000;
        a.order.push((7, 7));
        let mut b = flat_map(1);
        b.global.next_id_code = 1_000_123;
        assert!(apply_global(&mut b, &global_bytes(&a)));
        assert_eq!(b.global.friendly_name, "Grundorf together");
        assert_eq!(b.global.next_id_code, 1_000_123);
        // (the tile list: the ones there, and the one listed without a tile is not written)
        assert_eq!(b.order, vec![(0, 0), (1, 0)]);
    }

    #[test]
    fn tile_files_that_leave_the_folder_get_the_plain_name() {
        let a = flat_map(2);
        let text = String::from_utf8(global_bytes(&a)).unwrap().replace("tile_1_0.map", "..\\..\\evil.map").replace("tile_0_0.map", "Tile 0 0 (alt).map");
        let mut b = flat_map(1);
        assert!(apply_global(&mut b, text.as_bytes()));
        let files: Vec<&str> = b.global.tiles.iter().map(|t| t.file.as_str()).collect();
        assert_eq!(files, vec!["Tile 0 0 (alt).map", "tile_1_0.map"]);
        for bad in ["", "../x", "a/b", "a\\b", "C:x", ".hidden", "nul\0", "NUL.map", "com1.map"] {
            assert!(!plain_file_name(bad), "{bad}");
        }
    }

    #[test]
    fn masks_of_another_size_are_made_to_fit() {
        let mut doc = flat_map(1);
        doc.global.ground_textures = vec![Default::default(), Default::default(), Default::default()];
        let size = crate::doc::mask_size(&doc.global.ground_textures[1]);
        let mut t = doc.tiles[&(0, 0)].clone();
        // a square mask of another size, one of no size a mask can have, and one too many
        t.masks = vec![Some(vec![1; 4]), Some(vec![200; 64 * 64]), Some(vec![5; 1000]), Some(vec![9; 16])];
        assert!(apply_tile(&mut doc, (0, 0), Some(&tile_bytes(&t))));
        let m = &doc.tiles[&(0, 0)].masks;
        assert_eq!(m.len(), 3);
        assert!(m[0].is_none());
        assert_eq!(m[1].as_ref().map(|m| (m.len(), m[0])), Some((size * size, 200)));
        assert!(m[2].is_none());
    }

    #[test]
    fn groups_come_over() {
        let mut a = flat_map(1);
        a.groups.push(crate::mapgroups::MapGroup { name: "Depot <b>".into(), ids: vec![7, 8] });
        let mut b = flat_map(1);
        assert!(apply_groups(&mut b, &groups_bytes(&a)));
        assert_eq!(b.groups, a.groups);
    }
}

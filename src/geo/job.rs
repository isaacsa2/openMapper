//! Loading imagery and heights for a list of tiles on a background thread, one tile after
//! another, telling the window how far it is.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use glam::DVec2;

use super::fetch;
use super::sources::{GeoPrefs, Source};
use super::{elevation, imagery, Georef, ImageryMeta, TerrainOpts};
use crate::doc::TileKey;
use crate::i18n::tr;

/// What to load.
pub struct Spec {
    pub dir: PathBuf,
    pub geo: Georef,
    pub ts: f64,
    /// The tiles and their terrain cells.
    pub tiles: Vec<(TileKey, usize)>,
    /// Imagery: the source (several: the sharpest of them there), the zoom (0: the finest
    /// it has there) and the picture's edge (0: the source's own detail).
    pub imagery: Option<(Vec<Source>, u8, u32)>,
    /// Heights: the source, the options and the tile whose mean is taken off.
    pub terrain: Option<(Source, TerrainOpts, TileKey)>,
    pub prefs: GeoPrefs,
}

pub enum Msg {
    /// A tile's imagery is written (or, not to be kept, given with it).
    Image(TileKey, ImageryMeta, Option<image::RgbaImage>),
    /// A tile's new heights (offsets applied).
    Heights(TileKey, Vec<f32>),
    /// The height `relative` takes off, fixed now.
    Base(f32),
    /// The zoom the imagery is made at, and its detail (m a pixel).
    Zoom(u8, f64),
    Error(TileKey, String),
    /// Ended (Some: stopped by this error).
    Done(Option<String>),
}

pub struct Job {
    pub rx: Receiver<Msg>,
    pub cancel: Arc<AtomicBool>,
    pub done: Arc<AtomicUsize>,
    pub total: usize,
    pub dir: PathBuf,
}

pub fn start(spec: Spec) -> Job {
    let (tx, rx) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicUsize::new(0));
    let job = Job { rx, cancel: cancel.clone(), done: done.clone(), total: spec.tiles.len(), dir: spec.dir.clone() };
    std::thread::Builder::new().name("geo-job".into()).stack_size(16 << 20).spawn(move || {
        // (a panic on an odd answer ends the job with a message, it does not leave the window
        // waiting for a job that is gone)
        let end = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&spec, &tx, &cancel, &done))).unwrap_or_else(|_| Err(tr("Stopped by an internal error (see the log)", "Остановлено внутренней ошибкой (см. журнал)").into()));
        if let Err(e) = &end {
            log::warn!("geo job ended: {e}");
        }
        let _ = tx.send(Msg::Done(end.err()));
    }).expect("start the geo job thread");
    job
}

/// The sharpest of `srcs` at (lat, lon) and its finest zoom there (each asked at once).
fn best(srcs: &[Source], prefs: &GeoPrefs, lat: f64, lon: f64) -> Result<(Source, u8), String> {
    let found: Vec<Result<Option<u8>, String>> = std::thread::scope(|s| {
        let hs: Vec<_> = srcs.iter().map(|src| s.spawn(move || fetch::finest_zoom(src, prefs, lat, lon))).collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("?".into()))).collect()
    });
    let mut v: Vec<(Source, Option<f64>)> = Vec::new();
    let mut zooms = std::collections::HashMap::new();
    let mut err = None;
    for (src, r) in srcs.iter().zip(found) {
        match r {
            Ok(Some(z)) => {
                zooms.insert(src.name.clone(), z);
                v.push((src.clone(), Some(src.metres_per_pixel(lat, z))));
            }
            Ok(None) => {}
            Err(e) => err = Some(e),
        }
    }
    super::sources::rank(&mut v);
    match v.into_iter().next() {
        Some((s, _)) => {
            let z = zooms[&s.name];
            Ok((s, z))
        }
        None => Err(err.unwrap_or_else(|| tr("No source has imagery here", "Здесь нет снимков ни у одного источника").into())),
    }
}

fn run(spec: &Spec, tx: &Sender<Msg>, cancel: &AtomicBool, done: &AtomicUsize) -> Result<(), String> {
    let mid = spec.tiles.first().map(|t| crate::doc::tile_origin(t.0) + DVec2::splat(spec.ts * 0.5)).unwrap_or_default();
    let (lat, lon) = spec.geo.lat_lon(mid);
    // the zoom and the picture's size, the same for every tile (one source, one zoom: the
    // tiles meet without a seam)
    let img = match &spec.imagery {
        Some((srcs, z, size)) => {
            let (src, z) = match (srcs.as_slice(), *z) {
                ([one], _) if one.kind == super::sources::Kind::OsmMap => (one.clone(), 19),
                ([one], z) if z > 0 => (one.clone(), z),
                _ => best(srcs, &spec.prefs, lat, lon)?,
            };
            // (the drawn map is made at the picture's own size: as sharp as the picture can be)
            let drawn = src.kind == super::sources::Kind::OsmMap;
            let mpp = if drawn { spec.ts / if *size > 0 { *size } else { 4096 } as f64 } else { src.metres_per_pixel(lat, z) };
            let size = if *size > 0 { *size } else if drawn { 4096 } else { imagery::image_size(spec.ts, mpp) };
            let _ = tx.send(Msg::Zoom(z, mpp));
            Some((src, z, size))
        }
        None => None,
    };
    let img = img.as_ref().map(|(s, z, size)| (s, *z, *size));
    let terrain = match &spec.terrain {
        Some((src, opts, anchor)) => {
            let cells = spec.tiles.first().map(|t| t.1).unwrap_or(60);
            let z = elevation::zoom_for(lat, spec.ts / cells as f64, src.max_zoom);
            let fetch = |z: u8, x: u32, y: u32| fetch::tile(src, &spec.prefs, z, x, y);
            let base = match (opts.relative, opts.base) {
                (false, _) => 0.0,
                (true, Some(b)) => b,
                (true, None) => {
                    let raw = elevation::sample_tile(&spec.geo, *anchor, spec.ts, cells, z, 0.0, &fetch, cancel)?;
                    let b = (elevation::mean(&raw) * 100.0).round() / 100.0;
                    let _ = tx.send(Msg::Base(b));
                    b
                }
            };
            Some((src, opts, z, base))
        }
        None => None,
    };
    // (the drawn map: what OpenStreetMap has for all the tiles asked for once, not a request
    // for each tile; if it does not answer, nothing more is asked)
    let drawn = match &img {
        Some((src, _, _)) if src.kind == super::sources::Kind::OsmMap => {
            let keys: Vec<crate::doc::TileKey> = spec.tiles.iter().map(|t| t.0).collect();
            Some(crate::osm_map::fetch(crate::osm_map::bbox(&spec.geo, &keys, spec.ts))?)
        }
        _ => None,
    };
    let mut failed = 0;
    for &(k, cells) in &spec.tiles {
        if cancel.load(Ordering::Relaxed) {
            return Err(tr("Cancelled", "Отменено").into());
        }
        let one = || -> Result<(), String> {
            if let Some((src, z, size)) = &img {
                let fetch = |z: u8, x: u32, y: u32| fetch::tile(src, &spec.prefs, z, x, y);
                let (pic, coarser) = if src.kind == super::sources::Kind::OsmMap {
                    (crate::osm_map::build(&spec.geo, k, spec.ts, *size, drawn.as_ref().ok_or("no data")?, cancel)?, false)
                } else {
                    imagery::build(&spec.geo, k, spec.ts, *z, *size, &fetch, cancel)?
                };
                let kept = src.kind != super::sources::Kind::Google;
                let path = super::imagery_path(&spec.dir, k);
                let memory = if kept {
                    imagery::save_jpeg(&pic, &path)?;
                    None
                } else {
                    // (an older file of this tile must not stand for it)
                    let _ = std::fs::remove_file(&path);
                    Some(image::DynamicImage::ImageRgb8(pic).to_rgba8())
                };
                let _ = tx.send(Msg::Image(k, ImageryMeta { source: src.name.clone(), zoom: *z, stamp: spec.geo.stamp(), coarser, kept }, memory));
            }
            if let Some((src, opts, z, base)) = &terrain {
                let fetch = |z: u8, x: u32, y: u32| fetch::tile(src, &spec.prefs, z, x, y);
                let raw = elevation::sample_tile(&spec.geo, k, spec.ts, cells, *z, opts.smooth as f64, &fetch, cancel)?;
                let _ = tx.send(Msg::Heights(k, raw.iter().map(|h| h - base + opts.offset).collect()));
            }
            Ok(())
        };
        // (a tile that fails is tried again before it is given up: a dropped connection or a
        // busy server is not the end of that tile)
        let mut result = one();
        for attempt in 1..=2 {
            if result.is_ok() || cancel.load(Ordering::Relaxed) {
                break;
            }
            log::info!("tile {k:?}: {:?}, trying again ({attempt})", result.as_ref().err());
            std::thread::sleep(std::time::Duration::from_millis(800 * attempt));
            result = one();
        }
        match result {
            Ok(()) => failed = 0,
            Err(e) => {
                if cancel.load(Ordering::Relaxed) {
                    return Err(e);
                }
                let _ = tx.send(Msg::Error(k, e.clone()));
                failed += 1;
                // (offline, or the server refuses: no use trying every tile)
                if failed >= 3 {
                    return Err(e);
                }
            }
        }
        done.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

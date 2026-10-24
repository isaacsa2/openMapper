//! A tile's imagery: the source's tiles around the map tile fetched, laid side by side
//! (a mosaic), and resampled into one picture covering exactly the map tile. Each pixel is
//! taken at its own world position, so two map tiles side by side meet without a seam.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use glam::DVec2;
use image::RgbImage;

use super::fetch::{Error, Tile};
use super::Georef;
use crate::doc::TileKey;
use crate::i18n::tr;

/// Gets one source tile (from the cache or the network).
pub type Fetch<'a> = &'a (dyn Fn(u8, u32, u32) -> Result<Tile, Error> + Sync);

/// How far up a missing tile is looked for in coarser zooms.
const UP: u8 = 5;

/// The source's tiles over an area, side by side.
pub struct Mosaic {
    z: u8,
    x0: u32,
    y0: u32,
    nx: u32,
    ny: u32,
    /// Pixels a side of one tile.
    px: u32,
    rgb: Vec<u8>,
    /// Some tiles came from a coarser zoom (the source had nothing finer there).
    pub coarser: bool,
}

impl Mosaic {
    /// The tiles of zoom `z` covering Web Mercator `lo..hi`. `heights`: the tiles are
    /// Terrarium heights (a coarser tile is enlarged without blending its codes).
    pub fn fetch(z: u8, lo: DVec2, hi: DVec2, heights: bool, fetch: Fetch, cancel: &AtomicBool) -> Result<Mosaic, Error> {
        let n = 1u32 << z;
        let cell = |v: f64| ((v * n as f64).floor().max(0.0) as u32).min(n - 1);
        let (x0, x1, y0, y1) = (cell(lo.x), cell(hi.x), cell(lo.y), cell(hi.y));
        let (nx, ny) = (x1 - x0 + 1, y1 - y0 + 1);
        if nx * ny > 40 * 40 {
            return Err(tr("The area is too large for this zoom", "Слишком большая область для этого масштаба").into());
        }
        let jobs: Vec<(u32, u32)> = (0..ny).flat_map(|j| (0..nx).map(move |i| (i, j))).collect();
        let cancelled = || -> Error { tr("Cancelled", "Отменено").into() };
        // (no picture at all: grey, or 0 m)
        let empty = if heights { [128u8, 0, 0] } else { [128u8, 128, 128] };
        // the first tile tells the pixels a tile has; every later one is copied into the
        // mosaic as it arrives, so the tiles are never held twice
        let first = one(z, x0, y0, heights, fetch)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        let px = first.as_ref().map(|t| t.0.width()).unwrap_or(256).max(1);
        let mut pixels = vec![0u8; nx as usize * px as usize * ny as usize * px as usize * 3];
        for c in pixels.chunks_exact_mut(3) {
            c.copy_from_slice(&empty);
        }
        struct Out {
            rgb: Vec<u8>,
            coarser: bool,
        }
        let out = Mutex::new(Out { rgb: pixels, coarser: false });
        let place = |i: usize, t: Option<(RgbImage, bool)>| {
            let (ix, iy) = jobs[i];
            let img = match t {
                Some((img, up)) => {
                    let img = if img.width() == px && img.height() == px {
                        img
                    } else {
                        image::imageops::resize(&img, px, px, if heights { image::imageops::FilterType::Nearest } else { image::imageops::FilterType::Triangle })
                    };
                    Some((img, up))
                }
                None => None,
            };
            let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
            match img {
                Some((img, up)) => {
                    o.coarser |= up;
                    for row in 0..px {
                        let dst = (((iy * px + row) * nx * px + ix * px) * 3) as usize;
                        let src = (row * px * 3) as usize;
                        o.rgb[dst..dst + (px * 3) as usize].copy_from_slice(&img.as_raw()[src..src + (px * 3) as usize]);
                    }
                }
                None => o.coarser = true,
            }
        };
        place(0, first);
        let next = Mutex::new(1usize);
        let err: Mutex<Option<Error>> = Mutex::new(None);
        std::thread::scope(|s| {
            for _ in 0..6 {
                s.spawn(|| loop {
                    let i = {
                        let mut n = next.lock().unwrap_or_else(|e| e.into_inner());
                        let i = *n;
                        *n += 1;
                        i
                    };
                    if i >= jobs.len() || cancel.load(Ordering::Relaxed) || err.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
                        return;
                    }
                    let (ix, iy) = jobs[i];
                    // (a panic on an odd picture is that tile's failure, not the end of the job)
                    let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| one(z, x0 + ix, y0 + iy, heights, fetch))).unwrap_or_else(|_| Err("a tile could not be read".into()));
                    match got {
                        Ok(t) => place(i, t),
                        Err(e) => *err.lock().unwrap_or_else(|e| e.into_inner()) = Some(e),
                    }
                });
            }
        });
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        if let Some(e) = err.into_inner().unwrap_or_else(|e| e.into_inner()) {
            return Err(e);
        }
        let Out { rgb, coarser } = out.into_inner().unwrap_or_else(|e| e.into_inner());
        Ok(Mosaic { z, x0, y0, nx, ny, px, rgb, coarser })
    }

    /// Where a Web Mercator position falls, in the mosaic's pixels (pixel centres at .5).
    fn pixel(&self, m: DVec2) -> (f64, f64) {
        let s = (1u64 << self.z) as f64 * self.px as f64;
        (m.x * s - (self.x0 * self.px) as f64 - 0.5, m.y * s - (self.y0 * self.px) as f64 - 0.5)
    }

    fn corners(&self, m: DVec2) -> ([usize; 4], f32, f32) {
        let (w, h) = ((self.nx * self.px) as i64, (self.ny * self.px) as i64);
        let (fx, fy) = self.pixel(m);
        let (ix, iy) = (fx.floor() as i64, fy.floor() as i64);
        let (u, v) = ((fx - ix as f64) as f32, (fy - iy as f64) as f32);
        let at = |x: i64, y: i64| (y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize;
        ([at(ix, iy), at(ix + 1, iy), at(ix, iy + 1), at(ix + 1, iy + 1)], u, v)
    }

    /// The colour at a Web Mercator position (bilinear).
    pub fn colour(&self, m: DVec2) -> [u8; 3] {
        let (c, u, v) = self.corners(m);
        let mut out = [0u8; 3];
        for ch in 0..3 {
            let g = |i: usize| self.rgb[c[i] * 3 + ch] as f32;
            let top = g(0) + (g(1) - g(0)) * u;
            let bot = g(2) + (g(3) - g(2)) * u;
            out[ch] = (top + (bot - top) * v).round().clamp(0.0, 255.0) as u8;
        }
        out
    }

    /// Terrarium heights of every pixel (m).
    pub fn heights(&self) -> Vec<f32> {
        self.rgb.chunks_exact(3).map(|p| terrarium(p[0], p[1], p[2])).collect()
    }

    /// A height at a Web Mercator position (bilinear between the pixels' heights).
    pub fn height(&self, heights: &[f32], m: DVec2) -> f32 {
        let (c, u, v) = self.corners(m);
        let top = heights[c[0]] + (heights[c[1]] - heights[c[0]]) * u;
        let bot = heights[c[2]] + (heights[c[3]] - heights[c[2]]) * u;
        top + (bot - top) * v
    }
}

/// A Terrarium pixel's height (m).
pub fn terrarium(r: u8, g: u8, b: u8) -> f32 {
    (r as f32 * 256.0 + g as f32 + b as f32 / 256.0) - 32768.0
}

/// One tile, or a part of a coarser one enlarged (true) when the source has none there.
fn one(z: u8, x: u32, y: u32, heights: bool, fetch: Fetch) -> Result<Option<(RgbImage, bool)>, Error> {
    for up in 0..=UP.min(z) {
        let Tile::Data(bytes) = fetch(z - up, x >> up, y >> up)? else { continue };
        let Ok(img) = image::load_from_memory(&bytes) else { continue };
        let img = img.to_rgb8();
        if up == 0 {
            return Ok(Some((img, false)));
        }
        let part = (img.width() >> up).max(1);
        let mask = (1u32 << up) - 1;
        let sub = image::imageops::crop_imm(&img, (x & mask) * part, (y & mask) * part, part, part).to_image();
        let filter = if heights { image::imageops::FilterType::Nearest } else { image::imageops::FilterType::Triangle };
        return Ok(Some((image::imageops::resize(&sub, img.width(), img.height(), filter), true)));
    }
    Ok(None)
}

/// The Web Mercator box of a map tile (a little wider, for the pixels at its rim).
pub fn tile_box(geo: &Georef, k: TileKey, ts: f64) -> (DVec2, DVec2) {
    let o = crate::doc::tile_origin(k);
    let mut lo = DVec2::splat(f64::MAX);
    let mut hi = DVec2::splat(f64::MIN);
    for i in 0..=8 {
        for j in 0..=8 {
            if i != 0 && i != 8 && j != 0 && j != 8 {
                continue;
            }
            let m = geo.merc(o + DVec2::new(i as f64, j as f64) * ts / 8.0);
            lo = lo.min(m);
            hi = hi.max(m);
        }
    }
    let pad = (hi - lo) * 0.01;
    (lo - pad, hi + pad)
}

/// The edge of a tile's picture for imagery of `mpp` metres a pixel: every source pixel
/// kept, in steps of 256, at most 4096.
pub fn image_size(ts: f64, mpp: f64) -> u32 {
    (((ts / mpp.max(0.01)) / 256.0).ceil() as u32 * 256).clamp(512, 4096)
}

/// The picture of map tile `k`, `size` pixels a side, row 0 its north edge.
pub fn render(geo: &Georef, k: TileKey, ts: f64, size: u32, mosaic: &Mosaic) -> RgbImage {
    let o = crate::doc::tile_origin(k);
    // (the exact position every 16 pixels; between them the mapping is straight to far
    // below a pixel)
    const STEP: u32 = 16;
    let g = size.div_ceil(STEP) + 1;
    let mut grid = Vec::with_capacity((g * g) as usize);
    for j in 0..g {
        for i in 0..g {
            let (x, y) = ((i * STEP) as f64 / size as f64, (j * STEP) as f64 / size as f64);
            grid.push(geo.merc(o + DVec2::new(x * ts, (1.0 - y) * ts)));
        }
    }
    let mut img = RgbImage::new(size, size);
    for (py, row) in img.enumerate_rows_mut() {
        let fy = (py as f64 + 0.5) / STEP as f64;
        let gy = (fy.floor() as u32).min(g - 2);
        let v = fy - gy as f64;
        for (px, _, p) in row {
            let fx = (px as f64 + 0.5) / STEP as f64;
            let gx = (fx.floor() as u32).min(g - 2);
            let u = fx - gx as f64;
            let at = |i: u32, j: u32| grid[(j * g + i) as usize];
            let top = at(gx, gy).lerp(at(gx + 1, gy), u);
            let bot = at(gx, gy + 1).lerp(at(gx + 1, gy + 1), u);
            p.0 = mosaic.colour(top.lerp(bot, v));
        }
    }
    img
}

/// Fetch and make map tile `k`'s picture: (picture, some of it was coarser).
pub fn build(geo: &Georef, k: TileKey, ts: f64, z: u8, size: u32, fetch: Fetch, cancel: &AtomicBool) -> Result<(RgbImage, bool), Error> {
    let (lo, hi) = tile_box(geo, k, ts);
    let m = Mosaic::fetch(z, lo, hi, false, fetch, cancel)?;
    Ok((render(geo, k, ts, size, &m), m.coarser))
}

/// Write a picture as JPEG (under a temporary name first).
pub fn save_jpeg(img: &RgbImage, path: &std::path::Path) -> Result<(), Error> {
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("jpg.openmapper-new");
    let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut w = std::io::BufWriter::new(f);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 90).encode_image(img).map_err(|e| e.to_string())?;
    drop(w);
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

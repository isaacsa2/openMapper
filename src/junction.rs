//! Junctions made on the spot, as Transport Fever makes them: where a new road meets or
//! crosses another, both are cut back from the meeting point and a scenery object of its own
//! fills the gap. The object is ordinary OMSI content, written into openOMSI's content folder
//! (`Sceneryobjects\openMapper\<map>\junction_<id>.sco`): a flat asphalt plate (a DirectX
//! `.x` mesh) and a `[path]` for every way through it - each lane coming in joined to the
//! lanes going out of the other arms (straight on, right, left: the AI follows them as it
//! follows any crossing's paths, which openOMSI links to the roads' lanes where their ends
//! meet), and footpaths round its corners.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use glam::{DVec2, DVec3};
use omsi_geometry::SplineCurve;
use omsi_map::MapObject;

use crate::assets::Assets;
use crate::doc::{tile_of, tile_origin, Document, TileKey};
use crate::roads::{self, Seg};

/// One road running into the junction: the spline whose end (or start) is at its edge.
#[derive(Debug, Clone, Copy)]
pub struct Arm {
    pub key: TileKey,
    pub id: i64,
    /// The spline's end lies at the junction (else its start).
    pub at_end: bool,
}

/// A lane end at the junction's edge (world).
#[derive(Debug, Clone, Copy)]
struct LaneEnd {
    arm: usize,
    pos: DVec3,
    /// The heading traffic moves with there (degrees).
    flow: f64,
    kind: i32,
    width: f64,
    /// Lateral place, right of the arm's way into the junction (m).
    side: f64,
    /// Traffic comes into the junction here (else it leaves; footways: both).
    incoming: bool,
}

/// Room a junction needs round its centre for these roads (m): the widest of them plus
/// a margin for the curves.
pub fn radius(half_widths: &[f64]) -> f64 {
    half_widths.iter().cloned().fold(3.0, f64::max) * 1.15 + 4.0
}

/// Room a junction needs when its roads meet at `angle` degrees (the sharper of the two
/// angles between them): at a sharp corner the kerbs meet far out from the centre, and the
/// arms must reach past that by enough for the kerb to be rounded off wider than the
/// footway beside it.
pub fn radius_at(half_widths: &[f64], angle: f64) -> f64 {
    let hw = half_widths.iter().cloned().fold(3.0, f64::max);
    let half = (angle.abs().rem_euclid(180.0).min(180.0 - angle.abs().rem_euclid(180.0)).max(20.0) * 0.5).to_radians();
    let base = radius(half_widths);
    base.max((hw + 2.5) / half.tan()).min(base * 3.0)
}

/// The arms' geometry: where each ends, which way it goes in, its half width and lanes.
fn arm_ends(doc: &Document, assets: &mut Assets, arms: &[Arm]) -> Option<(Vec<(DVec3, f64, f64, String)>, Vec<LaneEnd>)> {
    let mut ends = Vec::new();
    let mut lanes = Vec::new();
    for (ai, a) in arms.iter().enumerate() {
        let s = doc.spline(a.key, a.id)?;
        let model = assets.spline(&s.file)?;
        let c = SplineCurve::from_map(s, tile_origin(a.key));
        let at = if a.at_end { s.length } else { 0.0 };
        let into = if a.at_end { c.heading_at(at) } else { c.heading_at(at) + 180.0 };
        let p = c.point_at(at);
        ends.push((p, roads::norm_deg(into), model.half_width as f64, s.file.clone()));
        let mirror = if s.mirror { -1.0 } else { 1.0 };
        for path in &model.def.paths {
            let x = path.start[0] as f64 * mirror;
            let z = path.start[2] as f64;
            let pos = c.offset_point(at, x, z);
            // along the spline: 0 forwards, 1 backwards, 2 both ways; (incoming, flow)
            let along = c.heading_at(at);
            let ways: Vec<(bool, f64)> = match path.direction {
                0 => vec![(a.at_end, along)],
                1 => vec![(!a.at_end, along + 180.0)],
                _ => vec![(true, if a.at_end { along } else { along + 180.0 }), (false, if a.at_end { along + 180.0 } else { along })],
            };
            // lateral place seen from the arm's way into the junction
            let side = if a.at_end { x } else { -x };
            for (incoming, flow) in ways {
                lanes.push(LaneEnd { arm: ai, pos, flow: roads::norm_deg(flow), kind: path.kind, width: path.width as f64, side, incoming });
            }
        }
    }
    Some((ends, lanes))
}

/// One `[path]` record (object frame = world less the junction's centre).
struct PathRec {
    seg: Seg,
    z0: f64,
    z1: f64,
    kind: i32,
    width: f64,
    direction: i32,
    blinker: i32,
    /// The light its traffic waits at (`[use_traffic_light]`), if any.
    light: Option<usize>,
}

/// What a junction has besides its roads: traffic lights (a plan of two phases, the main
/// road's and the side roads'), and zebra crossings on some of its arms. Kept in the
/// junction's own `.sco` as a comment line, so it is made again with them.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub lights: bool,
    /// Green for the main road and for the side roads (s).
    pub main_green: f64,
    pub side_green: f64,
    /// The arms with a zebra crossing, by their heading out of the junction (degrees).
    pub crossings: Vec<f64>,
}

impl Default for Options {
    fn default() -> Self {
        Options { lights: false, main_green: 30.0, side_green: 20.0, crossings: Vec::new() }
    }
}

const OPTIONS_TAG: &str = "openMapper junction:";

impl Options {
    fn line(&self) -> String {
        let c: Vec<String> = self.crossings.iter().map(|h| format!("{h:.1}")).collect();
        // (helpers=omsi: its spline helpers are in OMSI's order, x height forward)
        format!("{OPTIONS_TAG} lights={} main={:.0} side={:.0} crossings={} helpers=omsi", self.lights as i32, self.main_green, self.side_green, c.join(","))
    }

    /// The options written in a junction's `.sco` text (defaults when none are).
    pub fn parse(sco: &str) -> Options {
        let mut o = Options::default();
        let Some(line) = sco.lines().find(|l| l.trim_start().starts_with(OPTIONS_TAG)) else { return o };
        for kv in line.trim_start()[OPTIONS_TAG.len()..].split_whitespace() {
            let Some((k, v)) = kv.split_once('=') else { continue };
            match k {
                "lights" => o.lights = v == "1",
                "main" => o.main_green = v.parse().unwrap_or(o.main_green),
                "side" => o.side_green = v.parse().unwrap_or(o.side_green),
                "crossings" => o.crossings = v.split(',').filter_map(|x| x.parse().ok()).collect(),
                _ => {}
            }
        }
        o
    }

    /// Read them from a junction's `.sco` file.
    pub fn read(path: &Path) -> Options {
        std::fs::read(path).ok().map(|b| Options::parse(&omsi_cfg::decode_text(&b))).unwrap_or_default()
    }

    /// Whether the arm going out along `out` has a crossing.
    pub fn crossing_at(&self, out: f64) -> bool {
        self.crossings.iter().any(|h| (roads::norm_deg(h - out + 180.0) - 180.0).abs() < 20.0)
    }
}

/// The two arms most nearly in line (the main road through the junction).
fn main_road(outs: &[f64]) -> (usize, usize) {
    let mut best = (0, 1, -1.0);
    for i in 0..outs.len() {
        for j in i + 1..outs.len() {
            let d = (roads::norm_deg(outs[i] - outs[j] + 180.0) - 180.0).abs();
            if d > best.2 {
                best = (i, j, d);
            }
        }
    }
    (best.0, best.1)
}

/// The light program: light 0 the main road's, light 1 the side roads': red-yellow 2 s,
/// green, yellow 3 s, then red, with 2 s of all red between them.
fn light_program(o: &Options) -> String {
    let (gm, gs) = (o.main_green.max(5.0), o.side_green.max(5.0));
    let main_turn = 2.0 + gm + 3.0 + 2.0;
    let cycle = main_turn + 2.0 + gs + 3.0 + 2.0;
    let mut t = format!("[traffic_lights_group]\n{}\n\n", num(cycle));
    let _ = write!(t, "[traffic_light]\nMain road\n\n[phase]\n3\n2\n\n[phase]\n6\n{}\n\n[phase]\n9\n3\n\n[phase]\n0\n0\n\n", num(gm));
    let _ = write!(t, "[traffic_light]\nSide roads\n\n[phase]\n0\n{}\n\n[phase]\n3\n2\n\n[phase]\n6\n{}\n\n[phase]\n9\n3\n\n[phase]\n0\n0\n\n", num(main_turn), num(gs));
    t
}

/// The signal heads and their poles of a junction with lights: one at each arm traffic
/// comes in by, on its right (left where traffic keeps left), facing it.
pub const LIGHT_HEAD: &str = "Sceneryobjects\\Verkehrszeichen_MC\\Ampel_Kfz_1.sco";
pub const LIGHT_POLE: &str = "Sceneryobjects\\Verkehrszeichen_MC\\Mast_Ampel_klein_1.sco";

fn signed_turn(from: f64, to: f64) -> f64 {
    (to - from + 540.0).rem_euclid(360.0) - 180.0
}

/// The ways through the junction.
fn paths(lanes: &[LaneEnd], centre: DVec3, light_of: &[Option<usize>]) -> Vec<PathRec> {
    let mut out = Vec::new();
    let c2 = centre.truncate();
    let arms = lanes.iter().map(|l| l.arm).max().map(|m| m + 1).unwrap_or(0);
    // traffic: every lane coming in to the lanes going out of the other arms, matched by
    // their place counted from the right (right turns from the right, left turns from the left)
    for i in 0..arms {
        let mut ins: Vec<&LaneEnd> = lanes.iter().filter(|l| l.arm == i && l.kind == 0 && l.incoming).collect();
        ins.sort_by(|a, b| b.side.total_cmp(&a.side));
        for j in (0..arms).filter(|j| *j != i) {
            let mut outs: Vec<&LaneEnd> = lanes.iter().filter(|l| l.arm == j && l.kind == 0 && !l.incoming).collect();
            // (seen leaving: its right is the arm's left coming in)
            outs.sort_by(|a, b| a.side.total_cmp(&b.side));
            if ins.is_empty() || outs.is_empty() {
                continue;
            }
            let turn = signed_turn(ins[0].flow, outs[0].flow);
            if turn.abs() > 150.0 {
                continue;
            }
            let blinker = if turn > 30.0 { 3 } else if turn < -30.0 { 2 } else { 0 };
            let pairs: Vec<(&LaneEnd, &LaneEnd)> = if blinker == 3 {
                vec![(ins[0], outs[0])]
            } else if blinker == 2 {
                vec![(ins[ins.len() - 1], outs[outs.len() - 1])]
            } else {
                (0..ins.len().max(outs.len())).map(|k| (ins[k.min(ins.len() - 1)], outs[k.min(outs.len() - 1)])).collect()
            };
            for (a, b) in pairs {
                let pa = a.pos.truncate() - c2;
                let pb = b.pos.truncate() - c2;
                let segs = roads::biarc(pa, a.flow, pb, b.flow).or_else(|| roads::straight(pa, pb).map(|s| vec![s]));
                let Some(segs) = segs else { continue };
                let total: f64 = segs.iter().map(|s| s.length).sum::<f64>().max(0.01);
                let mut acc = 0.0;
                for (n, s) in segs.into_iter().enumerate() {
                    let z0 = a.pos.z + (b.pos.z - a.pos.z) * acc / total - centre.z;
                    acc += s.length;
                    let z1 = a.pos.z + (b.pos.z - a.pos.z) * acc / total - centre.z;
                    // (traffic waits where it comes in: the light is on the way's first piece)
                    let light = if n == 0 { light_of.get(i).copied().flatten() } else { None };
                    out.push(PathRec { seg: s, z0, z1, kind: 0, width: a.width.max(2.5), direction: 0, blinker, light });
                }
            }
        }
    }
    // footways round the corners: each arm's outermost footway lane on a side to the
    // nearest one of the next arm round the junction
    let mut order: Vec<usize> = (0..arms).collect();
    let angle = |i: usize| -> f64 {
        let l = lanes.iter().find(|l| l.arm == i);
        l.map(|l| { let d = l.pos.truncate() - c2; d.y.atan2(d.x) }).unwrap_or(0.0)
    };
    order.sort_by(|a, b| angle(*a).total_cmp(&angle(*b)));
    if arms >= 2 {
        for k in 0..arms {
            let (i, j) = (order[k], order[(k + 1) % arms]);
            if arms == 2 && k == 1 {
                break;
            }
            let walk_i: Vec<&LaneEnd> = lanes.iter().filter(|l| l.arm == i && l.kind == 1).collect();
            let walk_j: Vec<&LaneEnd> = lanes.iter().filter(|l| l.arm == j && l.kind == 1).collect();
            let mut best: Option<(f64, &LaneEnd, &LaneEnd)> = None;
            for a in &walk_i {
                for b in &walk_j {
                    let d = (a.pos - b.pos).length();
                    if best.map(|x| d < x.0).unwrap_or(true) {
                        best = Some((d, a, b));
                    }
                }
            }
            if let Some((_, a, b)) = best {
                let pa = a.pos.truncate() - c2;
                let pb = b.pos.truncate() - c2;
                if let Some(s) = roads::straight(pa, pb) {
                    out.push(PathRec { seg: s, z0: a.pos.z - centre.z, z1: b.pos.z - centre.z, kind: 1, width: a.width.max(1.5), direction: 2, blinker: 0, light: None });
                }
            }
        }
    }
    out
}

/// One side of a road's cross-section beyond its kerb line (kerb, footway...): the
/// points in order outward, as (distance past the kerb line, height, u, v scale).
#[derive(Clone)]
struct SideProfile {
    texture: Option<PathBuf>,
    points: Vec<(f64, f64, f64, f64)>,
}

/// An arm as the junction's mesh needs it, seen from the junction looking out along it.
struct ArmGeo {
    end: DVec3,
    out: f64,
    /// Roadway edges (the kerb lines) to the right and left of the way out (m).
    right_w: f64,
    left_w: f64,
    #[allow(dead_code)]
    right: Vec<SideProfile>,
    left: Vec<SideProfile>,
    /// Texture phase where the arm meets the junction, and which way it runs into it.
    v0: f64,
    vdir: f64,
    /// The roadway's own texture and where its lane (no markings) lies: (texture, u of a
    /// lane's middle, the roadway's u range, metres per repeat along the road).
    road_tex: Option<(PathBuf, f64, f64, f64, f64, f64)>,
}

fn arm_geometry(doc: &Document, assets: &mut Assets, a: &Arm) -> Option<ArmGeo> {
    let s = doc.spline(a.key, a.id)?.clone();
    let model = assets.spline(&s.file)?;
    let c = SplineCurve::from_map(&s, tile_origin(a.key));
    let at = if a.at_end { s.length } else { 0.0 };
    let end = c.point_at(at);
    let out = roads::norm_deg(if a.at_end { c.heading_at(at) + 180.0 } else { c.heading_at(at) });
    let m = if s.mirror { -1.0 } else { 1.0 };
    let profs: Vec<(usize, Vec<(f64, f64, f64, f64)>)> = model.def.profiles.iter().filter(|p| p.points.len() >= 2).map(|p| (p.texture, p.points.iter().map(|q| (q.x as f64 * m, q.z as f64, q.u as f64, q.v_scale as f64)).collect())).collect();
    // the roadway: where the spline's street lanes run (its `[path]`s say it: a half of a
    // divided road has its centre line on the median's edge, with grass on one side); else
    // the profiles that reach the centre line; else the whole width
    let lanes: Vec<(f64, f64)> = model.def.paths.iter().filter(|q| q.kind == 0).map(|q| (q.start[0] as f64 * m, q.width as f64)).collect();
    let all_lo = profs.iter().flat_map(|p| p.1.iter()).map(|q| q.0).fold(0.0, f64::min);
    let all_hi = profs.iter().flat_map(|p| p.1.iter()).map(|q| q.0).fold(0.0, f64::max);
    let touches = |pts: &Vec<(f64, f64, f64, f64)>| pts.iter().map(|q| q.0).fold(f64::MAX, f64::min) <= 0.05 && pts.iter().map(|q| q.0).fold(f64::MIN, f64::max) >= -0.05;
    let span = |pts: &Vec<(f64, f64, f64, f64)>| (pts.iter().map(|q| q.0).fold(f64::MAX, f64::min), pts.iter().map(|q| q.0).fold(f64::MIN, f64::max));
    let (xl, xr) = if !lanes.is_empty() {
        let lo = lanes.iter().map(|(x, w)| x - w / 2.0).fold(f64::MAX, f64::min);
        let hi = lanes.iter().map(|(x, w)| x + w / 2.0).fold(f64::MIN, f64::max);
        // (out to the edges of the profiles the lanes run on: the asphalt often reaches
        // further than the lanes, to a gutter or a parking strip, and a kerb is where it ends)
        let under: Vec<(f64, f64)> = profs.iter().map(|p| span(&p.1)).filter(|(a, b)| *b > lo + 0.05 && *a < hi - 0.05).collect();
        (under.iter().map(|s| s.0).fold(lo, f64::min), under.iter().map(|s| s.1).fold(hi, f64::max))
    } else {
        let road: Vec<&(usize, Vec<(f64, f64, f64, f64)>)> = profs.iter().filter(|p| touches(&p.1)).collect();
        if road.is_empty() {
            (all_lo, all_hi)
        } else {
            (road.iter().flat_map(|p| p.1.iter()).map(|q| q.0).fold(0.0, f64::min), road.iter().flat_map(|p| p.1.iter()).map(|q| q.0).fold(0.0, f64::max))
        }
    };
    let inside = |pts: &Vec<(f64, f64, f64, f64)>| pts.iter().all(|q| q.0 >= xl - 0.05 && q.0 <= xr + 0.05);
    // (the lane on the right: its texture is the asphalt the junction is laid with, taken
    // from the middle of a lane, where no markings are)
    let lane_mid = lanes.iter().map(|(x, _)| *x).find(|x| *x > xl && *x < xr).unwrap_or((xl + xr) * 0.5);
    let central = profs.iter().filter(|p| inside(&p.1)).find(|p| p.1.iter().any(|q| q.0 <= lane_mid) && p.1.iter().any(|q| q.0 >= lane_mid)).or_else(|| profs.iter().find(|p| inside(&p.1)));
    let side = |right: bool| -> Vec<SideProfile> {
        profs
            .iter()
            .filter(|(_, pts)| if right { pts.iter().all(|q| q.0 >= xr - 0.05) } else { pts.iter().all(|q| q.0 <= xl + 0.05) })
            .map(|(t, pts)| {
                let mut points: Vec<(f64, f64, f64, f64)> = pts.iter().map(|q| ((if right { q.0 - xr } else { xl - q.0 }).max(0.0), q.1, q.2, q.3)).collect();
                if points.first().map(|p| p.0).unwrap_or(0.0) > points.last().map(|p| p.0).unwrap_or(0.0) {
                    points.reverse();
                }
                SideProfile { texture: model.materials.get(*t).and_then(|m| m.texture.clone()), points }
            })
            .collect()
    };
    let (spline_right, spline_left) = (side(true), side(false));
    // (looking out along an arm whose end is at the junction, the spline's right is our left)
    let (right, left, right_w, left_w) = if a.at_end { (spline_left, spline_right, -xl, xr) } else { (spline_right, spline_left, xr, -xl) };
    let vs = central.map(|c| c.1[0].3).unwrap_or(0.2).abs().max(1e-3);
    let v0 = vs * (at + s.tex_offset) * m;
    let vdir = if a.at_end { m } else { -m };
    let road_tex = central.and_then(|(t, pts)| {
        let tex = model.materials.get(*t)?.texture.clone()?;
        // the u in the middle of a lane
        let x = lane_mid;
        let mut u = pts[0].2;
        for w in pts.windows(2) {
            if (w[0].0 - x) * (w[1].0 - x) <= 0.0 && (w[1].0 - w[0].0).abs() > 1e-6 {
                u = w[0].2 + (w[1].2 - w[0].2) * (x - w[0].0) / (w[1].0 - w[0].0);
            }
        }
        let range = (pts.iter().map(|q| q.2).fold(f64::MIN, f64::max) - pts.iter().map(|q| q.2).fold(f64::MAX, f64::min)).abs();
        let width = (pts.iter().map(|q| q.0).fold(f64::MIN, f64::max) - pts.iter().map(|q| q.0).fold(f64::MAX, f64::min)).abs().max(0.5);
        // (a lane's width: the street lanes' own, else half the roadway)
        let lane = model.def.paths.iter().filter(|p| p.kind == 0).map(|p| p.width as f64).fold(0.0, f64::max);
        let lane = if lane > 1.5 { lane / 0.9 } else { width * 0.5 };
        Some((tex, u, range, 1.0 / vs, width, lane))
    });
    Some(ArmGeo { end, out, right_w, left_w, right, left, v0, vdir, road_tex })
}

/// The junction's mesh: vertices (relative to the centre: position, normal, uv), triangles
/// with their material, and the materials' textures.
#[derive(Default)]
struct Mesh {
    verts: Vec<([f64; 3], [f64; 3], [f64; 2])>,
    tris: Vec<([usize; 3], usize)>,
    textures: Vec<Option<PathBuf>>,
}

impl Mesh {
    fn material(&mut self, tex: Option<PathBuf>) -> usize {
        match self.textures.iter().position(|t| *t == tex) {
            Some(i) => i,
            None => {
                self.textures.push(tex);
                self.textures.len() - 1
            }
        }
    }

    /// A triangle facing `n` (the vertices are turned round if they face the other way).
    fn tri(&mut self, p: [DVec3; 3], uv: [[f64; 2]; 3], n: DVec3, mat: usize) {
        let face = (p[1] - p[0]).cross(p[2] - p[0]);
        if face.length() < 1e-9 {
            return;
        }
        let order = if face.dot(n) >= 0.0 { [0, 1, 2] } else { [0, 2, 1] };
        let base = self.verts.len();
        let n = n.normalize_or_zero();
        for i in order {
            self.verts.push((p[i].to_array(), n.to_array(), uv[i]));
        }
        self.tris.push(([base, base + 1, base + 2], mat));
    }
}

/// Build the mesh: the roadway in the middle, and round each corner the kerb and footway
/// of the roads, swept along a smooth curve from one arm's kerb line to the next's.
/// `along`: the heading the roadway's texture runs (the widest road's), so its slabs and
/// patches lie as the road's do, not slanted across it.
fn junction_mesh(arms: &[ArmGeo], centre: DVec3, core_tex: Option<PathBuf>, period: f64, along: f64) -> Mesh {
    let mut mesh = Mesh::default();
    let core = mesh.material(core_tex);
    let rel = |p: DVec3| p - centre;
    // the arms round the junction, counter-clockwise
    let mut order: Vec<usize> = (0..arms.len()).collect();
    let ang = |i: usize| { let d = roads::dir(arms[i].out); d.y.atan2(d.x) };
    order.sort_by(|a, b| ang(*a).total_cmp(&ang(*b)));
    let mut outline: Vec<DVec3> = Vec::new();
    for k in 0..order.len() {
        let a = &arms[order[k]];
        let b = &arms[order[(k + 1) % order.len()]];
        let r_pt = a.end.truncate() + roads::right(a.out) * a.right_w;
        let l_pt = a.end.truncate() + roads::right(a.out) * -a.left_w;
        outline.push(DVec3::new(r_pt.x, r_pt.y, a.end.z));
        outline.push(DVec3::new(l_pt.x, l_pt.y, a.end.z));
        // the kerb from this arm's left kerb line round to the next arm's right one
        let from = l_pt;
        let to = b.end.truncate() + roads::right(b.out) * b.right_w;
        let segs = kerb_round(from, a.out, to, b.out);
        let total: f64 = segs.iter().map(|s| s.length).sum();
        if total < 0.05 {
            continue;
        }
        let n = (total / 0.75).ceil().max(4.0) as usize;
        // stations along the kerb: (point, heading, distance)
        let mut st: Vec<(DVec2, f64, f64)> = Vec::new();
        for i in 0..=n {
            let mut d = total * i as f64 / n as f64;
            let mut seg = &segs[0];
            for sgm in &segs {
                seg = sgm;
                if d <= sgm.length + 1e-9 {
                    break;
                }
                d -= sgm.length;
            }
            let d = d.min(seg.length);
            st.push((seg.point(d), seg.heading_at(d), total * i as f64 / n as f64));
        }
        for p in st.iter().skip(1).take(n.saturating_sub(1)) {
            let t = p.2 / total;
            outline.push(DVec3::new(p.0.x, p.0.y, a.end.z + (b.end.z - a.end.z) * t));
        }
        // the corner's kerb and footway: this arm's left side or the next arm's right one,
        // whichever reaches further out (the narrower one then runs into it, as a footway
        // meets a verge), swept along the curve (it lies to the right of the way round)
        let reach = |v: &[SideProfile]| v.iter().flat_map(|p| p.points.iter()).map(|q| q.0).fold(0.0, f64::max);
        let (sides, v0, vdir, back) = if reach(&b.right) > reach(&a.left) + 0.5 { (&b.right, b.v0, b.vdir, true) } else { (&a.left, a.v0, a.vdir, false) };
        for prof in sides.iter() {
            let mat = mesh.material(prof.texture.clone());
            for w in prof.points.windows(2) {
                let (d0, z0, u0, vs0) = w[0];
                let (d1, z1, u1, vs1) = w[1];
                // the face's normal in the cross-section: up for a flat part, towards the road
                // for the face of a kerb
                let (dd, dz) = (d1 - d0, z1 - z0);
                for q in st.windows(2) {
                    let (p0, h0, s0) = q[0];
                    let (p1, h1, s1) = q[1];
                    let base = |p: DVec2, h: f64, s: f64, d: f64, z: f64| {
                        let xy = p + roads::right(h) * d;
                        let zz = a.end.z + (b.end.z - a.end.z) * (s / total) + z;
                        rel(DVec3::new(xy.x, xy.y, zz))
                    };
                    // (the texture runs on from the arm it is taken from)
                    let v = |s: f64, vs: f64| if back { v0 - vdir * vs * (total - s) } else { v0 + vdir * vs * s };
                    let pa = base(p0, h0, s0, d0, z0);
                    let pb = base(p0, h0, s0, d1, z1);
                    let pc = base(p1, h1, s1, d0, z0);
                    let pd = base(p1, h1, s1, d1, z1);
                    let r = roads::right((h0 + h1) * 0.5 + if (h1 - h0).abs() > 180.0 { 180.0 } else { 0.0 });
                    // (in the cross-section, the left of the way outward: up, or towards the road)
                    let nrm = DVec3::new(-r.x * dz, -r.y * dz, dd);
                    let nrm = if nrm.length() < 1e-9 { DVec3::Z } else { nrm };
                    let (ua, ub) = ([u0, v(s0, vs0)], [u1, v(s0, vs1)]);
                    let (uc, ud) = ([u0, v(s1, vs0)], [u1, v(s1, vs1)]);
                    mesh.tri([pa, pb, pc], [ua, ub, uc], nrm, mat);
                    mesh.tri([pb, pd, pc], [ub, ud, uc], nrm, mat);
                }
            }
        }
    }
    log::debug!("junction outline: {:?}", outline.iter().map(|p| { let q = *p - centre; (q.x.round() as i64, q.y.round() as i64) }).collect::<Vec<_>>());
    for (i, g) in order.iter().map(|i| &arms[*i]).enumerate() {
        log::debug!("arm {i}: end ({:.1}, {:.1}) out {:.0} right {:.1} left {:.1}", g.end.x - centre.x, g.end.y - centre.y, g.out, g.right_w, g.left_w);
    }
    // the roadway: a fan round the centre, its texture laid flat on the map
    let mid = DVec3::new(0.0, 0.0, outline.iter().map(|p| p.z - centre.z).sum::<f64>() / outline.len().max(1) as f64);
    let (fwd, side) = (roads::dir(along), roads::right(along));
    let uv = |p: DVec3| [(p.x * side.x + p.y * side.y) / period, -(p.x * fwd.x + p.y * fwd.y) / period];
    for i in 0..outline.len() {
        let p0 = rel(outline[i]);
        let p1 = rel(outline[(i + 1) % outline.len()]);
        mesh.tri([mid, p0, p1], [uv(mid), uv(p0), uv(p1)], DVec3::Z, core);
    }
    mesh
}

/// The kerb round a corner: from `a` (on a kerb line running out along `out_a`) to `b` (on
/// one running out along `out_b`). The two lines are carried on into the junction to where
/// they meet, and a circle touching both rounds the corner there, as large as the junction
/// has room for (a fillet); kerb lines in one line, or meeting outside, are joined straight.
fn kerb_round(a: DVec2, out_a: f64, b: DVec2, out_b: f64) -> Vec<Seg> {
    let (da, db) = (roads::dir(out_a), roads::dir(out_b));
    let straight = || roads::straight(a, b).map(|s| vec![s]).unwrap_or_default();
    // (the outside of a bend: the kerb lines part instead of meeting, and a smooth curve
    // carries the old edge round - a straight cut there left the ground showing)
    let smooth = || roads::biarc(a, roads::norm_deg(out_a + 180.0), b, out_b).unwrap_or_else(straight);
    let denom = da.perp_dot(db);
    if denom.abs() < 0.05 {
        // in one line (the straight side of a T): straight across; side by side: round
        return if da.dot(db) < 0.0 { straight() } else { smooth() };
    }
    // a + da * u = b + db * v
    let u = (b - a).perp_dot(db) / denom;
    let v = (b - a).perp_dot(da) / denom;
    if u > 0.01 || v > 0.01 {
        return smooth();
    }
    let x = a + da * u;
    let t = (-u).min(-v);
    let (ta, tb) = (x + da * t, x + db * t);
    let mut segs = Vec::new();
    if (ta - a).length() > 0.02 {
        segs.push(Seg { start: a, heading: roads::norm_deg(out_a + 180.0), length: (ta - a).length(), radius: 0.0 });
    }
    // the circle touching both lines at ta and tb (equally far from where they meet)
    let side = (tb - ta).dot(roads::right(out_a + 180.0));
    let chord = (tb - ta).length();
    let fwd = (tb - ta).dot(roads::dir(out_a + 180.0));
    if chord > 0.02 && side.abs() > 1e-6 {
        let r = (fwd * fwd + side * side) / (2.0 * side);
        let sweep = 2.0 * side.atan2(fwd);
        segs.push(Seg { start: ta, heading: roads::norm_deg(out_a + 180.0), length: (r * sweep).abs(), radius: r });
    }
    if (b - tb).length() > 0.02 {
        segs.push(Seg { start: tb, heading: roads::norm_deg(out_b), length: (b - tb).length(), radius: 0.0 });
    }
    if segs.is_empty() { straight() } else { segs }
}

/// The mesh as a DirectX `.x` text file in OMSI's frame (x right, y up, z forward).
/// (Seen from the side a triangle faces, OMSI's front faces turn clockwise.)
fn mesh_x(m: &Mesh, names: &[String]) -> String {
    let mut x = String::from("xof 0302txt 0032\n\nMesh {\n");
    let nv = m.verts.len();
    let _ = writeln!(x, " {nv};");
    for (i, (p, _, _)) in m.verts.iter().enumerate() {
        let _ = writeln!(x, " {:.4};{:.4};{:.4};{}", p[0], p[2], p[1], if i + 1 == nv { ";" } else { "," });
    }
    let nf = m.tris.len();
    let _ = writeln!(x, " {nf};");
    for (i, (f, _)) in m.tris.iter().enumerate() {
        let _ = writeln!(x, " 3;{},{},{};{}", f[0], f[2], f[1], if i + 1 == nf { ";" } else { "," });
    }
    let _ = writeln!(x, " MeshNormals {{\n  {nv};");
    for (i, (_, n, _)) in m.verts.iter().enumerate() {
        let _ = writeln!(x, "  {:.4};{:.4};{:.4};{}", n[0], n[2], n[1], if i + 1 == nv { ";" } else { "," });
    }
    let _ = writeln!(x, "  {nf};");
    for (i, (f, _)) in m.tris.iter().enumerate() {
        let _ = writeln!(x, "  3;{},{},{};{}", f[0], f[2], f[1], if i + 1 == nf { ";" } else { "," });
    }
    x.push_str(" }\n");
    let _ = writeln!(x, " MeshTextureCoords {{\n  {nv};");
    for (i, (_, _, t)) in m.verts.iter().enumerate() {
        let _ = writeln!(x, "  {:.5};{:.5};{}", t[0], t[1], if i + 1 == nv { ";" } else { "," });
    }
    x.push_str(" }\n");
    let _ = writeln!(x, " MeshMaterialList {{\n  {};\n  {nf};", names.len().max(1));
    for (i, (_, mat)) in m.tris.iter().enumerate() {
        let _ = writeln!(x, "  {}{}", mat, if i + 1 == nf { ";" } else { "," });
    }
    for n in names {
        let _ = writeln!(x, "  Material {{\n   1.0;1.0;1.0;1.0;;\n   10.0;\n   0.0;0.0;0.0;;\n   0.0;0.0;0.0;;");
        if !n.is_empty() {
            let _ = writeln!(x, "   TextureFilename {{\n    \"{n}\";\n   }}");
        }
        x.push_str("  }\n");
    }
    x.push_str(" }\n}\n");
    x
}

/// A plain square of a road's own surface, `period` metres a side (what the texture spans
/// along the road): `band_m` metres of a lane's middle (no markings), as wide as it is on
/// the road, mirrored to and fro across so it tiles without a seam, and the texture's full
/// length along. The junction's roadway, laid flat at the same scale, then has the roads'
/// grain and colour, not a stretched smear.
fn clean_asphalt(tex: &Path, u: f64, u_per_m: f64, period: f64, lane_m: f64, out: &Path) -> Option<()> {
    let img = omsi_texture::decode_file(tex).ok()?;
    let (w, h) = (img.width as usize, img.height as usize);
    let col = |uu: f64| -> f64 {
        let sx = ((uu.rem_euclid(1.0) * w as f64) as usize).min(w - 1);
        (0..h).step_by((h / 64).max(1)).map(|y| { let p = &img.rgba[(y * w + sx) * 4..][..3]; (p[0] as f64 + p[1] as f64 + p[2] as f64) / 765.0 }).sum::<f64>() / (h as f64 / (h / 64).max(1) as f64)
    };
    // painted lines at the lane's edges (brighter than its middle) stay out: a band of its
    // middle, mirrored to and fro; joints or nothing there: the whole lane, repeated, so
    // slabs keep their grid
    let half_u = lane_m * 0.5 * u_per_m;
    let mid = col(u);
    let edge = (0..=6).map(|i| col(u - half_u + half_u * 0.08 * i as f64)).chain((0..=6).map(|i| col(u + half_u - half_u * 0.08 * i as f64))).fold(0.0, f64::max);
    let painted = edge > mid + 0.12;
    let (band_m, mirror) = if painted { ((lane_m * 0.5).clamp(0.8, 2.0), true) } else { (lane_m.clamp(1.0, 5.0), false) };
    let band_u = (band_m * u_per_m).max(0.002);
    let size = 512usize;
    let mut buf = image::RgbaImage::new(size as u32, size as u32);
    // (each band side by side taken from another stretch of the road, so its patches and
    // slabs do not stand in rows of the same one)
    let bands = (period / band_m).round().max(1.0) as usize;
    let band_m = period / bands as f64;
    for y in 0..size {
        for x in 0..size {
            let m = x as f64 / size as f64 * period;
            let j = ((m / band_m) as usize).min(bands - 1);
            let shift = if j == 0 { 0.0 } else { (j as f64 * 0.618_034).fract() };
            let sy = (((y as f64 / size as f64 + shift).fract() * h as f64) as usize).min(h - 1);
            let t = if mirror {
                let f = (m / band_m).rem_euclid(2.0);
                if f < 1.0 { f } else { 2.0 - f }
            } else {
                (m / band_m).rem_euclid(1.0)
            };
            let uu = (u - band_u * 0.5 + band_u * t).rem_euclid(1.0);
            let sx = ((uu * w as f64) as usize).min(w - 1);
            let p = &img.rgba[(sy * w + sx) * 4..][..4];
            buf.put_pixel(x as u32, y as u32, image::Rgba([p[0], p[1], p[2], 255]));
        }
    }
    buf.save(out).ok()
}

fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

/// The same with lights and crossings as `opts` says.
pub fn make_with(doc: &mut Document, assets: &mut Assets, content: &Path, asphalt: Option<&Path>, centre: DVec3, arms: &[Arm], opts: &Options) -> anyhow::Result<i64> {
    let (_, lanes) = arm_ends(doc, assets, arms).ok_or_else(|| anyhow::anyhow!("a road of the junction is not loaded"))?;
    let geo: Vec<ArmGeo> = arms.iter().map(|a| arm_geometry(doc, assets, a)).collect::<Option<_>>().ok_or_else(|| anyhow::anyhow!("a road of the junction is not loaded"))?;
    let outs: Vec<f64> = geo.iter().map(|g| g.out).collect();
    let light_of: Vec<Option<usize>> = if opts.lights && arms.len() >= 3 {
        let (a, b) = main_road(&outs);
        (0..arms.len()).map(|i| Some(if i == a || i == b { 0 } else { 1 })).collect()
    } else {
        vec![None; arms.len()]
    };
    let mut recs = paths(&lanes, centre, &light_of);
    // zebra crossings: a footway across each arm chosen, a step inside its end
    let mut zebras: Vec<(DVec2, f64, f64, f64)> = Vec::new();
    for g in geo.iter().filter(|g| opts.crossing_at(g.out)) {
        let r = roads::right(g.out);
        let mid = g.end.truncate() - roads::dir(g.out) * 2.0 - centre.truncate();
        let (a, b) = (mid - r * (g.left_w + 0.5), mid + r * (g.right_w + 0.5));
        if let Some(seg) = roads::straight(a, b) {
            let z = g.end.z - centre.z;
            recs.push(PathRec { seg, z0: z, z1: z, kind: 1, width: 3.0, direction: 2, blinker: 0, light: None });
            zebras.push((mid, g.out, g.left_w, g.right_w));
        }
    }
    let id = doc.new_id();
    let folder: String = doc.dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "map".into()).chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let sco_root = std::fs::read_dir(content)?.flatten().map(|e| e.path()).find(|p| p.is_dir() && p.file_name().map(|f| f.to_string_lossy().eq_ignore_ascii_case("sceneryobjects")).unwrap_or(false)).unwrap_or_else(|| content.join("Sceneryobjects"));
    let dir = sco_root.join("openMapper").join(&folder);
    let tex_dir = dir.join("texture");
    std::fs::create_dir_all(dir.join("model"))?;
    std::fs::create_dir_all(&tex_dir)?;
    // the roadway's texture: the installation's own junction asphalt (what OMSI 2's crossings
    // are made of: a road's texture repeated on a plate brought its patches along in rows),
    // else one made from the roads' asphalt
    let widest = geo.iter().filter(|g| g.road_tex.is_some()).max_by(|a, b| (a.left_w + a.right_w).total_cmp(&(b.left_w + b.right_w)));
    let road_tex = if asphalt.is_some() { None } else { widest.and_then(|g| g.road_tex.clone()) };
    let (core_tex, period) = match road_tex {
        Some((tex, u, range, period, width, lane)) => {
            let stem = tex.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "road".into());
            let out = tex_dir.join(format!("om_{stem}_plain5.png"));
            // (one cut short, a full disk say, is made again)
            if out.exists() && image::open(&out).is_err() {
                let _ = std::fs::remove_file(&out);
            }
            if !out.exists() {
                // (most of a lane: slabs and patches keep their size; the markings at its
                // edges stay out)
                clean_asphalt(&tex, u, range / width, period.clamp(2.0, 40.0), lane, &out);
            }
            (out.exists().then_some(out).or_else(|| asphalt.map(Path::to_path_buf)), period.clamp(2.0, 40.0))
        }
        None => (asphalt.map(Path::to_path_buf), 8.0),
    };
    let along = widest.map(|g| g.out).unwrap_or(0.0);
    let mut mesh = junction_mesh(&geo, centre, core_tex, period, along);
    if !zebras.is_empty() {
        let white = tex_dir.join("om_zebra_white.png");
        if !white.exists() {
            let _ = image::RgbaImage::from_pixel(8, 8, image::Rgba([235, 235, 230, 255])).save(&white);
        }
        let mat = mesh.material(Some(white));
        let z = geo.iter().map(|g| g.end.z - centre.z).sum::<f64>() / geo.len().max(1) as f64 + 0.03;
        for (mid, out, lw, rw) in &zebras {
            let (d, r) = (roads::dir(*out), roads::right(*out));
            // stripes 0.5 m wide, a metre apart, 3 m along the road
            let mut x = -lw + 0.25;
            while x < rw - 0.25 {
                let c = *mid + r * x;
                let q = [c - r * 0.25 - d * 1.5, c + r * 0.25 - d * 1.5, c + r * 0.25 + d * 1.5, c - r * 0.25 + d * 1.5].map(|p| DVec3::new(p.x, p.y, z));
                let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
                mesh.tri([q[0], q[1], q[2]], uv, DVec3::Z, mat);
                mesh.tri([q[0], q[2], q[3]], [[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]], DVec3::Z, mat);
                x += 1.0;
            }
        }
    }
    // every texture beside the object
    let mut names = Vec::new();
    for t in &mesh.textures {
        let Some(p) = t else {
            names.push(String::new());
            continue;
        };
        let name = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let to = tex_dir.join(&name);
        if !to.exists() {
            let _ = std::fs::copy(p, &to);
        }
        names.push(name);
    }
    let name = format!("junction_{id}");
    std::fs::write(dir.join("model").join(format!("{name}.x")), mesh_x(&mesh, &names))?;
    let mut sco = String::new();
    let _ = write!(sco, "{}\n\n[friendlyname]\nJunction {id}\n\n[groups]\n1\nopenMapper\n\n[surface]\n\n[absheight]\n\n[mesh]\n{name}.x\n\n", opts.line());
    if light_of.iter().any(|l| l.is_some()) {
        sco.push_str(&light_program(opts));
    }
    // where roads join it (the editors snap to these)
    for (g, a) in geo.iter().zip(arms) {
        let file = doc.spline(a.key, a.id).map(|s| s.file.clone()).unwrap_or_default();
        let p = g.end - centre;
        // (OMSI's order: x, height, forward)
        let _ = write!(sco, "[splinehelper]\n{file}\n{}\n{}\n{}\n{}\n0.1\n0\n\n", num(p.x), num(p.z), num(p.y), num(roads::norm_deg(g.out + 180.0)));
    }
    for p in &recs {
        let s = &p.seg;
        let grad = if s.length > 0.01 { (p.z1 - p.z0) / s.length } else { 0.0 };
        let _ = write!(sco, "[path]\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n\n", num(s.start.x), num(s.start.y), num(p.z0), num(s.heading), num(s.radius), num(s.length), num(grad), num(grad), p.kind, num(p.width), p.direction, p.blinker);
        if let Some(l) = p.light {
            let _ = write!(sco, "[use_traffic_light]\n{l}\n\n");
        }
    }
    std::fs::write(dir.join(format!("{name}.sco")), omsi_cfg::encode_text(&sco.replace('\n', "\r\n")))?;
    crate::hot::own_change();
    let k = tile_of(centre.truncate());
    let k = if doc.tiles.contains_key(&k) { k } else { arms[0].key };
    let org = tile_origin(k);
    let rel = format!("Sceneryobjects\\openMapper\\{folder}\\{name}.sco");
    let t = doc.tiles.get_mut(&k).ok_or_else(|| anyhow::anyhow!("no tile"))?;
    t.tile.objects.push(MapObject { file: rel, id, pos: [centre.x - org.x, centre.y - org.y, centre.z], rot: [0.0; 3], ..Default::default() });
    t.touch_content();
    // the signal heads, on poles, bound to the junction's lights by [varparent]
    let left_hand = doc.global.left_hand_traffic;
    for (i, g) in geo.iter().enumerate() {
        let Some(light) = light_of[i] else { continue };
        if !lanes.iter().any(|l| l.arm == i && l.kind == 0 && l.incoming) {
            continue;
        }
        // (traffic coming in keeps to the left of the way out where it keeps right)
        let side = if left_hand { roads::right(g.out) * (g.right_w + 1.2) } else { -roads::right(g.out) * (g.left_w + 1.2) };
        let p = g.end.truncate() + side - roads::dir(g.out) * 1.0;
        let hk = tile_of(p);
        if !doc.tiles.contains_key(&hk) {
            continue;
        }
        let org = tile_origin(hk);
        for (file, z, extra) in [(LIGHT_POLE, 0.0, Vec::new()), (LIGHT_HEAD, 2.55, vec![light.to_string()])] {
            let oid = doc.new_id();
            let t = doc.tiles.get_mut(&hk).unwrap();
            let flag = extra.len() as i32;
            t.tile.objects.push(MapObject { file: file.to_string(), id: oid, pos: [p.x - org.x, p.y - org.y, z], rot: [g.out, 0.0, 0.0], flag, extra, var_parent: Some(id), ..Default::default() });
            t.touch_content();
        }
    }
    Ok(id)
}

/// Delete the junction files (`junction_<id>.sco` and its `.x`) in `dir` whose object file
/// (`Sceneryobjects\\openMapper\\<folder>\\junction_<id>.sco`, lower case) is not in `used`,
/// leaving those numbered from `keep_from` on (made in this session: undo may need them).
pub fn remove_unused(dir: &Path, folder: &str, used: &std::collections::HashSet<String>, keep_from: i64) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        if !(lower.starts_with("junction_") && lower.ends_with(".sco")) {
            continue;
        }
        let rel = format!("sceneryobjects\\openmapper\\{}\\{lower}", folder.to_lowercase());
        let num: Option<i64> = lower["junction_".len()..lower.len() - 4].parse().ok();
        if used.contains(&rel) || num.map(|n| n >= keep_from).unwrap_or(true) {
            continue;
        }
        let stem = &name[..name.len() - 4];
        let _ = std::fs::remove_file(e.path());
        let _ = std::fs::remove_file(dir.join("model").join(format!("{stem}.x")));
        n += 1;
    }
    if n > 0 {
        crate::hot::own_change();
    }
    n
}

/// The objects that belong to junction `id` (its signal heads and poles).
pub fn children(doc: &Document, id: i64) -> Vec<(TileKey, i64)> {
    doc.tiles.iter().flat_map(|(k, t)| t.tile.objects.iter().filter(|o| o.var_parent == Some(id)).map(move |o| (*k, o.id))).collect()
}

/// Where two polylines cross: (point, distance along a, distance along b).
pub fn crossings(a: &[(DVec2, f64)], b: &[(DVec2, f64)]) -> Vec<(DVec2, f64, f64)> {
    let mut out = Vec::new();
    for wa in a.windows(2) {
        for wb in b.windows(2) {
            let (p, r) = (wa[0].0, wa[1].0 - wa[0].0);
            let (q, s) = (wb[0].0, wb[1].0 - wb[0].0);
            let rxs = r.perp_dot(s);
            if rxs.abs() < 1e-9 {
                continue;
            }
            let t = (q - p).perp_dot(s) / rxs;
            let u = (q - p).perp_dot(r) / rxs;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                let pt = p + r * t;
                // (a crossing right on a joint of the lines is found on both sides of it)
                if !out.iter().any(|o: &(DVec2, f64, f64)| (o.0 - pt).length() < 0.01) {
                    out.push((pt, wa[0].1 + (wa[1].1 - wa[0].1) * t, wb[0].1 + (wb[1].1 - wb[0].1) * u));
                }
            }
        }
    }
    out
}

/// A spline's centre line every 2 m: (point, distance along it).
pub fn polyline(c: &SplineCurve) -> Vec<(DVec2, f64)> {
    let n = (c.length / 2.0).ceil().max(1.0) as usize;
    (0..=n).map(|i| {
        let s = c.length * i as f64 / n as f64;
        (c.point_at(s).truncate(), s)
    }).collect()
}

/// The asphalt texture junction plates are drawn with: the installation's own.
pub fn asphalt_texture(omsi: &Path) -> Option<PathBuf> {
    let dir = std::fs::read_dir(omsi).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().map(|f| f.to_string_lossy().eq_ignore_ascii_case("texture")).unwrap_or(false))?;
    for name in ["str_asphdrk.bmp", "str_asphalt.bmp", "asphalt.bmp"] {
        if let Some(p) = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().map(|f| f.to_string_lossy().eq_ignore_ascii_case(name)).unwrap_or(false)) {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn options_are_read_back_from_the_line_written() {
        let o = Options { lights: true, main_green: 42.0, side_green: 17.0, crossings: vec![90.0, 270.5] };
        let text = format!("{}\r\n\r\n[friendlyname]\r\nJunction 1\r\n", o.line());
        let back = Options::parse(&text);
        assert_eq!(back.lights, true);
        assert_eq!((back.main_green, back.side_green), (42.0, 17.0));
        assert!(back.crossing_at(91.0) && back.crossing_at(270.0) && !back.crossing_at(0.0));
        assert_eq!(Options::parse("[friendlyname]\nx"), Options::default());
    }

    #[test]
    fn the_main_road_is_the_pair_most_in_line_and_its_light_comes_first() {
        assert_eq!(main_road(&[0.5, 90.0, 270.0]), (1, 2));
        let p = light_program(&Options { lights: true, main_green: 30.0, side_green: 20.0, crossings: vec![] });
        // a cycle of both greens, their yellows, red-yellows and all-red times
        assert!(p.starts_with("[traffic_lights_group]\n64\n"), "{p}");
        assert_eq!(p.matches("[traffic_light]").count(), 2);
    }

    #[test]
    fn only_junction_files_nothing_uses_from_before_are_removed() {
        let dir = std::env::temp_dir().join(format!("om-gc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("model")).unwrap();
        for n in [5, 6, 50] {
            std::fs::write(dir.join(format!("junction_{n}.sco")), "x").unwrap();
            std::fs::write(dir.join("model").join(format!("junction_{n}.x")), "x").unwrap();
        }
        std::fs::write(dir.join("other.sco"), "x").unwrap();
        let used: std::collections::HashSet<String> = ["sceneryobjects\\openmapper\\m\\junction_6.sco".to_string()].into();
        // 5 unused and old: removed; 6 used; 50 made this session (from 40 on)
        assert_eq!(remove_unused(&dir, "m", &used, 40), 1);
        assert!(!dir.join("junction_5.sco").exists() && !dir.join("model/junction_5.x").exists());
        assert!(dir.join("junction_6.sco").exists() && dir.join("junction_50.sco").exists() && dir.join("other.sco").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;

    #[test]
    fn two_roads_crossing_meet_once() {
        let a: Vec<(DVec2, f64)> = (0..=10).map(|i| (DVec2::new(i as f64 * 10.0 - 50.0, 0.0), i as f64 * 10.0)).collect();
        let b: Vec<(DVec2, f64)> = (0..=10).map(|i| (DVec2::new(5.0, i as f64 * 10.0 - 50.0), i as f64 * 10.0)).collect();
        let x = crossings(&a, &b);
        assert_eq!(x.len(), 1);
        assert!((x[0].0 - DVec2::new(5.0, 0.0)).length() < 1e-9);
        assert!((x[0].1 - 55.0).abs() < 1e-9 && (x[0].2 - 50.0).abs() < 1e-9);
    }

    #[test]
    fn a_kerb_rounds_a_corner_smoothly() {
        // an arm running out east with its left kerb 4 m north of the centre line, ending
        // 12 m out; the next arm running out north with its right kerb 4 m east
        let a = DVec2::new(12.0, 4.0);
        let b = DVec2::new(4.0, 12.0);
        let segs = kerb_round(a, 90.0, b, 0.0);
        assert!(!segs.is_empty(), "{segs:?}");
        let end = segs.last().unwrap();
        assert!((end.end() - b).length() < 1e-6, "ends at {:?}", end.end());
        for w in segs.windows(2) {
            assert!((w[0].end() - w[1].start).length() < 1e-6);
            assert!((roads::norm_deg(w[0].end_heading() - w[1].heading + 180.0) - 180.0).abs() < 1e-6);
        }
        // it rounds the corner through its inside
        let mid = segs[segs.len() / 2].point(segs[segs.len() / 2].length * 0.5);
        assert!(mid.x > 4.0 && mid.y > 4.0 && mid.x < 12.0 && mid.y < 12.0, "{mid:?}");
    }

    #[test]
    fn a_junction_mesh_reads_back_through_openomsi() {
        let arm = |end: DVec3, out: f64| ArmGeo {
            end,
            out,
            right_w: 3.5,
            left_w: 3.5,
            right: vec![SideProfile { texture: None, points: vec![(0.0, 0.0, 0.0, 0.2), (0.0, 0.15, 0.1, 0.2), (2.0, 0.15, 0.5, 0.2)] }],
            left: vec![SideProfile { texture: None, points: vec![(0.0, 0.0, 0.0, 0.2), (0.0, 0.15, 0.1, 0.2), (2.0, 0.15, 0.5, 0.2)] }],
            v0: 0.0,
            vdir: 1.0,
            road_tex: None,
        };
        let arms = vec![arm(DVec3::new(10.0, 0.0, 0.0), 90.0), arm(DVec3::new(-10.0, 0.0, 0.0), 270.0), arm(DVec3::new(0.0, 10.0, 0.0), 0.0)];
        let mesh = junction_mesh(&arms, DVec3::ZERO, None, 8.0, 0.0);
        assert!(mesh.tris.len() > 20);
        // every roadway triangle faces up
        for (f, mat) in &mesh.tris {
            if *mat == 0 {
                let p: Vec<DVec3> = f.iter().map(|i| DVec3::from(mesh.verts[*i].0)).collect();
                assert!((p[1] - p[0]).cross(p[2] - p[0]).z >= 0.0);
            }
        }
        let x = mesh_x(&mesh, &[String::new()]);
        let dir = std::env::temp_dir().join(format!("om-junction-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("j.x"), x).unwrap();
        let m = omsi_o3d::load_mesh(&dir.join("j.x")).unwrap();
        assert_eq!(m.triangles.len(), mesh.tris.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! UV maps: unwrapping a mesh flat onto its picture (by seams, by angle, by projection),
//! packing the pieces into the square, and moving them about.

use std::collections::{HashMap, HashSet};
use std::f32::consts::{PI, TAU};

use glam::{Affine2, DVec2, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::{edge_key, Axis, Face, Mesh};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Unwrap {
    /// Least-squares conformal unwrap of the islands the marked seams cut out, packed into
    /// the square.
    Conformal,
    /// Faces gathered into islands where they meet at less than `angle_deg`, each laid flat
    /// and packed into the square.
    Smart { angle_deg: f32 },
    /// Each face from the side of a box it faces most, the six sides laid out apart in the
    /// picture (true to one another's sizes).
    Cube { size: f32 },
    /// Round `axis`, the picture once round; height in metres / `size`.
    Cylinder { axis: Axis, size: f32 },
    /// Longitude and latitude about the faces' middle.
    Sphere,
    /// Straight along `direction` (from the view, from above...), `size` metres to the picture.
    Planar { direction: Vec3, size: f32 },
    /// Every face its own island, packed with a margin: for light maps and baking.
    LightmapPack,
}

/// The angle Conformal splits an island at when its seams leave it closed or ring-shaped.
const FALLBACK_ANGLE: f32 = 66.0;

/// Give the faces `faces` (all if empty) new UVs. `margin`: space between packed islands, in
/// the square's units.
pub fn unwrap(mesh: &mut Mesh, faces: &[usize], how: Unwrap, margin: f32) {
    let list = chosen(mesh, faces);
    if list.is_empty() {
        return;
    }
    for &f in &list {
        let n = mesh.faces[f].verts.len();
        mesh.faces[f].uvs.resize(n, Vec2::ZERO);
    }
    match how {
        Unwrap::Conformal => {
            let seams: HashSet<(u32, u32)> = mesh.seams.iter().map(|&(a, b)| edge_key(a, b)).collect();
            let mut pieces = Vec::new();
            for island in seam_islands(mesh, &list, &seams) {
                let sheet = Sheet::new(mesh, &island, &seams);
                if sheet.is_disk() {
                    sheet.flatten(mesh);
                    pieces.push(island);
                    continue;
                }
                // closed or ring-shaped: it cannot lie flat in one piece without overlapping
                // or stretching badly, so cut it where it bends
                for chart in angle_charts(mesh, &island, FALLBACK_ANGLE) {
                    let sheet = Sheet::new(mesh, &chart, &seams);
                    if sheet.is_disk() {
                        sheet.flatten(mesh);
                    } else {
                        project_chart(mesh, &chart);
                    }
                    pieces.push(chart);
                }
            }
            lay_out(mesh, &pieces, margin);
        }
        Unwrap::Smart { angle_deg } => {
            let charts = angle_charts(mesh, &list, angle_deg);
            for c in &charts {
                project_chart(mesh, c);
            }
            lay_out(mesh, &charts, margin);
        }
        Unwrap::Cube { size } => {
            let k = 1.0 / positive(size);
            let mut sides: [Vec<usize>; 6] = Default::default();
            for &f in &list {
                let n = mesh.face_normal(f);
                let a = n.abs();
                let dir = if n == Vec3::ZERO {
                    Vec3::NEG_Z
                } else if a.x >= a.y && a.x >= a.z {
                    Vec3::new(-n.x.signum(), 0.0, 0.0)
                } else if a.y >= a.z {
                    Vec3::new(0.0, -n.y.signum(), 0.0)
                } else {
                    Vec3::new(0.0, 0.0, -n.z.signum())
                };
                project_face(mesh, f, dir, k);
                let side = [dir.x != 0.0, dir.y != 0.0, dir.z != 0.0].iter().position(|&b| b).unwrap_or(2) * 2 + (dir.x + dir.y + dir.z > 0.0) as usize;
                sides[side].push(f);
            }
            // (each side of the box on a part of the picture of its own, not all in one place)
            let sides: Vec<Vec<usize>> = sides.into_iter().filter(|s| !s.is_empty()).collect();
            lay_out(mesh, &sides, margin);
        }
        Unwrap::Cylinder { axis, size } => cylinder(mesh, &list, axis, positive(size)),
        Unwrap::Sphere => sphere(mesh, &list),
        Unwrap::Planar { direction, size } => {
            let k = 1.0 / positive(size);
            for &f in &list {
                project_face(mesh, f, direction, k);
            }
        }
        Unwrap::LightmapPack => {
            let pieces: Vec<Vec<usize>> = list.iter().map(|&f| vec![f]).collect();
            for p in &pieces {
                project_chart(mesh, p);
            }
            lay_out(mesh, &pieces, margin);
        }
    }
}

/// The islands among `faces` (all if empty): faces joined by an edge whose two corners have
/// the same UVs on both faces.
pub fn islands(mesh: &Mesh, faces: &[usize]) -> Vec<Vec<usize>> {
    let list = chosen(mesh, faces);
    let mut sets = Sets::new(list.len());
    for users in edge_users(mesh, &list).into_values() {
        for i in 0..users.len() {
            for j in i + 1..users.len() {
                if same_uvs(mesh, &list, users[i], users[j]) {
                    sets.join(users[i].0, users[j].0);
                }
            }
        }
    }
    sets.groups().into_iter().map(|g| g.into_iter().map(|k| list[k]).collect()).collect()
}

/// Pack the islands of `faces` (all if empty) into the unit square without overlap.
pub fn pack(mesh: &mut Mesh, faces: &[usize], margin: f32, rotate: bool) {
    let list = islands(mesh, faces);
    pack_islands(mesh, &list, margin, rotate);
}

/// Scale the islands so each has the same picture area per square metre.
pub fn average_scale(mesh: &mut Mesh, faces: &[usize]) {
    let list = islands(mesh, faces);
    let areas: Vec<(f32, f32)> = list.iter().map(|i| (i.iter().map(|&f| mesh.face_area(f)).sum(), i.iter().map(|&f| uv_area(mesh, f)).sum())).collect();
    let (total, total_uv) = areas.iter().filter(|(a, u)| *a > 0.0 && *u > 0.0).fold((0.0, 0.0), |(t, tu), (a, u)| (t + a, tu + u));
    if !(total > 0.0 && total_uv > 0.0) {
        return;
    }
    let ratio = total_uv / total;
    for (island, &(a, u)) in list.iter().zip(&areas) {
        if !(a > 0.0 && u > 0.0) {
            continue;
        }
        let k = (ratio * a / u).sqrt();
        let (lo, hi) = bounds_of(mesh, island);
        let c = (lo + hi) * 0.5;
        for &f in island {
            for uv in &mut mesh.faces[f].uvs {
                *uv = c + (*uv - c) * k;
            }
        }
    }
}

/// Move, turn or scale the UVs of `faces` (all if empty).
pub fn transform(mesh: &mut Mesh, faces: &[usize], m: Affine2) {
    for f in chosen(mesh, faces) {
        for uv in &mut mesh.faces[f].uvs {
            *uv = m.transform_point2(*uv);
        }
    }
}

/// Mark as seams the edges where islands meet (and clear the others).
pub fn seams_from_islands(mesh: &mut Mesh) {
    let all: Vec<usize> = (0..mesh.faces.len()).collect();
    let mut seams: Vec<(u32, u32)> = edge_users(mesh, &all)
        .into_iter()
        .filter(|(_, users)| users.iter().enumerate().any(|(i, &a)| users[i + 1..].iter().any(|&b| !same_uvs(mesh, &all, a, b))))
        .map(|(k, _)| k)
        .collect();
    seams.sort_unstable();
    mesh.seams = seams;
}

/// How much a face's picture is stretched: its share of the UV area over its share of the
/// mesh's area (1 = even).
pub fn stretch(mesh: &Mesh, face: usize) -> f32 {
    stretch_all(mesh).get(face).copied().unwrap_or(1.0)
}

/// [`stretch`] of every face at once.
pub fn stretch_all(mesh: &Mesh) -> Vec<f32> {
    let n = mesh.faces.len();
    let area: Vec<f32> = (0..n).map(|f| mesh.face_area(f)).collect();
    let uv: Vec<f32> = (0..n).map(|f| uv_area(mesh, f)).collect();
    let (total, total_uv) = (area.iter().sum::<f32>(), uv.iter().sum::<f32>());
    (0..n).map(|f| if area[f] > 0.0 && total > 0.0 && total_uv > 0.0 { (uv[f] / total_uv) / (area[f] / total) } else { 1.0 }).collect()
}

/// The smallest and the largest UV of `faces` (all if empty); zero for none.
pub fn uv_bounds(mesh: &Mesh, faces: &[usize]) -> (Vec2, Vec2) {
    bounds_of(mesh, &chosen(mesh, faces))
}

/// Mirror the UVs of `faces` (all if empty) about the middle of their bounds: left to right
/// if `horizontal`, else top to bottom.
pub fn flip(mesh: &mut Mesh, faces: &[usize], horizontal: bool) {
    let (lo, hi) = uv_bounds(mesh, faces);
    let c = lo + hi;
    let m = if horizontal { Affine2::from_cols(Vec2::NEG_X, Vec2::Y, Vec2::new(c.x, 0.0)) } else { Affine2::from_cols(Vec2::X, Vec2::NEG_Y, Vec2::new(0.0, c.y)) };
    transform(mesh, faces, m);
}

/// Turn the UVs of `faces` (all if empty) `degrees` counter-clockwise as the picture is seen,
/// about the middle of their bounds.
pub fn rotate(mesh: &mut Mesh, faces: &[usize], degrees: f32) {
    let (lo, hi) = uv_bounds(mesh, faces);
    let c = (lo + hi) * 0.5;
    // v runs down the picture, so a turn that looks counter-clockwise is clockwise in (u, v)
    let m = Affine2::from_translation(c) * Affine2::from_angle(-degrees.to_radians()) * Affine2::from_translation(-c);
    transform(mesh, faces, m);
}

/// Scale and move the UVs of `faces` (all if empty) to fill the square, 0..1 both ways (a way
/// they do not reach along goes to 0).
pub fn fit(mesh: &mut Mesh, faces: &[usize]) {
    let (lo, hi) = uv_bounds(mesh, faces);
    let size = hi - lo;
    let k = Vec2::new(if size.x > 0.0 { 1.0 / size.x } else { 0.0 }, if size.y > 0.0 { 1.0 / size.y } else { 0.0 });
    transform(mesh, faces, Affine2::from_scale(k) * Affine2::from_translation(-lo));
}

/// The faces to work on: those of `faces` that exist, once each, or all if it is empty.
fn chosen(mesh: &Mesh, faces: &[usize]) -> Vec<usize> {
    if faces.is_empty() {
        return (0..mesh.faces.len()).collect();
    }
    let mut out: Vec<usize> = faces.iter().copied().filter(|&f| f < mesh.faces.len()).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Disjoint sets of numbers (union-find).
pub(crate) struct Sets(Vec<usize>);

impl Sets {
    pub(crate) fn new(n: usize) -> Sets {
        Sets((0..n).collect())
    }

    pub(crate) fn find(&mut self, mut a: usize) -> usize {
        while self.0[a] != a {
            self.0[a] = self.0[self.0[a]];
            a = self.0[a];
        }
        a
    }

    pub(crate) fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }

    /// The members of each set, sets in the order of their first member.
    fn groups(&mut self) -> Vec<Vec<usize>> {
        let mut at = vec![usize::MAX; self.0.len()];
        let mut out: Vec<Vec<usize>> = Vec::new();
        for i in 0..self.0.len() {
            let r = self.find(i);
            if at[r] == usize::MAX {
                at[r] = out.len();
                out.push(Vec::new());
            }
            out[at[r]].push(i);
        }
        out
    }
}

/// The faces of `list` on each edge: (place in `list`, the corner the edge starts at).
fn edge_users(mesh: &Mesh, list: &[usize]) -> HashMap<(u32, u32), Vec<(usize, usize)>> {
    let mut out: HashMap<(u32, u32), Vec<(usize, usize)>> = HashMap::new();
    for (k, &f) in list.iter().enumerate() {
        for (i, (a, b)) in mesh.faces[f].edges().enumerate() {
            out.entry(edge_key(a, b)).or_default().push((k, i));
        }
    }
    out
}

/// The corner of `face` at vertex `v` on the edge starting at corner `start`.
fn corner_at(face: &Face, start: usize, v: u32) -> usize {
    if face.verts[start] == v {
        start
    } else {
        (start + 1) % face.verts.len()
    }
}

fn uv_of(face: &Face, corner: usize) -> Vec2 {
    face.uvs.get(corner).copied().unwrap_or(Vec2::ZERO)
}

/// Whether two faces' corners on their shared edge have the same UVs.
fn same_uvs(mesh: &Mesh, list: &[usize], (ka, ia): (usize, usize), (kb, ib): (usize, usize)) -> bool {
    let (fa, fb) = (&mesh.faces[list[ka]], &mesh.faces[list[kb]]);
    [ia, (ia + 1) % fa.verts.len()].iter().all(|&ca| {
        let (a, b) = (uv_of(fa, ca), uv_of(fb, corner_at(fb, ib, fa.verts[ca])));
        (a - b).abs().max_element() <= 1e-6 * (1.0 + a.abs().max_element())
    })
}

/// The smallest and the largest UV of these faces; zero for none.
fn bounds_of(mesh: &Mesh, faces: &[usize]) -> (Vec2, Vec2) {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for &f in faces {
        for uv in mesh.faces[f].uvs.iter().filter(|uv| uv.is_finite()) {
            lo = lo.min(*uv);
            hi = hi.max(*uv);
        }
    }
    if lo.x > hi.x {
        (Vec2::ZERO, Vec2::ZERO)
    } else {
        (lo, hi)
    }
}

/// A face's area in the picture.
fn uv_area(mesh: &Mesh, f: usize) -> f32 {
    let face = &mesh.faces[f];
    mesh.triangulate_face(f).iter().map(|t| ((uv_of(face, t[1]) - uv_of(face, t[0])).perp_dot(uv_of(face, t[2]) - uv_of(face, t[0])) * 0.5).abs()).sum()
}

fn positive(size: f32) -> f32 {
    if size.is_finite() && size.abs() > 1e-9 {
        size.abs()
    } else {
        1.0
    }
}

/// Right and up across the picture seen looking along `dir` with z up (looking straight down
/// or up: north up). A point's UV is (right, -up): u to the right, v down.
fn view_basis(dir: Vec3) -> (Vec3, Vec3) {
    let f = dir.try_normalize().unwrap_or(Vec3::NEG_Z);
    let hint = if f.z.abs() > 0.999 { Vec3::Y } else { Vec3::Z };
    let right = f.cross(hint).normalize();
    (right, right.cross(f))
}

/// Lay the face flat seen looking along `dir`, `k` picture units a metre.
fn project_face(mesh: &mut Mesh, f: usize, dir: Vec3, k: f32) {
    let (right, up) = view_basis(dir);
    let Mesh { positions, faces, .. } = mesh;
    let face = &mut faces[f];
    for (uv, &v) in face.uvs.iter_mut().zip(&face.verts) {
        let p = positions[v as usize];
        *uv = Vec2::new(p.dot(right), -p.dot(up)) * k;
    }
}

/// Lay the faces flat seen from the side they face on average, in metres.
fn project_chart(mesh: &mut Mesh, chart: &[usize]) {
    let mut n: Vec3 = chart.iter().map(|&f| mesh.face_normal(f) * mesh.face_area(f)).sum();
    if n.length_squared() < 1e-20 {
        n = chart.iter().map(|&f| mesh.face_normal(f)).sum();
    }
    for &f in chart {
        project_face(mesh, f, -n, 1.0);
    }
}

/// The faces joined by edges that are not seams.
fn seam_islands(mesh: &Mesh, list: &[usize], seams: &HashSet<(u32, u32)>) -> Vec<Vec<usize>> {
    let mut sets = Sets::new(list.len());
    for (key, users) in edge_users(mesh, list) {
        if !seams.contains(&key) {
            for w in users.windows(2) {
                sets.join(w[0].0, w[1].0);
            }
        }
    }
    sets.groups().into_iter().map(|g| g.into_iter().map(|k| list[k]).collect()).collect()
}

/// Charts grown face by face from the largest faces, over edges to faces that face less than
/// `angle_deg` away from the chart's mean.
fn angle_charts(mesh: &Mesh, list: &[usize], angle_deg: f32) -> Vec<Vec<usize>> {
    let angle = if angle_deg.is_finite() { angle_deg.clamp(1.0, 89.0) } else { FALLBACK_ANGLE };
    let limit = angle.to_radians().cos();
    let normals: Vec<Vec3> = list.iter().map(|&f| mesh.face_normal(f)).collect();
    let areas: Vec<f32> = list.iter().map(|&f| mesh.face_area(f).max(1e-12)).collect();
    let users = edge_users(mesh, list);
    let next: Vec<Vec<usize>> = list
        .iter()
        .enumerate()
        .map(|(k, &f)| {
            let mut out: Vec<usize> = mesh.faces[f].edges().flat_map(|(a, b)| users[&edge_key(a, b)].iter().map(|u| u.0)).filter(|&g| g != k).collect();
            out.dedup();
            out
        })
        .collect();
    let mut order: Vec<usize> = (0..list.len()).collect();
    order.sort_by(|&a, &b| areas[b].total_cmp(&areas[a]));
    let mut taken = vec![false; list.len()];
    let mut out = Vec::new();
    for seed in order {
        if taken[seed] {
            continue;
        }
        taken[seed] = true;
        let mut chart = vec![seed];
        let mut sum = normals[seed] * areas[seed];
        let mut i = 0;
        while i < chart.len() {
            let k = chart[i];
            i += 1;
            for &g in &next[k] {
                let mean = sum.normalize_or_zero();
                if !taken[g] && (normals[g] == Vec3::ZERO || mean == Vec3::ZERO || normals[g].dot(mean) >= limit) {
                    taken[g] = true;
                    chart.push(g);
                    sum += normals[g] * areas[g];
                }
            }
        }
        let mut faces: Vec<usize> = chart.into_iter().map(|k| list[k]).collect();
        faces.sort_unstable();
        out.push(faces);
    }
    out
}

/// Bring islands to their true size, turn each to its smallest bounding rectangle and pack
/// them.
fn lay_out(mesh: &mut Mesh, islands: &[Vec<usize>], margin: f32) {
    for island in islands {
        scale_to_area(mesh, island);
        orient_min_rect(mesh, island);
    }
    pack_islands(mesh, islands, margin, true);
}

/// Scale an island so its picture has the area of its faces in square metres.
fn scale_to_area(mesh: &mut Mesh, island: &[usize]) {
    let area: f32 = island.iter().map(|&f| mesh.face_area(f)).sum();
    let uv: f32 = island.iter().map(|&f| uv_area(mesh, f)).sum();
    if area > 0.0 && uv > 0.0 && (area / uv).is_finite() {
        let k = (area / uv).sqrt();
        for &f in island {
            for p in &mut mesh.faces[f].uvs {
                *p *= k;
            }
        }
    }
}

/// Turn an island so its smallest bounding rectangle lies along the axes (of the turns that
/// do, the smallest).
fn orient_min_rect(mesh: &mut Mesh, island: &[usize]) {
    let pts: Vec<Vec2> = island.iter().flat_map(|&f| mesh.faces[f].uvs.iter().copied()).filter(|p| p.is_finite()).collect();
    let hull = convex_hull(pts);
    if hull.len() < 3 {
        return;
    }
    let area_along = |d: Vec2| {
        let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
        for q in &hull {
            let r = Vec2::new(q.dot(d), q.dot(d.perp()));
            lo = lo.min(r);
            hi = hi.max(r);
        }
        (hi.x - lo.x) * (hi.y - lo.y)
    };
    let mut best = (area_along(Vec2::X), Vec2::X);
    for i in 0..hull.len() {
        let e = (hull[(i + 1) % hull.len()] - hull[i]).normalize_or_zero();
        if e == Vec2::ZERO {
            continue;
        }
        let d = [e, e.perp(), -e, -e.perp()].into_iter().max_by(|a, b| a.x.total_cmp(&b.x)).unwrap_or(e);
        let a = area_along(d);
        if a < best.0 * (1.0 - 1e-4) {
            best = (a, d);
        }
    }
    let d = best.1;
    if d != Vec2::X {
        for &f in island {
            for p in &mut mesh.faces[f].uvs {
                *p = Vec2::new(p.dot(d), p.dot(d.perp()));
            }
        }
    }
}

/// The convex hull of points, counter-clockwise (Andrew's monotone chain).
fn convex_hull(mut pts: Vec<Vec2>) -> Vec<Vec2> {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let turn = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut hull: Vec<Vec2> = Vec::with_capacity(pts.len() + 1);
    for pass in [&pts[..], &pts.iter().rev().copied().collect::<Vec<_>>()[..]] {
        let floor = hull.len();
        for &p in pass {
            while hull.len() >= floor + 2 && turn(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// Pack these islands into the square, each kept to its shape, all scaled alike as large as
/// they go with `margin` between them and round the edge; with `rotate`, turned a quarter
/// where that packs them larger.
fn pack_islands(mesh: &mut Mesh, islands: &[Vec<usize>], margin: f32, rotate: bool) {
    if islands.is_empty() {
        return;
    }
    let boxes: Vec<(Vec2, Vec2)> = islands.iter().map(|i| bounds_of(mesh, i)).collect();
    let sizes: Vec<Vec2> = boxes.iter().map(|&(lo, hi)| hi - lo).collect();
    let mut tries = vec![vec![false; islands.len()]];
    if rotate {
        tries.push(sizes.iter().map(|s| s.y > s.x * 1.0001).collect());
    }
    let mut m = if margin.is_finite() { margin.clamp(0.0, 0.25) } else { 0.0 };
    let (scale, places, turned) = loop {
        let best = tries
            .iter()
            .filter_map(|t| {
                let turned: Vec<Vec2> = sizes.iter().zip(t).map(|(s, &t)| if t { Vec2::new(s.y, s.x) } else { *s }).collect();
                layout(&turned, m).map(|(k, p)| (k, p, t))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((k, p, t)) = best {
            break (k, p, t.clone());
        }
        // too many islands for the margin: narrow it
        m = if m > 1e-6 { m * 0.5 } else { 0.0 };
    };
    for (i, island) in islands.iter().enumerate() {
        let (lo, size) = (boxes[i].0, sizes[i]);
        let at = places[i] + Vec2::splat(m);
        for &f in island {
            for uv in &mut mesh.faces[f].uvs {
                let mut q = if uv.is_finite() { *uv - lo } else { Vec2::ZERO };
                if turned[i] {
                    q = Vec2::new(size.y - q.y, q.x);
                }
                *uv = at + q * scale;
            }
        }
    }
}

/// The largest scale at which rectangles of `sizes` pack into the unit square with `m` between
/// them and round the edge, and where each one's corner goes (less the margin); None if they
/// do not fit even at no size.
fn layout(sizes: &[Vec2], m: f32) -> Option<(f32, Vec<Vec2>)> {
    let room = 1.0 - m;
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| sizes[b].y.total_cmp(&sizes[a].y).then(sizes[b].x.total_cmp(&sizes[a].x)));
    let place = |s: f32| {
        let grown: Vec<Vec2> = sizes.iter().map(|z| *z * s + Vec2::splat(m)).collect();
        skyline(&grown, &order, room)
    };
    let mut best = (0.0, place(0.0)?);
    let biggest = sizes.iter().map(|s| s.max_element()).fold(0.0, f32::max);
    let area: f32 = sizes.iter().map(|s| s.x * s.y).sum();
    let mut hi = (room - m) / biggest;
    if area > 0.0 {
        hi = hi.min(room / area.sqrt());
    }
    if !(hi > 0.0 && hi.is_finite()) {
        return Some(best);
    }
    if let Some(p) = place(hi) {
        return Some((hi, p));
    }
    let mut lo = 0.0;
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        match place(mid) {
            Some(p) => {
                lo = mid;
                best = (mid, p);
            }
            None => hi = mid,
        }
        if hi - lo <= hi * 1e-5 {
            break;
        }
    }
    Some(best)
}

/// Bottom-left skyline packing of rectangles, tallest first, into a square of side `room`:
/// where each goes, or None if they do not all fit.
fn skyline(sizes: &[Vec2], order: &[usize], room: f32) -> Option<Vec<Vec2>> {
    let tol = room * 1e-6;
    // the top of what is placed so far: (x, y, width), left to right
    let mut sky: Vec<(f32, f32, f32)> = vec![(0.0, 0.0, room)];
    let mut at = vec![Vec2::ZERO; sizes.len()];
    for &i in order {
        let s = sizes[i];
        let mut best: Option<(f32, f32, usize)> = None;
        for k in 0..sky.len() {
            let x = sky[k].0;
            if x + s.x > room + tol {
                break;
            }
            let mut y = sky[k].1;
            let mut j = k + 1;
            while j < sky.len() && sky[j].0 < x + s.x - tol {
                y = y.max(sky[j].1);
                j += 1;
            }
            if y + s.y > room + tol {
                continue;
            }
            if best.is_none_or(|b| y < b.0 - tol) {
                best = Some((y, x, k));
            }
        }
        let (y, x, k) = best?;
        at[i] = Vec2::new(x, y);
        if s.x <= 0.0 {
            continue;
        }
        let end = x + s.x;
        while k < sky.len() && sky[k].0 < end {
            let seg_end = sky[k].0 + sky[k].2;
            if seg_end <= end {
                sky.remove(k);
            } else {
                sky[k] = (end, sky[k].1, seg_end - end);
                break;
            }
        }
        sky.insert(k, (x, y + s.y, s.x));
        // join level neighbours
        let mut j = k.saturating_sub(1);
        while j + 1 < sky.len() && j <= k + 1 {
            if (sky[j].1 - sky[j + 1].1).abs() <= tol {
                sky[j] = (sky[j].0, sky[j].1.max(sky[j + 1].1), sky[j].2 + sky[j + 1].2);
                sky.remove(j + 1);
            } else {
                j += 1;
            }
        }
    }
    Some(at)
}

/// The middle of the faces' bounding box and its diagonal.
fn centre_of(mesh: &Mesh, list: &[usize]) -> (Vec3, f32) {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for &f in list {
        for &v in &mesh.faces[f].verts {
            lo = lo.min(mesh.positions[v as usize]);
            hi = hi.max(mesh.positions[v as usize]);
        }
    }
    if lo.x > hi.x {
        return (Vec3::ZERO, 0.0);
    }
    ((lo + hi) * 0.5, (hi - lo).length())
}

fn cylinder(mesh: &mut Mesh, list: &[usize], axis: Axis, size: f32) {
    let (c, extent) = centre_of(mesh, list);
    let a = axis.vec();
    // across the axis, and where the picture's two edges meet: at the back, or underneath
    let (e1, e2, seam) = match axis {
        Axis::Z => (Vec3::X, Vec3::Y, Vec3::Y),
        Axis::X => (Vec3::Y, Vec3::Z, Vec3::NEG_Z),
        Axis::Y => (Vec3::Z, Vec3::X, Vec3::NEG_Z),
    };
    let start = seam.dot(e2).atan2(seam.dot(e1));
    let top = list.iter().flat_map(|&f| mesh.faces[f].verts.iter()).map(|&v| (mesh.positions[v as usize] - c).dot(a)).fold(f32::NEG_INFINITY, f32::max);
    wrap(mesh, list, |p| {
        let d = p - c;
        let h = d.dot(a);
        let r = d - a * h;
        let u = (r.length() > extent * 1e-6).then(|| ((r.dot(e2).atan2(r.dot(e1)) - start) / TAU).rem_euclid(1.0));
        (u, (top - h) / size)
    });
}

fn sphere(mesh: &mut Mesh, list: &[usize]) {
    let (c, extent) = centre_of(mesh, list);
    // the picture's edges meet at the back (north)
    let start = PI / 2.0;
    wrap(mesh, list, |p| {
        let d = p - c;
        let r = d.length();
        if r <= extent * 1e-6 {
            return (None, 0.5);
        }
        let u = (Vec2::new(d.x, d.y).length() > r * 1e-5).then(|| ((d.y.atan2(d.x) - start) / TAU).rem_euclid(1.0));
        (u, 0.5 - (d.z / r).clamp(-1.0, 1.0).asin() / PI)
    });
}

/// Give each face's corners the (u, v) `at` finds for them, u round in 0..1 (None on the
/// axis): a face across the seam takes its u from one side, a corner on the axis the mean of
/// the face's others.
fn wrap(mesh: &mut Mesh, list: &[usize], at: impl Fn(Vec3) -> (Option<f32>, f32)) {
    for &f in list {
        let got: Vec<(Option<f32>, f32)> = mesh.faces[f].verts.iter().map(|&v| at(mesh.positions[v as usize])).collect();
        let (lo, hi) = got.iter().filter_map(|g| g.0).fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), u| (lo.min(u), hi.max(u)));
        let lift = |u: f32| if hi - lo > 0.5 && u < 0.5 { u + 1.0 } else { u };
        let (sum, count) = got.iter().filter_map(|g| g.0).fold((0.0, 0), |(s, n), u| (s + lift(u), n + 1));
        let mean = if count > 0 { sum / count as f32 } else { 0.5 };
        for (uv, (u, v)) in mesh.faces[f].uvs.iter_mut().zip(got) {
            *uv = Vec2::new(u.map_or(mean, lift), v);
        }
    }
}

/// Faces cut apart along seams and along the edges to faces outside them: each corner tied to
/// the point it shares with the corners it is joined to, the faces' triangles over those
/// points.
struct Sheet {
    /// (face, corner, point) for every corner.
    corners: Vec<(usize, usize, usize)>,
    points: Vec<Vec3>,
    tris: Vec<[usize; 3]>,
}

impl Sheet {
    fn new(mesh: &Mesh, faces: &[usize], seams: &HashSet<(u32, u32)>) -> Sheet {
        let mut first = Vec::with_capacity(faces.len());
        let mut total = 0;
        for &f in faces {
            first.push(total);
            total += mesh.faces[f].verts.len();
        }
        let mut sets = Sets::new(total);
        for (key, users) in edge_users(mesh, faces) {
            if seams.contains(&key) {
                continue;
            }
            for i in 0..users.len() {
                for &(kb, ib) in &users[i + 1..] {
                    let (ka, ia) = users[i];
                    if ka == kb {
                        continue;
                    }
                    for v in [key.0, key.1] {
                        let ca = corner_at(&mesh.faces[faces[ka]], ia, v);
                        let cb = corner_at(&mesh.faces[faces[kb]], ib, v);
                        sets.join(first[ka] + ca, first[kb] + cb);
                    }
                }
            }
        }
        let mut point_of = vec![usize::MAX; total];
        let mut corners = Vec::with_capacity(total);
        let mut points = Vec::new();
        for (k, &f) in faces.iter().enumerate() {
            for (c, &v) in mesh.faces[f].verts.iter().enumerate() {
                let r = sets.find(first[k] + c);
                if point_of[r] == usize::MAX {
                    point_of[r] = points.len();
                    points.push(mesh.positions[v as usize]);
                }
                corners.push((f, c, point_of[r]));
            }
        }
        let mut tris = Vec::new();
        for (k, &f) in faces.iter().enumerate() {
            for t in mesh.triangulate_face(f) {
                let p = t.map(|c| corners[first[k] + c].2);
                if p[0] != p[1] && p[1] != p[2] && p[2] != p[0] {
                    tris.push(p);
                }
            }
        }
        Sheet { corners, points, tris }
    }

    /// Whether the sheet is a disk: one piece, every edge on one or two triangles, a single
    /// boundary loop and Euler characteristic 1 (no handles, no holes).
    fn is_disk(&self) -> bool {
        if self.tris.is_empty() {
            return false;
        }
        let mut edges: HashMap<(usize, usize), u32> = HashMap::new();
        let mut used = vec![false; self.points.len()];
        for t in &self.tris {
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
                used[a] = true;
            }
        }
        if edges.values().any(|&n| n > 2) {
            return false;
        }
        let verts = used.iter().filter(|&&u| u).count() as i64;
        if verts - edges.len() as i64 + self.tris.len() as i64 != 1 {
            return false;
        }
        let mut loops = Sets::new(self.points.len());
        let mut on_border = vec![false; self.points.len()];
        for (&(a, b), _) in edges.iter().filter(|(_, n)| **n == 1) {
            loops.join(a, b);
            on_border[a] = true;
            on_border[b] = true;
        }
        let mut roots: Vec<usize> = (0..self.points.len()).filter(|&p| on_border[p]).map(|p| loops.find(p)).collect();
        roots.sort_unstable();
        roots.dedup();
        roots.len() == 1
    }

    /// Give the faces' corners the sheet's conformal map.
    fn flatten(&self, mesh: &mut Mesh) {
        let uv = self.lscm();
        for &(f, c, p) in &self.corners {
            mesh.faces[f].uvs[c] = uv[p];
        }
    }

    /// The least-squares conformal map (Lévy et al. 2002): a UV for each point that keeps
    /// each triangle's angles as well as can be, two points far apart pinned. The points are
    /// first projected flat; conjugate gradients take it from there.
    fn lscm(&self) -> Vec<Vec2> {
        let n = self.points.len();
        let normal: Vec3 = self.tris.iter().map(|t| (self.points[t[1]] - self.points[t[0]]).cross(self.points[t[2]] - self.points[t[0]])).sum();
        let (right, up) = view_basis(-normal);
        // in (right, up): counter-clockwise as the faces are seen from outside
        let mut x: Vec<DVec2> = self.points.iter().map(|p| DVec2::new(p.dot(right) as f64, p.dot(up) as f64)).collect();
        let done = |x: Vec<DVec2>| x.iter().map(|p| Vec2::new(p.x as f32, -p.y as f32)).collect();
        let centre = self.points.iter().sum::<Vec3>() / n.max(1) as f32;
        let far = |from: Vec3| (0..n).max_by(|&a, &b| self.points[a].distance_squared(from).total_cmp(&self.points[b].distance_squared(from)));
        let Some(a) = far(centre) else { return done(x) };
        let Some(b) = far(self.points[a]) else { return done(x) };
        let span = self.points[a].distance(self.points[b]) as f64;
        if a == b || span.is_nan() || span <= 0.0 {
            return done(x);
        }
        if x[a].distance(x[b]) < span * 1e-6 {
            x[b] = x[a] + DVec2::new(span, 0.0);
        }
        let mut var = vec![usize::MAX; n];
        let mut count = 0;
        for (p, v) in var.iter_mut().enumerate() {
            if p != a && p != b {
                *v = count;
                count += 1;
            }
        }
        // two rows a triangle (the Cauchy-Riemann equations in its own plane), each weighted
        // by the square root of its area
        let mut start = vec![0];
        let mut entries: Vec<(usize, f64)> = Vec::new();
        let mut rhs = Vec::new();
        for t in &self.tris {
            let p = t.map(|i| self.points[i].as_dvec3());
            let (e1, e2) = (p[1] - p[0], p[2] - p[0]);
            let cross = e1.cross(e2);
            let area = cross.length() * 0.5;
            let longest = e1.length_squared().max(e2.length_squared()).max((p[2] - p[1]).length_squared());
            let sound = area > 1e-9 * longest;
            if !sound {
                continue;
            }
            let ax = e1.normalize();
            let ay = cross.normalize().cross(ax);
            let q = [DVec2::ZERO, DVec2::new(e1.length(), 0.0), DVec2::new(e2.dot(ax), e2.dot(ay))];
            let s = 0.5 / area.sqrt();
            for row in 0..2 {
                let mut b = 0.0;
                for j in 0..3 {
                    let e = q[(j + 2) % 3] - q[(j + 1) % 3];
                    let (cu, cv) = if row == 0 { (-e.y * s, -e.x * s) } else { (e.x * s, -e.y * s) };
                    let pt = t[j];
                    if var[pt] == usize::MAX {
                        b -= cu * x[pt].x + cv * x[pt].y;
                    } else {
                        entries.push((2 * var[pt], cu));
                        entries.push((2 * var[pt] + 1, cv));
                    }
                }
                rhs.push(b);
                start.push(entries.len());
            }
        }
        let mut unknown = vec![0.0; 2 * count];
        for p in 0..n {
            if var[p] != usize::MAX {
                unknown[2 * var[p]] = x[p].x;
                unknown[2 * var[p] + 1] = x[p].y;
            }
        }
        least_squares(&start, &entries, &rhs, &mut unknown, span);
        for p in 0..n {
            if var[p] != usize::MAX {
                x[p] = DVec2::new(unknown[2 * var[p]], unknown[2 * var[p] + 1]);
            }
        }
        done(x)
    }
}

/// Least squares: `x` (holding a first guess) minimising |A x - b|, A given row by row
/// (`entries[start[r]..start[r + 1]]`), by conjugate gradients on the normal equations
/// AᵀA x = Aᵀb, preconditioned by symmetric successive over-relaxation. `size`: how large the
/// answer is, for when to stop.
fn least_squares(start: &[usize], entries: &[(usize, f64)], b: &[f64], x: &mut [f64], size: f64) {
    const OMEGA: f64 = 1.5;
    let n = x.len();
    // the rows each unknown is in
    let mut col_start = vec![0usize; n + 1];
    for &(c, _) in entries {
        col_start[c + 1] += 1;
    }
    for i in 0..n {
        col_start[i + 1] += col_start[i];
    }
    let mut in_rows = vec![(0usize, 0.0f64); entries.len()];
    let mut fill = col_start.clone();
    for r in 0..b.len() {
        for &(c, a) in &entries[start[r]..start[r + 1]] {
            in_rows[fill[c]] = (r, a);
            fill[c] += 1;
        }
    }
    // AᵀA row by row: its diagonal apart, the rest in column order (those left of the
    // diagonal up to `mid`); and Aᵀb
    let mut acc = vec![0.0f64; n];
    let mut seen = vec![false; n];
    let mut touched = Vec::new();
    let (mut row_start, mut mid) = (vec![0usize], Vec::with_capacity(n));
    let (mut cols, mut vals) = (Vec::new(), Vec::new());
    let mut diag = vec![1.0f64; n];
    let mut atb = vec![0.0; n];
    for c in 0..n {
        for &(r, a) in &in_rows[col_start[c]..col_start[c + 1]] {
            atb[c] += a * b[r];
            for &(c2, a2) in &entries[start[r]..start[r + 1]] {
                if !seen[c2] {
                    seen[c2] = true;
                    touched.push(c2);
                }
                acc[c2] += a * a2;
            }
        }
        touched.sort_unstable();
        mid.push(cols.len() + touched.partition_point(|&c2| c2 < c));
        for &c2 in &touched {
            if c2 != c {
                cols.push(c2);
                vals.push(acc[c2]);
            } else if acc[c2] > 0.0 {
                diag[c] = acc[c2];
            }
            acc[c2] = 0.0;
            seen[c2] = false;
        }
        touched.clear();
        row_start.push(cols.len());
    }
    let times = |v: &[f64], out: &mut [f64]| {
        for (i, o) in out.iter_mut().enumerate() {
            *o = diag[i] * v[i] + (row_start[i]..row_start[i + 1]).map(|k| vals[k] * v[cols[k]]).sum::<f64>();
        }
    };
    let precondition = |r: &[f64], z: &mut [f64]| {
        for i in 0..n {
            let s = r[i] - (row_start[i]..mid[i]).map(|k| vals[k] * z[cols[k]]).sum::<f64>();
            z[i] = s * OMEGA / diag[i];
        }
        for i in 0..n {
            z[i] *= diag[i] / OMEGA;
        }
        for i in (0..n).rev() {
            let s = z[i] - (mid[i]..row_start[i + 1]).map(|k| vals[k] * z[cols[k]]).sum::<f64>();
            z[i] = s * OMEGA / diag[i];
        }
    };
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let mut r = vec![0.0; n];
    times(x, &mut r);
    for (ri, bi) in r.iter_mut().zip(&atb) {
        *ri = bi - *ri;
    }
    let mut z = vec![0.0; n];
    precondition(&r, &mut z);
    let mut p = z.clone();
    let mut rz = dot(&r, &z);
    let stop = (dot(&r, &r) * 1e-14).max((size * 1e-11).powi(2));
    let mut q = vec![0.0; n];
    for _ in 0..(4 * n + 100).min(20_000) {
        if dot(&r, &r) <= stop {
            break;
        }
        times(&p, &mut q);
        let pq = dot(&p, &q);
        if pq.is_nan() || pq <= 0.0 {
            break;
        }
        let alpha = rz / pq;
        for (xi, pi) in x.iter_mut().zip(&p) {
            *xi += alpha * pi;
        }
        for (ri, qi) in r.iter_mut().zip(&q) {
            *ri -= alpha * qi;
        }
        precondition(&r, &mut z);
        let rz2 = dot(&r, &z);
        let beta = rz2 / rz;
        for (pi, zi) in p.iter_mut().zip(&z) {
            *pi = zi + beta * *pi;
        }
        rz = rz2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use glam::{Mat4, Quat};

    /// Every face's UV triangles.
    fn uv_tris(m: &Mesh, faces: &[usize]) -> Vec<[Vec2; 3]> {
        chosen(m, faces).into_iter().flat_map(|f| m.triangulate_face(f).into_iter().map(move |t| t.map(|c| m.faces[f].uvs[c]))).collect()
    }

    fn signed(t: &[Vec2; 3]) -> f32 {
        (t[1] - t[0]).perp_dot(t[2] - t[0])
    }

    /// Whether two triangles' insides overlap (touching is not overlapping).
    fn overlap(a: &[Vec2; 3], b: &[Vec2; 3]) -> bool {
        for tri in [a, b] {
            for i in 0..3 {
                let n = (tri[(i + 1) % 3] - tri[i]).perp();
                let along = |t: &[Vec2; 3]| t.iter().map(|p| p.dot(n)).fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), d| (lo.min(d), hi.max(d)));
                let ((alo, ahi), (blo, bhi)) = (along(a), along(b));
                let eps = 1e-6 * n.length();
                if ahi <= blo + eps || bhi <= alo + eps {
                    return false;
                }
            }
        }
        true
    }

    fn assert_no_overlap(m: &Mesh, faces: &[usize]) {
        let tris: Vec<[Vec2; 3]> = uv_tris(m, faces).into_iter().filter(|t| signed(t).abs() > 1e-12).collect();
        for i in 0..tris.len() {
            for j in i + 1..tris.len() {
                assert!(!overlap(&tris[i], &tris[j]), "triangles {i} and {j} overlap: {:?} {:?}", tris[i], tris[j]);
            }
        }
    }

    fn assert_in_square(m: &Mesh, margin: f32) {
        let (lo, hi) = uv_bounds(m, &[]);
        assert!(lo.min_element() >= margin - 1e-4 && hi.max_element() <= 1.0 - margin + 1e-4, "{lo} {hi}");
    }

    /// The ratios of UV to 3D length over every triangle edge: (smallest, largest).
    fn length_ratios(m: &Mesh) -> (f32, f32) {
        let mut out = (f32::INFINITY, 0.0f32);
        for f in 0..m.faces.len() {
            for t in m.triangulate_face(f) {
                for i in 0..3 {
                    let (a, b) = (t[i], t[(i + 1) % 3]);
                    let p = |c: usize| m.positions[m.faces[f].verts[c] as usize];
                    let r = m.faces[f].uvs[a].distance(m.faces[f].uvs[b]) / p(a).distance(p(b));
                    out = (out.0.min(r), out.1.max(r));
                }
            }
        }
        out
    }

    #[test]
    fn a_tilted_grid_unwraps_conformally_without_distortion() {
        let mut m = Primitive::Grid { size: Vec2::new(3.0, 2.0), x_div: 6, y_div: 4 }.build();
        m.transform(Mat4::from_rotation_translation(Quat::from_euler(glam::EulerRot::XYZ, 0.4, -0.7, 1.1), Vec3::new(5.0, -2.0, 1.0)));
        unwrap(&mut m, &[], Unwrap::Conformal, 0.02);
        let (lo, hi) = length_ratios(&m);
        assert!(hi / lo < 1.001, "{lo} {hi}");
        assert!(uv_tris(&m, &[]).iter().all(|t| signed(t) < 0.0), "a triangle is turned over");
        assert_in_square(&m, 0.02);
        assert_eq!(islands(&m, &[]).len(), 1);
    }

    #[test]
    fn a_bent_strip_unrolls_flat() {
        // a quarter of a cylinder: bends one way only, so it unrolls into a rectangle
        let (seg, rows) = (24u32, 6u32);
        let mut m = Mesh::default();
        for j in 0..=rows {
            for i in 0..=seg {
                let a = PI / 2.0 * i as f32 / seg as f32;
                m.add_vertex(Vec3::new(a.cos() * 2.0, j as f32 * 0.5, a.sin() * 2.0));
            }
        }
        let at = |i: u32, j: u32| j * (seg + 1) + i;
        for j in 0..rows {
            for i in 0..seg {
                m.add_face(vec![at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j)], vec![Vec2::ZERO; 4], 0);
            }
        }
        unwrap(&mut m, &[], Unwrap::Conformal, 0.0);
        let (lo, hi) = length_ratios(&m);
        assert!(hi / lo < 1.003, "{lo} {hi}");
        assert!(uv_tris(&m, &[]).iter().all(|t| signed(t) < 0.0));
        let (a, b) = uv_bounds(&m, &[]);
        let size = b - a;
        let want = PI / 3.0; // an arc of pi metres by 3 metres
        assert!((size.max_element() / size.min_element() - want).abs() < 0.01, "{size}");
    }

    #[test]
    fn a_cube_cut_along_every_edge_packs_six_squares_apart() {
        let mut m = Primitive::Cube { size: Vec3::ONE }.build();
        for (a, b) in m.edges() {
            m.set_seam(a, b, true);
        }
        unwrap(&mut m, &[], Unwrap::Conformal, 0.05);
        assert_eq!(islands(&m, &[]).len(), 6);
        assert_no_overlap(&m, &[]);
        assert_in_square(&m, 0.05);
        let (lo, hi) = length_ratios(&m);
        assert!(hi / lo < 1.001);
        // the islands keep the margin between them
        let boxes: Vec<(Vec2, Vec2)> = islands(&m, &[]).iter().map(|i| uv_bounds(&m, i)).collect();
        for i in 0..6 {
            for j in i + 1..6 {
                let gap = (boxes[j].0 - boxes[i].1).max(boxes[i].0 - boxes[j].1);
                assert!(gap.max_element() >= 0.05 - 1e-4, "{gap}");
            }
        }
    }

    #[test]
    fn a_cube_cut_into_a_cross_unfolds_in_one_piece() {
        let mut m = Primitive::Cube { size: Vec3::ONE }.build();
        // the four upright edges and the top's edges but the front one: the sides hang on
        // the bottom, the top on the front
        let pos = m.positions.clone();
        for (a, b) in m.edges() {
            let (pa, pb) = (pos[a as usize], pos[b as usize]);
            let top = pa.z > 0.5 && pb.z > 0.5;
            if (pa.z - pb.z).abs() > 0.5 || (top && !(pa.y < 0.0 && pb.y < 0.0)) {
                m.set_seam(a, b, true);
            }
        }
        assert_eq!(m.seams.len(), 7);
        let seams_before = m.seams.clone();
        unwrap(&mut m, &[], Unwrap::Conformal, 0.01);
        assert_eq!(islands(&m, &[]).len(), 1);
        assert_no_overlap(&m, &[]);
        assert_in_square(&m, 0.01);
        let (lo, hi) = length_ratios(&m);
        assert!(hi / lo < 1.001, "{lo} {hi}");
        seams_from_islands(&mut m);
        assert_eq!(m.seams, seams_before);
    }

    #[test]
    fn a_closed_sphere_without_seams_is_cut_where_it_bends() {
        let mut m = Primitive::UvSphere { radius: 1.0, segments: 24, rings: 12 }.build();
        unwrap(&mut m, &[], Unwrap::Conformal, 0.01);
        assert!(islands(&m, &[]).len() > 1);
        assert!(uv_tris(&m, &[]).iter().all(|t| signed(t) <= 0.0));
        assert_no_overlap(&m, &[]);
        assert_in_square(&m, 0.01);
        let mut c = Primitive::Cylinder { radius: 0.5, height: 2.0, segments: 16 }.build();
        unwrap(&mut c, &[], Unwrap::Conformal, 0.01);
        assert_no_overlap(&c, &[]);
    }

    #[test]
    fn smart_project_of_a_sphere_has_no_overlaps() {
        let mut m = Primitive::UvSphere { radius: 0.5, segments: 32, rings: 16 }.build();
        unwrap(&mut m, &[], Unwrap::Smart { angle_deg: 66.0 }, 0.01);
        let n = islands(&m, &[]).len();
        assert!(n > 2 && n < 40, "{n} islands");
        assert!(uv_tris(&m, &[]).iter().all(|t| signed(t) <= 0.0));
        assert_no_overlap(&m, &[]);
        assert_in_square(&m, 0.01);
    }

    #[test]
    fn smart_project_of_a_cube_lays_each_side_flat_and_marks_its_edges() {
        let mut m = Primitive::Cube { size: Vec3::new(1.0, 2.0, 3.0) }.build();
        unwrap(&mut m, &[], Unwrap::Smart { angle_deg: 66.0 }, 0.02);
        assert_eq!(islands(&m, &[]).len(), 6);
        assert_no_overlap(&m, &[]);
        let s = stretch_all(&m);
        assert!(s.iter().all(|s| (s - 1.0).abs() < 1e-3), "{s:?}");
        seams_from_islands(&mut m);
        assert_eq!(m.seams.len(), 12);
    }

    #[test]
    fn cube_projection_lays_the_six_sides_apart_true_to_each_other() {
        let mut m = Primitive::Cube { size: Vec3::new(2.0, 3.0, 4.0) }.build();
        unwrap(&mut m, &[], Unwrap::Cube { size: 1.0 }, 0.01);
        assert_eq!(islands(&m, &[]).len(), 6);
        assert_no_overlap(&m, &[]);
        assert_in_square(&m, 0.0);
        // (one scale for all: a picture area per square metre that is the same everywhere)
        let s = stretch_all(&m);
        assert!(s.iter().all(|s| (s - 1.0).abs() < 1e-3), "{s:?}");
        assert!(uv_tris(&m, &[]).iter().all(|t| signed(t) < 0.0), "a side is mirrored");
    }

    #[test]
    fn planar_projection_looks_along_its_direction() {
        let mut m = Primitive::Plane { size: Vec2::new(2.0, 2.0) }.build();
        unwrap(&mut m, &[], Unwrap::Planar { direction: Vec3::NEG_Z, size: 2.0 }, 0.0);
        for (c, &v) in m.faces[0].verts.iter().enumerate() {
            let p = m.positions[v as usize];
            assert!((m.faces[0].uvs[c] - Vec2::new(p.x, -p.y) / 2.0).length() < 1e-6);
        }
        // a wall seen from the south: east right, up up
        let mut w = Primitive::Cube { size: Vec3::ONE }.build();
        let front = (0..w.faces.len()).find(|&f| w.face_normal(f).y < -0.5).unwrap();
        unwrap(&mut w, &[front], Unwrap::Planar { direction: Vec3::Y, size: 1.0 }, 0.0);
        for (c, &v) in w.faces[front].verts.iter().enumerate() {
            let p = w.positions[v as usize];
            assert!((w.faces[front].uvs[c] - Vec2::new(p.x, -p.z)).length() < 1e-6);
        }
    }

    #[test]
    fn cylinder_and_sphere_wrap_once_round() {
        let mut m = Primitive::Cylinder { radius: 0.5, height: 2.0, segments: 16 }.build();
        let sides: Vec<usize> = (0..m.faces.len()).filter(|&f| m.face_normal(f).z.abs() < 0.5).collect();
        unwrap(&mut m, &sides, Unwrap::Cylinder { axis: Axis::Z, size: 2.0 }, 0.0);
        for &f in &sides {
            let (lo, hi) = uv_bounds(&m, &[f]);
            assert!((hi.x - lo.x - 1.0 / 16.0).abs() < 1e-4, "face {f}: {lo} {hi}");
            assert!(lo.y.abs() < 1e-5 && (hi.y - 1.0).abs() < 1e-5);
        }
        let (lo, hi) = uv_bounds(&m, &sides);
        assert!(lo.x.abs() < 1e-4 && (hi.x - 1.0).abs() < 1e-4, "{lo} {hi}");
        assert!(uv_tris(&m, &sides).iter().all(|t| signed(t) < 0.0));

        let mut s = Primitive::UvSphere { radius: 0.5, segments: 16, rings: 8 }.build();
        unwrap(&mut s, &[], Unwrap::Sphere, 0.0);
        let (lo, hi) = uv_bounds(&s, &[]);
        assert!(lo.y.abs() < 1e-5 && (hi.y - 1.0).abs() < 1e-5, "{lo} {hi}");
        for f in 0..s.faces.len() {
            let (lo, hi) = uv_bounds(&s, &[f]);
            assert!(hi.x - lo.x < 0.1, "face {f} spans {lo} {hi}");
        }
        assert!(uv_tris(&s, &[]).iter().all(|t| signed(t) < 1e-9));
    }

    #[test]
    fn lightmap_pack_gives_each_face_an_island_of_its_own() {
        let mut m = Primitive::Stairs { width: 1.2, depth: 2.4, height: 1.6, steps: 5 }.build();
        unwrap(&mut m, &[], Unwrap::LightmapPack, 0.01);
        assert_eq!(islands(&m, &[]).len(), m.faces.len());
        assert_no_overlap(&m, &[]);
        assert_in_square(&m, 0.01);
        let s = stretch_all(&m);
        assert!(s.iter().all(|s| (s - 1.0).abs() < 1e-3), "{s:?}");
    }

    #[test]
    fn islands_are_found_by_shared_uvs_and_packed_apart() {
        let mut m = Primitive::Grid { size: Vec2::new(2.0, 1.0), x_div: 2, y_div: 1 }.build();
        assert_eq!(islands(&m, &[]).len(), 1);
        // the right square's own UVs: now two islands, on top of each other
        let r = &mut m.faces[1];
        r.uvs = vec![Vec2::new(0.0, 1.0), Vec2::new(1.0, 1.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 0.0)];
        assert_eq!(islands(&m, &[]), vec![vec![0], vec![1]]);
        pack(&mut m, &[], 0.1, false);
        assert_no_overlap(&m, &[]);
        assert_in_square(&m, 0.1);
        for f in 0..2 {
            let (lo, hi) = uv_bounds(&m, &[f]);
            assert!(((hi - lo).x - (hi - lo).y).abs() < 1e-5, "squares stay square");
        }
        // a tall island beside a wide one packs larger turned
        let mut t = Primitive::Grid { size: Vec2::new(1.0, 4.0), x_div: 1, y_div: 1 }.build();
        let wide = Primitive::Grid { size: Vec2::new(4.0, 1.0), x_div: 1, y_div: 1 }.build();
        t.append(&wide, 0);
        t.faces[1].uvs.iter_mut().for_each(|uv| *uv += Vec2::new(5.0, 0.0));
        let mut turned = t.clone();
        pack(&mut t, &[], 0.0, false);
        pack(&mut turned, &[], 0.0, true);
        let size = |m: &Mesh| {
            let (lo, hi) = uv_bounds(m, &[0]);
            (hi - lo).max_element()
        };
        assert!((size(&t) - 0.8).abs() < 1e-3 && (size(&turned) - 1.0).abs() < 1e-3, "{} {}", size(&turned), size(&t));
        assert_no_overlap(&turned, &[]);
        assert!(uv_tris(&turned, &[]).iter().all(|t| signed(t) < 0.0), "turning must not mirror");
    }

    #[test]
    fn many_islands_pack_tightly() {
        let mut m = Primitive::Grid { size: Vec2::new(8.0, 8.0), x_div: 8, y_div: 8 }.build();
        unwrap(&mut m, &[], Unwrap::LightmapPack, 0.0);
        // 64 unit squares: 8 a row
        let (lo, hi) = uv_bounds(&m, &[0]);
        assert!(((hi - lo).x - 0.125).abs() < 1e-3, "{lo} {hi}");
        assert_no_overlap(&m, &[]);
    }

    #[test]
    fn average_scale_evens_out_the_islands() {
        let mut m = Primitive::Grid { size: Vec2::new(2.0, 1.0), x_div: 2, y_div: 1 }.build();
        m.faces[1].uvs = vec![Vec2::new(0.0, 3.0), Vec2::new(3.0, 3.0), Vec2::new(3.0, 0.0), Vec2::new(0.0, 0.0)];
        assert!((stretch(&m, 0) - 0.2).abs() < 1e-5 && (stretch(&m, 1) - 1.8).abs() < 1e-5);
        average_scale(&mut m, &[]);
        assert!((stretch(&m, 0) - 1.0).abs() < 1e-4 && (stretch(&m, 1) - 1.0).abs() < 1e-4, "{} {}", stretch(&m, 0), stretch(&m, 1));
    }

    #[test]
    fn rotate_flip_and_fit_move_the_uvs() {
        let mut m = Primitive::Plane { size: Vec2::new(2.0, 1.0) }.build();
        let (lo, hi) = uv_bounds(&m, &[]);
        assert_eq!((lo, hi), (Vec2::ZERO, Vec2::new(2.0, 1.0)));
        let before = m.faces[0].uvs.clone();
        rotate(&mut m, &[], 90.0);
        let (lo, hi) = uv_bounds(&m, &[]);
        assert!((lo - Vec2::new(0.5, -0.5)).length() < 1e-5 && (hi - Vec2::new(1.5, 1.5)).length() < 1e-5, "{lo} {hi}");
        // the right edge's middle went to the top
        let i = before.iter().position(|uv| uv.x == 2.0).unwrap();
        assert!((m.faces[0].uvs[i].y - -0.5).abs() < 1e-5);
        rotate(&mut m, &[], -90.0);
        assert!(m.faces[0].uvs.iter().zip(&before).all(|(a, b)| a.distance(*b) < 1e-5));
        flip(&mut m, &[], true);
        assert!(m.faces[0].uvs.iter().zip(&before).all(|(a, b)| a.distance(Vec2::new(2.0 - b.x, b.y)) < 1e-6));
        flip(&mut m, &[], false);
        assert!(m.faces[0].uvs.iter().zip(&before).all(|(a, b)| a.distance(Vec2::new(2.0 - b.x, 1.0 - b.y)) < 1e-6));
        transform(&mut m, &[], Affine2::from_scale_angle_translation(Vec2::splat(3.0), 0.3, Vec2::new(4.0, -7.0)));
        fit(&mut m, &[]);
        let (lo, hi) = uv_bounds(&m, &[]);
        assert!(lo.length() < 1e-5 && (hi - Vec2::ONE).length() < 1e-5, "{lo} {hi}");
    }

    #[test]
    fn degenerate_input_does_not_panic() {
        let all = [
            Unwrap::Conformal,
            Unwrap::Smart { angle_deg: 66.0 },
            Unwrap::Smart { angle_deg: f32::NAN },
            Unwrap::Cube { size: 0.0 },
            Unwrap::Cylinder { axis: Axis::X, size: -1.0 },
            Unwrap::Sphere,
            Unwrap::Planar { direction: Vec3::ZERO, size: 1.0 },
            Unwrap::LightmapPack,
        ];
        let mut odd = Mesh::default();
        for p in [Vec3::ZERO, Vec3::ZERO, Vec3::X, Vec3::X * 2.0, Vec3::Y] {
            odd.add_vertex(p);
        }
        odd.add_face(vec![0, 1, 2], vec![Vec2::ZERO; 3], 0); // two corners in one place
        odd.add_face(vec![0, 2, 3], vec![Vec2::ZERO; 3], 0); // a line
        odd.faces.push(Face { verts: vec![0, 0, 4], uvs: vec![Vec2::ZERO; 2], material: 0, smooth: false }); // a corner twice, a UV short
        odd.add_face(vec![2, 3, 4, 1], vec![Vec2::ZERO; 4], 0);
        for how in all {
            let mut empty = Mesh::default();
            unwrap(&mut empty, &[], how, 0.01);
            let mut m = odd.clone();
            unwrap(&mut m, &[], how, 0.01);
            unwrap(&mut m, &[2, 99], how, 1.0);
            assert!(m.faces.iter().all(|f| f.uvs.iter().all(|uv| uv.is_finite())), "{how:?}");
        }
        let mut empty = Mesh::default();
        pack(&mut empty, &[], 0.1, true);
        average_scale(&mut empty, &[]);
        seams_from_islands(&mut empty);
        fit(&mut empty, &[]);
        rotate(&mut empty, &[], 30.0);
        flip(&mut empty, &[], true);
        assert_eq!(uv_bounds(&empty, &[]), (Vec2::ZERO, Vec2::ZERO));
        assert_eq!(stretch(&empty, 3), 1.0);
        let mut m = odd.clone();
        pack(&mut m, &[], 0.6, true);
        average_scale(&mut m, &[]);
        fit(&mut m, &[]);
        assert!(m.faces.iter().all(|f| f.uvs.iter().all(|uv| uv.is_finite())));
    }
}

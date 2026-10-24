//! Edit mode: picking vertices, edges and faces and the operations on them (extrude, inset,
//! bevel, loop cut, merge, fill, bridge, ...), as Blender's Edit Mode has them.
//!
//! Every operation takes the selection as it comes (numbers out of range are left out), leaves
//! a valid mesh whose faces still agree with their neighbours, and gives back what is selected
//! after it.

use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};

use glam::{Mat4, Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::primitives::planar_uvs;
use crate::{edge_key, Axis, Face, Mesh};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectMode {
    #[default]
    Vertex,
    Edge,
    Face,
}

/// What is selected, kept in step for all three modes: a face is selected when all its
/// vertices are, an edge when both of its are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub verts: BTreeSet<u32>,
    pub edges: BTreeSet<(u32, u32)>,
    pub faces: BTreeSet<usize>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.verts.is_empty() && self.edges.is_empty() && self.faces.is_empty()
    }

    /// These vertices, with the edges and faces they wholly make up.
    pub fn from_verts(mesh: &Mesh, verts: impl IntoIterator<Item = u32>) -> Selection {
        let n = mesh.positions.len() as u32;
        let verts: BTreeSet<u32> = verts.into_iter().filter(|&v| v < n).collect();
        let edges = mesh.edges().into_iter().filter(|(a, b)| verts.contains(a) && verts.contains(b)).collect();
        let faces = mesh.faces.iter().enumerate().filter(|(_, f)| f.verts.iter().all(|v| verts.contains(v))).map(|(i, _)| i).collect();
        Selection { verts, edges, faces }
    }

    /// These edges (those the mesh has), their vertices and the faces whose edges all are
    /// among them.
    pub fn from_edges(mesh: &Mesh, edges: impl IntoIterator<Item = (u32, u32)>) -> Selection {
        let all: HashSet<(u32, u32)> = mesh.edges().into_iter().collect();
        let edges: BTreeSet<(u32, u32)> = edges.into_iter().map(|(a, b)| edge_key(a, b)).filter(|e| all.contains(e)).collect();
        let verts = edges.iter().flat_map(|&(a, b)| [a, b]).collect();
        let faces = mesh.faces.iter().enumerate().filter(|(_, f)| f.edges().all(|(a, b)| edges.contains(&edge_key(a, b)))).map(|(i, _)| i).collect();
        Selection { verts, edges, faces }
    }

    /// These faces with their vertices and edges.
    pub fn from_faces(mesh: &Mesh, faces: impl IntoIterator<Item = usize>) -> Selection {
        let faces: BTreeSet<usize> = faces.into_iter().filter(|&f| f < mesh.faces.len()).collect();
        let mut verts = BTreeSet::new();
        let mut edges = BTreeSet::new();
        for &f in &faces {
            for (a, b) in mesh.faces[f].edges() {
                verts.insert(a);
                edges.insert(edge_key(a, b));
            }
        }
        Selection { verts, edges, faces }
    }

    pub fn all(mesh: &Mesh) -> Selection {
        Selection::from_verts(mesh, 0..mesh.positions.len() as u32)
    }

    /// Select or deselect a vertex (vertex mode: edges and faces follow).
    pub fn toggle_vert(&mut self, mesh: &Mesh, v: u32) {
        let mut verts = self.verts.clone();
        if !verts.remove(&v) {
            verts.insert(v);
        }
        *self = Selection::from_verts(mesh, verts);
    }

    /// Select or deselect an edge (edge mode: vertices and faces follow).
    pub fn toggle_edge(&mut self, mesh: &Mesh, edge: (u32, u32)) {
        let k = edge_key(edge.0, edge.1);
        let mut edges = self.edges.clone();
        if !edges.remove(&k) {
            edges.insert(k);
        }
        *self = Selection::from_edges(mesh, edges);
    }

    /// Select or deselect a face (face mode: vertices and edges follow).
    pub fn toggle_face(&mut self, mesh: &Mesh, face: usize) {
        let mut faces = self.faces.clone();
        if !faces.remove(&face) {
            faces.insert(face);
        }
        *self = Selection::from_faces(mesh, faces);
    }

    /// The selected vertices in order.
    pub fn vertices(&self) -> Vec<u32> {
        self.verts.iter().copied().collect()
    }
}

/// The selection with what is out of range left out.
fn kept(mesh: &Mesh, sel: &Selection) -> Selection {
    let all: HashSet<(u32, u32)> = mesh.edges().into_iter().collect();
    let n = mesh.positions.len() as u32;
    Selection {
        verts: sel.verts.iter().copied().filter(|&v| v < n).collect(),
        edges: sel.edges.iter().map(|&(a, b)| edge_key(a, b)).filter(|e| all.contains(e)).collect(),
        faces: sel.faces.iter().copied().filter(|&f| f < mesh.faces.len()).collect(),
    }
}

/// The faces an operation works on: the selected ones, or, when only vertices are given,
/// those whose vertices all are selected.
fn sel_faces(mesh: &Mesh, sel: &Selection) -> BTreeSet<usize> {
    let picked: BTreeSet<usize> = sel.faces.iter().copied().filter(|&f| f < mesh.faces.len()).collect();
    if !picked.is_empty() || sel.verts.is_empty() || !sel.edges.is_empty() {
        return picked;
    }
    mesh.faces.iter().enumerate().filter(|(_, f)| f.verts.iter().all(|v| sel.verts.contains(v))).map(|(i, _)| i).collect()
}

/// The edges an operation works on: the selected ones the mesh has and those of the selected
/// faces; when no edge is given, those whose two vertices are selected.
fn sel_edges(mesh: &Mesh, sel: &Selection) -> BTreeSet<(u32, u32)> {
    let all = mesh.edges();
    let set: HashSet<(u32, u32)> = all.iter().copied().collect();
    let mut out: BTreeSet<(u32, u32)> = sel.edges.iter().map(|&(a, b)| edge_key(a, b)).filter(|e| set.contains(e)).collect();
    for f in sel_faces(mesh, sel) {
        out.extend(mesh.faces[f].edges().map(|(a, b)| edge_key(a, b)));
    }
    if sel.edges.is_empty() {
        out.extend(all.into_iter().filter(|(a, b)| sel.verts.contains(a) && sel.verts.contains(b)));
    }
    out
}

/// The vertices an operation works on: the selected ones and those of selected edges and faces.
fn sel_verts(mesh: &Mesh, sel: &Selection) -> BTreeSet<u32> {
    let n = mesh.positions.len() as u32;
    let mut out: BTreeSet<u32> = sel.verts.iter().copied().filter(|&v| v < n).collect();
    for (a, b) in sel_edges(mesh, sel) {
        out.insert(a);
        out.insert(b);
    }
    out
}

/// Each edge as a face runs it (from, to) and that face.
fn directed(mesh: &Mesh) -> HashMap<(u32, u32), usize> {
    let mut out = HashMap::new();
    for (i, f) in mesh.faces.iter().enumerate() {
        for e in f.edges() {
            out.entry(e).or_insert(i);
        }
    }
    out
}

/// The vertices joined to each vertex by an edge.
fn neighbours(mesh: &Mesh) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); mesh.positions.len()];
    for (a, b) in mesh.edges() {
        out[a as usize].push(b);
        out[b as usize].push(a);
    }
    out
}

/// The faces round a vertex in order, each as (face, corner): the next face is the one across
/// the edge leaving the vertex in this one. `closed` when they go all the way round.
struct Fan {
    corners: Vec<(usize, usize)>,
    closed: bool,
}

impl Fan {
    /// The other end of each edge at the vertex: edge `i` comes into face `i`'s corner, face
    /// `i` leaves by edge `i + 1` (round to 0 when closed; an open fan has one edge more).
    fn others(&self, mesh: &Mesh) -> Vec<u32> {
        let mut out: Vec<u32> = self.corners.iter().map(|&(f, c)| prev_vert(&mesh.faces[f], c)).collect();
        if !self.closed {
            if let Some(&(f, c)) = self.corners.last() {
                out.push(next_vert(&mesh.faces[f], c));
            }
        }
        out
    }

    /// The edge face `i` leaves the vertex by.
    fn out_edge(&self, i: usize) -> usize {
        if self.closed {
            (i + 1) % self.corners.len()
        } else {
            i + 1
        }
    }
}

fn prev_vert(f: &Face, c: usize) -> u32 {
    let n = f.verts.len();
    f.verts[(c + n - 1) % n]
}

fn next_vert(f: &Face, c: usize) -> u32 {
    f.verts[(c + 1) % f.verts.len()]
}

/// The fan round `v`, if the faces there make one surface (each holds `v` once, no more
/// than two faces on an edge).
fn fan(mesh: &Mesh, dir: &HashMap<(u32, u32), usize>, vf: &[Vec<usize>], v: u32) -> Option<Fan> {
    let faces = vf.get(v as usize)?;
    let &start = faces.first()?;
    for &f in faces {
        if mesh.faces[f].verts.iter().filter(|&&x| x == v).count() != 1 {
            return None;
        }
    }
    let corner = |f: usize| mesh.faces[f].verts.iter().position(|&x| x == v).unwrap_or(0);
    let after = |f: usize| dir.get(&(next_vert(&mesh.faces[f], corner(f)), v)).copied();
    let before = |f: usize| dir.get(&(v, prev_vert(&mesh.faces[f], corner(f)))).copied();
    let mut order = vec![start];
    let mut closed = false;
    let mut f = start;
    while let Some(g) = after(f) {
        if g == start {
            closed = true;
            break;
        }
        if order.contains(&g) {
            break;
        }
        order.push(g);
        f = g;
    }
    if !closed {
        let mut f = start;
        while let Some(g) = before(f) {
            if order.contains(&g) {
                break;
            }
            order.insert(0, g);
            f = g;
        }
    }
    if order.len() != faces.len() {
        return None;
    }
    Some(Fan { corners: order.iter().map(|&f| (f, corner(f))).collect(), closed })
}

/// Drop the vertices no face uses (as [`Mesh::remove_unused`]) and marks on edges no face has
/// any more; each old vertex's new number (u32::MAX: gone).
fn compact(mesh: &mut Mesh) -> Vec<u32> {
    let mut used = vec![false; mesh.positions.len()];
    for f in &mesh.faces {
        for &v in &f.verts {
            used[v as usize] = true;
        }
    }
    let mut map = vec![u32::MAX; mesh.positions.len()];
    let mut kept = Vec::new();
    for (i, p) in mesh.positions.iter().enumerate() {
        if used[i] {
            map[i] = kept.len() as u32;
            kept.push(*p);
        }
    }
    mesh.positions = kept;
    mesh.remap(&map);
    prune_marks(mesh);
    map
}

/// Forget seams and sharp edges on edges the faces no longer have.
fn prune_marks(mesh: &mut Mesh) {
    if mesh.seams.is_empty() && mesh.sharp.is_empty() {
        return;
    }
    let edges: HashSet<(u32, u32)> = mesh.edges().into_iter().collect();
    mesh.seams.retain(|e| edges.contains(e));
    mesh.sharp.retain(|e| edges.contains(e));
}

/// Give the edges between copies (`copy` maps old vertices to new) the seams and sharpness of
/// the edges they copy.
fn copy_marks(mesh: &mut Mesh, copy: &BTreeMap<u32, u32>, edges: &BTreeSet<(u32, u32)>) {
    let to = |v: u32| copy.get(&v).copied().unwrap_or(v);
    for &(a, b) in edges {
        if mesh.is_seam(a, b) {
            mesh.set_seam(to(a), to(b), true);
        }
        if mesh.is_sharp(a, b) {
            mesh.set_sharp(to(a), to(b), true);
        }
    }
}

/// The new number of vertex `v`, if it is still there.
fn mapped(map: &[u32], v: u32) -> Option<u32> {
    map.get(v as usize).copied().filter(|&m| m != u32::MAX)
}

/// A face of these corners taking material and shading from `like`.
fn face_like(verts: Vec<u32>, uvs: Vec<Vec2>, like: &Face) -> Face {
    Face { verts, uvs, material: like.material, smooth: like.smooth }
}

/// Barycentric coordinates of `p` (projected into the triangle's plane), or none for a
/// degenerate triangle.
fn barycentric(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<Vec3> {
    let (v0, v1, v2) = (b - a, c - a, p - a);
    let (d00, d01, d11, d20, d21) = (v0.dot(v0), v0.dot(v1), v1.dot(v1), v2.dot(v0), v2.dot(v1));
    let den = d00 * d11 - d01 * d01;
    if den.abs() < 1e-14 {
        return None;
    }
    let v = (d11 * d20 - d01 * d21) / den;
    let w = (d00 * d21 - d01 * d20) / den;
    Some(Vec3::new(1.0 - v - w, v, w))
}

/// The UV that a point of face `f`'s plane has: interpolated in the face's triangle it lies in
/// (or is nearest to).
fn uv_at(mesh: &Mesh, f: usize, p: Vec3) -> Vec2 {
    let face = &mesh.faces[f];
    let pos = |i: usize| mesh.positions[face.verts[i] as usize];
    let mut best: Option<(f32, Vec2)> = None;
    for t in mesh.triangulate_face(f) {
        if let Some(b) = barycentric(p, pos(t[0]), pos(t[1]), pos(t[2])) {
            let score = b.min_element();
            if best.is_none_or(|(s, _)| score > s) {
                best = Some((score, face.uvs[t[0]] * b.x + face.uvs[t[1]] * b.y + face.uvs[t[2]] * b.z));
            }
        }
    }
    best.map(|b| b.1).unwrap_or_else(|| face.uvs.iter().copied().sum::<Vec2>() / face.uvs.len().max(1) as f32)
}

/// Planar UVs in metres for these faces as they are shaped now.
fn refresh_uvs(mesh: &mut Mesh, faces: &[usize]) {
    for &f in faces {
        let n = mesh.face_normal(f);
        if n != Vec3::ZERO {
            let pts: Vec<Vec3> = mesh.faces[f].verts.iter().map(|&v| mesh.positions[v as usize]).collect();
            mesh.faces[f].uvs = planar_uvs(&pts, n);
        }
    }
}

/// A polygon's normal by Newell's method (zero when degenerate).
fn polygon_normal(pts: &[Vec3]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n.normalize_or_zero()
}

/// A new face over these corners with planar UVs, material and shading from `like`.
fn planar_face(mesh: &Mesh, verts: Vec<u32>, like: &Face) -> Face {
    let pts: Vec<Vec3> = verts.iter().map(|&v| mesh.positions[v as usize]).collect();
    let uvs = planar_uvs(&pts, polygon_normal(&pts));
    face_like(verts, uvs, like)
}

/// Directed edges joined end to start into chains: each chain's vertices and whether it
/// closes on itself. A closed chain through a vertex twice is cut there into simple loops.
fn chains(edges: &[(u32, u32)]) -> Vec<(Vec<u32>, bool)> {
    let mut out: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut balance: HashMap<u32, i32> = HashMap::new();
    for &(a, b) in edges {
        if a != b && !out.get(&a).is_some_and(|l| l.contains(&b)) {
            out.entry(a).or_default().push(b);
            *balance.entry(a).or_default() += 1;
            *balance.entry(b).or_default() -= 1;
        }
    }
    // open chains first, from vertices more edges leave than come into
    let starts: Vec<u32> = out.keys().copied().filter(|v| balance[v] > 0).chain(out.keys().copied()).collect();
    let mut result = Vec::new();
    for s in starts {
        while out.get(&s).is_some_and(|l| !l.is_empty()) {
            let mut chain = vec![s];
            let mut cur = s;
            let mut closed = false;
            while let Some(n) = out.get_mut(&cur).and_then(|l| (!l.is_empty()).then(|| l.remove(0))) {
                if n == s {
                    closed = true;
                    break;
                }
                chain.push(n);
                cur = n;
            }
            if closed {
                result.extend(simple_loops(chain).into_iter().map(|l| (l, true)));
            } else if chain.len() >= 2 {
                result.push((chain, false));
            }
        }
    }
    result
}

/// A closed loop of vertices cut where it comes through a vertex twice, loops of fewer than
/// three dropped.
fn simple_loops(l: Vec<u32>) -> Vec<Vec<u32>> {
    for i in 0..l.len() {
        if let Some(j) = (i + 1..l.len()).find(|&j| l[j] == l[i]) {
            let a = l[i..j].to_vec();
            let b = l[j..].iter().chain(&l[..i]).copied().collect();
            return simple_loops(a).into_iter().chain(simple_loops(b)).collect();
        }
    }
    if l.len() >= 3 {
        vec![l]
    } else {
        Vec::new()
    }
}

/// A face's corners from corner `i` round to corner `j`, both included.
fn sub_polygon(f: &Face, i: usize, j: usize) -> Face {
    let n = f.verts.len();
    let mut verts = Vec::new();
    let mut uvs = Vec::new();
    let mut k = i;
    loop {
        verts.push(f.verts[k]);
        uvs.push(f.uvs[k]);
        if k == j {
            break;
        }
        k = (k + 1) % n;
    }
    face_like(verts, uvs, f)
}

/// A face cut along chords between its corners (in turn, each through the piece holding
/// both ends; chords along an edge or across no piece are skipped).
fn split_by_chords(face: &Face, chords: &[(u32, u32)]) -> Vec<Face> {
    let mut pieces = vec![face.clone()];
    for &(u, w) in chords {
        for k in 0..pieces.len() {
            let p = &pieces[k];
            let n = p.verts.len();
            let (Some(i), Some(j)) = (p.verts.iter().position(|&x| x == u), p.verts.iter().position(|&x| x == w)) else { continue };
            if i == j || (i + 1) % n == j || (j + 1) % n == i {
                continue;
            }
            let (a, b) = (sub_polygon(p, i, j), sub_polygon(p, j, i));
            pieces[k] = a;
            pieces.push(b);
            break;
        }
    }
    pieces
}

/// Where along the segment a→b point p lies (0 at a, 1 at b).
fn param(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let d = b - a;
    let l = d.length_squared();
    if l < 1e-20 {
        0.0
    } else {
        ((p - a).dot(d) / l).clamp(0.0, 1.0)
    }
}

/// New vertices on edges (keyed by [`edge_key`], listed from the key's first vertex): the
/// face with them put in as corners, UVs along its edges.
fn with_edge_points(positions: &[Vec3], face: &Face, split: &HashMap<(u32, u32), Vec<u32>>) -> Face {
    let n = face.verts.len();
    let mut verts = Vec::with_capacity(n);
    let mut uvs = Vec::with_capacity(n);
    for i in 0..n {
        let (a, b) = (face.verts[i], face.verts[(i + 1) % n]);
        verts.push(a);
        uvs.push(face.uvs[i]);
        let k = edge_key(a, b);
        if let Some(pts) = split.get(&k) {
            let (pa, pb) = (positions[a as usize], positions[b as usize]);
            let ordered: Vec<u32> = if k.0 == a { pts.clone() } else { pts.iter().rev().copied().collect() };
            for p in ordered {
                let t = param(positions[p as usize], pa, pb);
                verts.push(p);
                uvs.push(face.uvs[i].lerp(face.uvs[(i + 1) % n], t));
            }
        }
    }
    face_like(verts, uvs, face)
}

/// The points on a split edge from `a` to `b`.
fn along(split: &HashMap<(u32, u32), Vec<u32>>, a: u32, b: u32) -> Vec<u32> {
    let k = edge_key(a, b);
    let pts = split.get(&k).cloned().unwrap_or_default();
    if k.0 == a {
        pts
    } else {
        pts.into_iter().rev().collect()
    }
}

/// Seams and sharp edges on split edges carried to their pieces.
fn split_marks(mesh: &mut Mesh, split: &HashMap<(u32, u32), Vec<u32>>) {
    for (&(a, b), pts) in split {
        let chain: Vec<u32> = std::iter::once(a).chain(pts.iter().copied()).chain(std::iter::once(b)).collect();
        for (marked, list) in [(mesh.is_seam(a, b), 0), (mesh.is_sharp(a, b), 1)] {
            if !marked {
                continue;
            }
            for w in chain.windows(2) {
                if list == 0 {
                    mesh.set_seam(w[0], w[1], true);
                } else {
                    mesh.set_sharp(w[0], w[1], true);
                }
            }
            if list == 0 {
                mesh.set_seam(a, b, false);
            } else {
                mesh.set_sharp(a, b, false);
            }
        }
    }
}

/// Everything joined to the selection through faces.
pub fn select_linked(mesh: &Mesh, sel: &Selection) -> Selection {
    let vf = mesh.vertex_faces();
    let mut seen = vec![false; mesh.positions.len()];
    let mut stack: Vec<u32> = sel_verts(mesh, sel).into_iter().collect();
    for &v in &stack {
        seen[v as usize] = true;
    }
    while let Some(v) = stack.pop() {
        for &f in &vf[v as usize] {
            for &w in &mesh.faces[f].verts {
                if !seen[w as usize] {
                    seen[w as usize] = true;
                    stack.push(w);
                }
            }
        }
    }
    Selection::from_verts(mesh, (0..mesh.positions.len() as u32).filter(|&v| seen[v as usize]))
}

/// The selection grown by the faces touching it.
pub fn select_more(mesh: &Mesh, sel: &Selection) -> Selection {
    let verts = sel_verts(mesh, sel);
    let mut out = verts.clone();
    for f in &mesh.faces {
        if f.verts.iter().any(|v| verts.contains(v)) {
            out.extend(f.verts.iter().copied());
        }
    }
    Selection::from_verts(mesh, out)
}

/// The selection shrunk: vertices next to an unselected one let go.
pub fn select_less(mesh: &Mesh, sel: &Selection) -> Selection {
    let verts = sel_verts(mesh, sel);
    let vf = mesh.vertex_faces();
    let keep = verts.iter().copied().filter(|&v| {
        let faces = &vf[v as usize];
        !faces.is_empty() && faces.iter().all(|&f| mesh.faces[f].verts.iter().all(|w| verts.contains(w)))
    });
    Selection::from_verts(mesh, keep.collect::<Vec<_>>())
}

/// Everything not selected, in the given mode's terms.
pub fn invert(mesh: &Mesh, sel: &Selection, mode: SelectMode) -> Selection {
    match mode {
        SelectMode::Vertex => {
            let verts = sel_verts(mesh, sel);
            Selection::from_verts(mesh, (0..mesh.positions.len() as u32).filter(|v| !verts.contains(v)))
        }
        SelectMode::Edge => {
            let edges = sel_edges(mesh, sel);
            Selection::from_edges(mesh, mesh.edges().into_iter().filter(|e| !edges.contains(e)))
        }
        SelectMode::Face => {
            let faces = sel_faces(mesh, sel);
            Selection::from_faces(mesh, (0..mesh.faces.len()).filter(|f| !faces.contains(f)))
        }
    }
}

/// The edge loop through an edge (Blender's Alt+click): on through vertices of four edges
/// between four faces, to the edge across from the one it came by; it stops at other
/// vertices. A boundary edge's loop follows the boundary.
pub fn edge_loop(mesh: &Mesh, edge: (u32, u32)) -> Selection {
    let ef = mesh.edge_faces();
    let start = edge_key(edge.0, edge.1);
    let Some(start_faces) = ef.get(&start) else { return Selection::default() };
    let boundary = start_faces.len() == 1;
    let nb = neighbours(mesh);
    let faces_of = |a: u32, b: u32| ef.get(&edge_key(a, b)).cloned().unwrap_or_default();
    let step = |prev: u32, cur: u32| -> Option<u32> {
        let around = &nb[cur as usize];
        if boundary {
            let mut it = around.iter().copied().filter(|&w| w != prev && faces_of(cur, w).len() == 1);
            let next = it.next()?;
            return it.next().is_none().then_some(next);
        }
        if around.len() != 4 || around.iter().any(|&w| faces_of(cur, w).len() != 2) {
            return None;
        }
        let here = faces_of(prev, cur);
        let mut it = around.iter().copied().filter(|&w| w != prev && faces_of(cur, w).iter().all(|f| !here.contains(f)));
        let next = it.next()?;
        it.next().is_none().then_some(next)
    };
    let mut seen: HashSet<(u32, u32)> = HashSet::from([start]);
    let mut out = vec![start];
    for (from, at) in [(start.0, start.1), (start.1, start.0)] {
        let (mut prev, mut cur) = (from, at);
        let mut closed = false;
        while let Some(n) = step(prev, cur) {
            let k = edge_key(cur, n);
            if !seen.insert(k) {
                closed = k == start;
                break;
            }
            out.push(k);
            prev = cur;
            cur = n;
        }
        if closed {
            break;
        }
    }
    Selection::from_edges(mesh, out)
}

/// An edge ring: edges across a strip of quads, each one's next across the quad from it,
/// in order (first vertices on one side), with the quads between them.
struct Ring {
    edges: Vec<(u32, u32)>,
    faces: Vec<usize>,
}

fn ring(mesh: &Mesh, ef: &HashMap<(u32, u32), Vec<usize>>, edge: (u32, u32)) -> Option<Ring> {
    let start = edge;
    let first = ef.get(&edge_key(edge.0, edge.1))?;
    let walk = |f0: usize| -> (Vec<(u32, u32)>, Vec<usize>, bool) {
        let (mut edges, mut faces) = (Vec::new(), Vec::new());
        let (mut e, mut f) = (start, f0);
        loop {
            let v = &mesh.faces[f].verts;
            if v.len() != 4 {
                break;
            }
            let Some(i) = (0..4).find(|&i| edge_key(v[i], v[(i + 1) % 4]) == edge_key(e.0, e.1)) else { break };
            let x = |k: usize| v[(i + k) % 4];
            let next = if e.0 == x(0) { (x(3), x(2)) } else { (x(2), x(3)) };
            faces.push(f);
            if edge_key(next.0, next.1) == edge_key(start.0, start.1) {
                return (edges, faces, true);
            }
            edges.push(next);
            let Some(list) = ef.get(&edge_key(next.0, next.1)) else { break };
            let Some(&g) = list.iter().find(|&&g| g != f) else { break };
            if list.len() != 2 || faces.contains(&g) {
                break;
            }
            e = next;
            f = g;
        }
        (edges, faces, false)
    };
    let (fwd_edges, fwd_faces, closed) = walk(first[0]);
    if closed {
        let mut edges = vec![start];
        edges.extend(fwd_edges);
        return Some(Ring { edges, faces: fwd_faces });
    }
    let (mut edges, mut faces) = (Vec::new(), Vec::new());
    if first.len() == 2 {
        let (back_edges, back_faces, _) = walk(first[1]);
        edges.extend(back_edges.into_iter().rev());
        faces.extend(back_faces.into_iter().rev());
    }
    edges.push(start);
    edges.extend(fwd_edges);
    faces.extend(fwd_faces);
    Some(Ring { edges, faces })
}

/// The edge ring through an edge (Blender's Ctrl+Alt+click): edges across quads.
pub fn edge_ring(mesh: &Mesh, edge: (u32, u32)) -> Selection {
    let ef = mesh.edge_faces();
    match ring(mesh, &ef, edge) {
        Some(r) => Selection::from_edges(mesh, r.edges),
        None => Selection::default(),
    }
}

/// Faces facing within `angle_deg` of a selected face.
pub fn select_similar_normal(mesh: &Mesh, sel: &Selection, angle_deg: f32) -> Selection {
    let picked: Vec<Vec3> = sel_faces(mesh, sel).into_iter().map(|f| mesh.face_normal(f)).collect();
    let cos = angle_deg.to_radians().cos();
    Selection::from_faces(
        mesh,
        (0..mesh.faces.len()).filter(|&f| {
            let n = mesh.face_normal(f);
            picked.iter().any(|p| p.dot(n) >= cos - 1e-6)
        }),
    )
}

/// The faces of one material.
pub fn select_by_material(mesh: &Mesh, material: u32) -> Selection {
    Selection::from_faces(mesh, (0..mesh.faces.len()).filter(|&f| mesh.faces[f].material == material))
}

/// The shortest way along edges from one vertex to another (just the two when there is none).
pub fn shortest_path(mesh: &Mesh, from_vert: u32, to_vert: u32) -> Selection {
    let n = mesh.positions.len();
    if from_vert as usize >= n || to_vert as usize >= n {
        return Selection::default();
    }
    let nb = neighbours(mesh);
    let mut dist = vec![f32::INFINITY; n];
    let mut prev = vec![u32::MAX; n];
    let mut heap = BinaryHeap::new();
    dist[from_vert as usize] = 0.0;
    heap.push(std::cmp::Reverse((0u32, from_vert)));
    while let Some(std::cmp::Reverse((bits, v))) = heap.pop() {
        let d = f32::from_bits(bits);
        if d > dist[v as usize] {
            continue;
        }
        if v == to_vert {
            break;
        }
        for &w in &nb[v as usize] {
            let nd = d + mesh.positions[v as usize].distance(mesh.positions[w as usize]);
            if nd < dist[w as usize] {
                dist[w as usize] = nd;
                prev[w as usize] = v;
                heap.push(std::cmp::Reverse((nd.to_bits(), w)));
            }
        }
    }
    if !dist[to_vert as usize].is_finite() {
        let mut s = Selection::default();
        s.verts.extend([from_vert, to_vert]);
        return s;
    }
    let mut edges = Vec::new();
    let mut v = to_vert;
    while v != from_vert {
        let p = prev[v as usize];
        edges.push((p, v));
        v = p;
    }
    let mut s = Selection::from_edges(mesh, edges);
    s.verts.insert(from_vert);
    s
}

/// The boundary of a set of faces: its edges with one face of the set, as that face runs
/// them, with the face; and whether any of them has a face outside the set too.
fn region_boundary(mesh: &Mesh, faces: &BTreeSet<usize>) -> (Vec<(u32, u32, usize)>, bool) {
    let ef = mesh.edge_faces();
    let mut out = Vec::new();
    let mut attached = false;
    for &f in faces {
        for (a, b) in mesh.faces[f].edges() {
            let list = ef.get(&edge_key(a, b)).map(|l| l.as_slice()).unwrap_or(&[]);
            if list.iter().filter(|g| faces.contains(g)).count() == 1 {
                attached |= list.len() > 1;
                out.push((a, b, f));
            }
        }
    }
    (out, attached)
}

/// The area-weighted normal of some faces (up when they have none).
fn faces_normal(mesh: &Mesh, faces: &BTreeSet<usize>) -> Vec3 {
    let n: Vec3 = faces.iter().map(|&f| mesh.face_normal(f) * mesh.face_area(f)).sum();
    let n = n.normalize_or_zero();
    if n == Vec3::ZERO {
        Vec3::Z
    } else {
        n
    }
}

/// UVs of a new side wall over an edge of length `len`, as if it stood a metre tall.
fn wall_uvs(len: f32) -> Vec<Vec2> {
    vec![Vec2::new(0.0, 1.0), Vec2::new(len, 1.0), Vec2::new(len, 0.0), Vec2::ZERO]
}

/// The pieces a set of faces makes, faces joined across shared edges.
fn edge_pieces(mesh: &Mesh, faces: &BTreeSet<usize>) -> Vec<BTreeSet<usize>> {
    let ef = mesh.edge_faces();
    let mut seen: HashSet<usize> = HashSet::new();
    let mut pieces = Vec::new();
    for &f in faces {
        if !seen.insert(f) {
            continue;
        }
        let mut piece = BTreeSet::from([f]);
        let mut stack = vec![f];
        while let Some(g) = stack.pop() {
            for (a, b) in mesh.faces[g].edges() {
                for &h in ef.get(&edge_key(a, b)).map(|l| l.as_slice()).unwrap_or(&[]) {
                    if faces.contains(&h) && seen.insert(h) {
                        piece.insert(h);
                        stack.push(h);
                    }
                }
            }
        }
        pieces.push(piece);
    }
    pieces
}

/// What an extrusion made: the cap faces (of a region), the side walls, the new vertices and
/// the new edges along the top (of an edge extrusion).
struct Extruded {
    caps: Vec<usize>,
    walls: Vec<usize>,
    made: Vec<u32>,
    top: Vec<(u32, u32)>,
}

/// The corners of a set of faces grouped round each vertex: corners of faces joined across
/// edges inside the set (at that vertex) share a group. Each (face, vertex) corner's group,
/// and each group's vertex.
fn corner_groups(mesh: &Mesh, faces: &BTreeSet<usize>) -> (HashMap<(usize, u32), usize>, Vec<u32>) {
    let mut id: HashMap<(usize, u32), usize> = HashMap::new();
    let mut parent: Vec<usize> = Vec::new();
    let mut vert: Vec<u32> = Vec::new();
    for &f in faces {
        for &v in &mesh.faces[f].verts {
            id.entry((f, v)).or_insert_with(|| {
                parent.push(parent.len());
                vert.push(v);
                parent.len() - 1
            });
        }
    }
    fn root(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut y = x;
        while parent[y] != r {
            let next = parent[y];
            parent[y] = r;
            y = next;
        }
        r
    }
    let ef = mesh.edge_faces();
    for &f in faces {
        for (a, b) in mesh.faces[f].edges() {
            for &g in ef.get(&edge_key(a, b)).map(|l| l.as_slice()).unwrap_or(&[]) {
                if g != f && faces.contains(&g) {
                    for v in [a, b] {
                        let (x, y) = (root(&mut parent, id[&(f, v)]), root(&mut parent, id[&(g, v)]));
                        parent[x.max(y)] = x.min(y);
                    }
                }
            }
        }
    }
    let mut dense: HashMap<usize, usize> = HashMap::new();
    let mut verts = Vec::new();
    let mut group = HashMap::new();
    let mut corners: Vec<((usize, u32), usize)> = id.into_iter().collect();
    corners.sort_unstable();
    for (corner, i) in corners {
        let r = root(&mut parent, i);
        let g = *dense.entry(r).or_insert_with(|| {
            verts.push(vert[r]);
            verts.len() - 1
        });
        group.insert(corner, g);
    }
    (group, verts)
}

/// Faces extruded as a region, not moved yet: the faces go on new vertices (the caps) and
/// walls join them to the boundary. A vertex gets a copy for each fan of the faces round it
/// joined by their edges, so faces touching at a corner come apart. The old faces go when the
/// selection is joined to other faces; on its own it keeps them, turned round, as its bottom.
fn extrude_region(mesh: &mut Mesh, faces: &BTreeSet<usize>) -> Extruded {
    let (boundary, attached) = region_boundary(mesh, faces);
    let (group, group_vert) = corner_groups(mesh, faces);
    let made: Vec<u32> = group_vert.iter().map(|&v| mesh.add_vertex(mesh.positions[v as usize])).collect();
    let copy = |f: usize, v: u32| made[group[&(f, v)]];
    for &f in faces {
        for (a, b) in mesh.faces[f].edges().collect::<Vec<_>>() {
            if mesh.is_seam(a, b) {
                mesh.set_seam(copy(f, a), copy(f, b), true);
            }
            if mesh.is_sharp(a, b) {
                mesh.set_sharp(copy(f, a), copy(f, b), true);
            }
        }
    }
    let old = std::mem::take(&mut mesh.faces);
    for (i, f) in old.iter().enumerate() {
        if !faces.contains(&i) {
            mesh.faces.push(f.clone());
        } else if !attached {
            let mut g = f.clone();
            g.reverse();
            mesh.faces.push(g);
        }
    }
    let mut walls = Vec::new();
    for &(a, b, f) in &boundary {
        let len = mesh.positions[a as usize].distance(mesh.positions[b as usize]);
        mesh.faces.push(face_like(vec![a, b, copy(f, b), copy(f, a)], wall_uvs(len), &old[f]));
        walls.push(mesh.faces.len() - 1);
    }
    let mut caps = Vec::new();
    for &f in faces {
        let src = &old[f];
        mesh.faces.push(face_like(src.verts.iter().map(|&v| copy(f, v)).collect(), src.uvs.clone(), src));
        caps.push(mesh.faces.len() - 1);
    }
    Extruded { caps, walls, made, top: Vec::new() }
}

/// Edges pulled out into quads, not moved yet. Each quad runs the edge against the face it
/// had, so the strip agrees with it.
fn extrude_edge_set(mesh: &mut Mesh, edges: &BTreeSet<(u32, u32)>) -> Extruded {
    let dir = directed(mesh);
    let mut copy: BTreeMap<u32, u32> = BTreeMap::new();
    let mut walls = Vec::new();
    let mut top = Vec::new();
    for &(a, b) in edges {
        let (a, b, f) = match (dir.get(&(a, b)), dir.get(&(b, a))) {
            (Some(&f), _) => (a, b, f),
            (None, Some(&f)) => (b, a, f),
            _ => continue,
        };
        let mut copy_of = |v: u32, mesh: &mut Mesh| *copy.entry(v).or_insert_with(|| mesh.add_vertex(mesh.positions[v as usize]));
        let (ca, cb) = (copy_of(a, mesh), copy_of(b, mesh));
        let len = mesh.positions[a as usize].distance(mesh.positions[b as usize]);
        let like = mesh.faces[f].clone();
        mesh.faces.push(face_like(vec![b, a, ca, cb], wall_uvs(len), &like));
        walls.push(mesh.faces.len() - 1);
        top.push((ca, cb));
    }
    Extruded { caps: Vec::new(), walls, made: copy.into_values().collect(), top }
}

/// The edges an edge extrusion pulls out: in vertex mode only boundary edges.
fn extrudable_edges(mesh: &Mesh, sel: &Selection, mode: SelectMode) -> BTreeSet<(u32, u32)> {
    let mut edges = sel_edges(mesh, sel);
    if mode == SelectMode::Vertex {
        let ef = mesh.edge_faces();
        edges.retain(|e| ef.get(e).is_some_and(|l| l.len() == 1));
    }
    edges
}

/// Extrude (E), not moved yet (the caller moves what comes back): selected faces as one
/// region per connected piece, the new caps selected; with no faces, selected edges (in vertex
/// mode, boundary edges between selected vertices) into quads, the new edges selected; lone
/// vertices stay as they are.
pub fn extrude(mesh: &mut Mesh, sel: &Selection, mode: SelectMode) -> Selection {
    let faces = sel_faces(mesh, sel);
    if !faces.is_empty() {
        let e = extrude_region(mesh, &faces);
        compact(mesh);
        return Selection::from_faces(mesh, e.caps);
    }
    if mode == SelectMode::Face {
        return kept(mesh, sel);
    }
    let edges = extrudable_edges(mesh, sel, mode);
    if edges.is_empty() {
        return kept(mesh, sel);
    }
    let e = extrude_edge_set(mesh, &edges);
    Selection::from_edges(mesh, e.top)
}

/// Extrude and move the new part `distance` along the selection's average normal.
pub fn extrude_along_normals(mesh: &mut Mesh, sel: &Selection, distance: f32) -> Selection {
    let faces = sel_faces(mesh, sel);
    if !faces.is_empty() {
        let n = faces_normal(mesh, &faces);
        let e = extrude_region(mesh, &faces);
        for &v in &e.made {
            mesh.positions[v as usize] += n * distance;
        }
        refresh_uvs(mesh, &e.walls);
        compact(mesh);
        return Selection::from_faces(mesh, e.caps);
    }
    let edges = extrudable_edges(mesh, sel, SelectMode::Edge);
    if edges.is_empty() {
        return kept(mesh, sel);
    }
    let n = selection_normal(mesh, sel);
    let e = extrude_edge_set(mesh, &edges);
    for &v in &e.made {
        mesh.positions[v as usize] += n * distance;
    }
    refresh_uvs(mesh, &e.walls);
    Selection::from_edges(mesh, e.top)
}

/// Extrude each selected face on its own, `distance` along its own normal.
pub fn extrude_individual(mesh: &mut Mesh, sel: &Selection, distance: f32) -> Selection {
    let faces = sel_faces(mesh, sel);
    if faces.is_empty() {
        return kept(mesh, sel);
    }
    let ef = mesh.edge_faces();
    let lone = |f: usize| mesh.faces[f].edges().all(|(a, b)| ef.get(&edge_key(a, b)).is_none_or(|l| l.len() == 1));
    let lone: Vec<bool> = (0..mesh.faces.len()).map(|f| faces.contains(&f) && lone(f)).collect();
    let old = std::mem::take(&mut mesh.faces);
    for (i, f) in old.iter().enumerate() {
        if !faces.contains(&i) {
            mesh.faces.push(f.clone());
        } else if lone[i] {
            let mut g = f.clone();
            g.reverse();
            mesh.faces.push(g);
        }
    }
    let (mut caps, mut walls) = (Vec::new(), Vec::new());
    for &f in &faces {
        let src = &old[f];
        let pts: Vec<Vec3> = src.verts.iter().map(|&v| mesh.positions[v as usize]).collect();
        let n = polygon_normal(&pts);
        let copies: Vec<u32> = pts.iter().map(|&p| mesh.add_vertex(p + n * distance)).collect();
        let k = src.verts.len();
        for i in 0..k {
            let j = (i + 1) % k;
            mesh.faces.push(face_like(vec![src.verts[i], src.verts[j], copies[j], copies[i]], wall_uvs(distance.abs()), src));
            walls.push(mesh.faces.len() - 1);
        }
        mesh.faces.push(face_like(copies, src.uvs.clone(), src));
        caps.push(mesh.faces.len() - 1);
    }
    refresh_uvs(mesh, &walls);
    compact(mesh);
    Selection::from_faces(mesh, caps)
}

/// The step in from a corner where two edges meet, given each edge's unit vector pointing
/// into the face: one unit from both edges.
fn miter(i1: Vec3, i2: Vec3) -> Vec3 {
    (i1 + i2) / (1.0 + i1.dot(i2)).max(0.1)
}

/// Inset (I): a ring of faces `thickness` wide inside the selection's boundary (or inside
/// each face's when `individual`), the inner faces raised by `depth`; the inner faces
/// selected after.
pub fn inset(mesh: &mut Mesh, sel: &Selection, thickness: f32, depth: f32, individual: bool) -> Selection {
    let faces = sel_faces(mesh, sel);
    if faces.is_empty() {
        return kept(mesh, sel);
    }
    if individual {
        for &f in &faces {
            let t = thickness.min(inset_limit(mesh, &BTreeSet::from([f])));
            inset_face(mesh, f, t, depth);
        }
    } else {
        let t = thickness.min(inset_limit(mesh, &faces));
        inset_region(mesh, &faces, t, depth);
    }
    Selection::from_faces(mesh, faces)
}

/// How far in a ring may go before it turns the faces inside out: just short of half the
/// narrower way across them, seen from the way they face.
fn inset_limit(mesh: &Mesh, faces: &BTreeSet<usize>) -> f32 {
    let n = faces_normal(mesh, faces).normalize_or_zero();
    if n == Vec3::ZERO {
        return f32::MAX;
    }
    let a = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let (u, v) = (n.cross(a).normalize(), n.cross(n.cross(a)).normalize());
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for &f in faces {
        for &i in &mesh.faces[f].verts {
            let p = mesh.positions[i as usize];
            let q = Vec2::new(p.dot(u), p.dot(v));
            lo = lo.min(q);
            hi = hi.max(q);
        }
    }
    let across = (hi - lo).min_element();
    if across.is_finite() && across > 1e-6 {
        0.49 * across
    } else {
        f32::MAX
    }
}

fn inset_region(mesh: &mut Mesh, faces: &BTreeSet<usize>, thickness: f32, depth: f32) {
    let (boundary, _) = region_boundary(mesh, faces);
    if boundary.is_empty() {
        return;
    }
    let (group, group_vert) = corner_groups(mesh, faces);
    let pos = mesh.positions.clone();
    // each fan of the region round a vertex steps in from its own two boundary edges
    let mut inward: Vec<Vec<Vec3>> = vec![Vec::new(); group_vert.len()];
    for &(a, b, f) in &boundary {
        let i = mesh.face_normal(f).cross(pos[b as usize] - pos[a as usize]).normalize_or_zero();
        inward[group[&(f, a)]].push(i);
        inward[group[&(f, b)]].push(i);
    }
    let mut normal = vec![Vec3::ZERO; group_vert.len()];
    for &f in faces {
        let n = mesh.face_normal(f);
        for &v in &mesh.faces[f].verts {
            normal[group[&(f, v)]] += n;
        }
    }
    let at: Vec<u32> = group_vert
        .iter()
        .zip(&inward)
        .map(|(&v, dirs)| match dirs.len() {
            0 => v,
            2 => mesh.add_vertex(pos[v as usize] + miter(dirs[0], dirs[1]) * thickness),
            _ => mesh.add_vertex(pos[v as usize] + dirs.iter().sum::<Vec3>().normalize_or_zero() * thickness),
        })
        .collect();
    let corner_at = |f: usize, v: u32| at[group[&(f, v)]];
    let mut ring = Vec::new();
    for &(a, b, f) in &boundary {
        let src = &mesh.faces[f];
        let corner = |v: u32| src.verts.iter().position(|&x| x == v).unwrap_or(0);
        let (ca, cb) = (corner_at(f, a), corner_at(f, b));
        let uvs = vec![src.uvs[corner(a)], src.uvs[corner(b)], uv_at(mesh, f, mesh.positions[cb as usize]), uv_at(mesh, f, mesh.positions[ca as usize])];
        ring.push(face_like(vec![a, b, cb, ca], uvs, src));
    }
    for &f in faces {
        let src = mesh.faces[f].clone();
        let verts: Vec<u32> = src.verts.iter().map(|&v| corner_at(f, v)).collect();
        let uvs = src.verts.iter().zip(&verts).zip(&src.uvs).map(|((&v, &w), &uv)| if v == w { uv } else { uv_at(mesh, f, mesh.positions[w as usize]) }).collect();
        mesh.faces[f] = face_like(verts, uvs, &src);
    }
    mesh.faces.extend(ring);
    for (g, n) in normal.iter().enumerate() {
        mesh.positions[at[g] as usize] += n.normalize_or_zero() * depth;
    }
}

fn inset_face(mesh: &mut Mesh, f: usize, thickness: f32, depth: f32) {
    let src = mesh.faces[f].clone();
    let n = mesh.face_normal(f);
    let k = src.verts.len();
    let p = |i: usize| mesh.positions[src.verts[i % k] as usize];
    let inner: Vec<Vec3> = (0..k)
        .map(|i| {
            let i1 = n.cross(p(i) - p(i + k - 1)).normalize_or_zero();
            let i2 = n.cross(p(i + 1) - p(i)).normalize_or_zero();
            p(i) + miter(i1, i2) * thickness
        })
        .collect();
    let uvs: Vec<Vec2> = inner.iter().map(|&q| uv_at(mesh, f, q)).collect();
    let copies: Vec<u32> = inner.iter().map(|&q| mesh.add_vertex(q + n * depth)).collect();
    for i in 0..k {
        let j = (i + 1) % k;
        mesh.faces.push(face_like(vec![src.verts[i], src.verts[j], copies[j], copies[i]], vec![src.uvs[i], src.uvs[j], uvs[j], uvs[i]], &src));
    }
    mesh.faces[f] = face_like(copies, uvs, &src);
}

/// What a bevel builds before the faces are put together.
#[derive(Default)]
struct Cut {
    /// The new corners standing in for a face's corner (face, corner) at a bevelled vertex.
    repl: HashMap<(usize, usize), Vec<u32>>,
    /// Where an edge (vertex, other end) now ends at that vertex.
    ends: HashMap<(u32, u32), u32>,
    /// The new faces: strips along edges and patches at corners.
    extra: Vec<Face>,
}

/// A face without corners repeating the one before.
fn dedup_corners(f: Face) -> Face {
    let n = f.verts.len();
    let mut verts = Vec::with_capacity(n);
    let mut uvs = Vec::with_capacity(n);
    for i in 0..n {
        if f.verts[i] != f.verts[(i + 1) % n] {
            verts.push(f.verts[i]);
            uvs.push(f.uvs[i]);
        }
    }
    face_like(verts, uvs, &f)
}

/// Put the faces of a bevel together: `orig`'s faces with their corners replaced, then the
/// new faces (selected after); marks follow edges to their new ends, those on `gone` edges go.
fn finish_cut(mesh: &mut Mesh, orig: &Mesh, cut: Cut, gone: &BTreeSet<(u32, u32)>) -> Selection {
    let mut faces = Vec::with_capacity(orig.faces.len() + cut.extra.len());
    for (fi, f) in orig.faces.iter().enumerate() {
        let (mut verts, mut uvs) = (Vec::new(), Vec::new());
        for (ci, &v) in f.verts.iter().enumerate() {
            match cut.repl.get(&(fi, ci)) {
                Some(list) => {
                    for &w in list {
                        verts.push(w);
                        uvs.push(if w == v { f.uvs[ci] } else { uv_at(orig, fi, mesh.positions[w as usize]) });
                    }
                }
                None => {
                    verts.push(v);
                    uvs.push(f.uvs[ci]);
                }
            }
        }
        let g = dedup_corners(face_like(verts, uvs, f));
        if g.verts.len() >= 3 {
            faces.push(g);
        }
    }
    let first = faces.len();
    // strips and patches squeezed to nothing (where a bevel meets itself) go
    faces.extend(cut.extra.into_iter().map(dedup_corners).filter(|f| f.verts.len() >= 3 && f.verts.iter().collect::<HashSet<_>>().len() == f.verts.len()));
    let end = |a: u32, b: u32| cut.ends.get(&(a, b)).copied().unwrap_or(a);
    let carry = |list: &[(u32, u32)]| {
        let mut out: Vec<(u32, u32)> = list.iter().filter(|e| !gone.contains(e)).map(|&(a, b)| (end(a, b), end(b, a))).filter(|(a, b)| a != b).map(|(a, b)| edge_key(a, b)).collect();
        out.sort_unstable();
        out.dedup();
        out
    };
    mesh.seams = carry(&orig.seams);
    mesh.sharp = carry(&orig.sharp);
    let n = faces.len();
    mesh.faces = faces;
    compact(mesh);
    Selection::from_faces(mesh, first..n)
}

/// How far along an edge (unit direction `along`) a point stands `w` off another edge
/// (direction `from`) of the same corner.
fn slide_dist(along: Vec3, from: Vec3, w: f32) -> f32 {
    let c = along.dot(from).clamp(-1.0, 1.0);
    w / (1.0 - c * c).sqrt().max(0.25)
}

/// New points on a rounded profile from `l` to `r` (both included): a quarter of an ellipse
/// touching the two faces, round the corner on the edge through `p` along `d`.
fn arc(mesh: &mut Mesh, l: u32, r: u32, p: Vec3, d: Vec3, segs: usize) -> Vec<u32> {
    let (pl, pr) = (mesh.positions[l as usize], mesh.positions[r as usize]);
    let c = p + d * ((pl + pr) * 0.5 - p).dot(d);
    let o = pl + pr - c;
    let mut out = vec![l];
    for k in 1..segs {
        let a = k as f32 / segs as f32 * std::f32::consts::FRAC_PI_2;
        out.push(mesh.add_vertex(o + (pl - o) * a.cos() + (pr - o) * a.sin()));
    }
    out.push(r);
    out
}

/// Bevel (Ctrl+B): selected edges between two faces cut back `width` on both sides and
/// rounded with `segments` faces across, corners where bevelled edges meet patched; in vertex
/// mode each selected vertex cut off flat `width` along its edges. The new faces are
/// selected after; edges and vertices that cannot be bevelled (on no closed fan of faces) are
/// left alone.
pub fn bevel(mesh: &mut Mesh, sel: &Selection, mode: SelectMode, width: f32, segments: u32) -> Selection {
    if !width.is_finite() || width <= 0.0 {
        return kept(mesh, sel);
    }
    if mode == SelectMode::Vertex {
        return bevel_vertices(mesh, sel, width);
    }
    let orig = mesh.clone();
    let dir = directed(&orig);
    let vf = orig.vertex_faces();
    let ef = orig.edge_faces();
    let mut bev: BTreeSet<(u32, u32)> =
        sel_edges(&orig, sel).into_iter().filter(|&(a, b)| ef.get(&(a, b)).is_some_and(|l| l.len() == 2) && dir.contains_key(&(a, b)) && dir.contains_key(&(b, a))).collect();
    let mut fans = BTreeMap::new();
    let ends: BTreeSet<u32> = bev.iter().flat_map(|&(a, b)| [a, b]).collect();
    for v in ends {
        match fan(&orig, &dir, &vf, v) {
            Some(f) => {
                fans.insert(v, f);
            }
            None => bev.retain(|&(a, b)| a != v && b != v),
        }
    }
    fans.retain(|v, _| bev.iter().any(|&(a, b)| a == *v || b == *v));
    if bev.is_empty() {
        return kept(mesh, sel);
    }
    let pos = &orig.positions;
    let mut w = width;
    for (&v, fan) in &fans {
        for o in fan.others(&orig) {
            w = w.min(0.5 * pos[v as usize].distance(pos[o as usize]));
        }
    }
    if w <= 1e-7 {
        return kept(mesh, sel);
    }
    let segs = segments.clamp(1, 64) as usize;
    let mut cut = Cut::default();
    let mut profiles: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (&v, fan) in &fans {
        bevel_corner(mesh, &orig, v, fan, &bev, w, segs, &mut cut, &mut profiles);
    }
    for &(a, b) in &bev {
        let (Some(pa), Some(pb)) = (profiles.get(&(a, b)), profiles.get(&(b, a))) else { continue };
        let pb: Vec<u32> = pb.iter().rev().copied().collect();
        let like = &orig.faces[dir[&(a, b)]];
        let len = pos[a as usize].distance(pos[b as usize]);
        let mut s = 0.0;
        for k in 0..segs {
            let step = mesh.positions[pa[k] as usize].distance(mesh.positions[pa[k + 1] as usize]);
            let uvs = vec![Vec2::new(len, s), Vec2::new(0.0, s), Vec2::new(0.0, s + step), Vec2::new(len, s + step)];
            cut.extra.push(face_like(vec![pb[k], pa[k], pa[k + 1], pb[k + 1]], uvs, like));
            s += step;
        }
    }
    // (a rounded bevel shades smooth: the strips and patches round one another, as a round bar does)
    if segs > 1 {
        for f in &mut cut.extra {
            f.smooth = true;
        }
    }
    finish_cut(mesh, &orig, cut, &bev)
}

/// One vertex of an edge bevel: the new corners of the faces round it, the profiles of its
/// bevelled edges (from the face an edge leaves the vertex in to the one it comes in by) and
/// the patch over what is left of the corner.
#[allow(clippy::too_many_arguments)]
fn bevel_corner(mesh: &mut Mesh, orig: &Mesh, v: u32, fan: &Fan, bev: &BTreeSet<(u32, u32)>, w: f32, segs: usize, cut: &mut Cut, profiles: &mut HashMap<(u32, u32), Vec<u32>>) {
    let pos = &orig.positions;
    let pv = pos[v as usize];
    let others = fan.others(orig);
    let (n, m) = (fan.corners.len(), others.len());
    let is_bev = |j: usize| bev.contains(&edge_key(v, others[j]));
    let dir_of = |j: usize| (pos[others[j] as usize] - pv).normalize_or_zero();
    // edge j comes into face j and leaves face j - 1
    let face_in = |j: usize| (j < n).then_some(j);
    let face_out = |j: usize| {
        if j > 0 {
            Some(j - 1)
        } else if fan.closed {
            Some(n - 1)
        } else {
            None
        }
    };
    // each edge not bevelled ends at a point slid along it to stand `w` off the bevelled edge
    // beside it (staying at v with none beside it)
    let mut slide = vec![v; m];
    for j in 0..m {
        if is_bev(j) {
            continue;
        }
        let mut dists = Vec::new();
        if let Some(i) = face_in(j) {
            let o = fan.out_edge(i);
            if o < m && is_bev(o) {
                dists.push(slide_dist(dir_of(j), dir_of(o), w));
            }
        }
        if let Some(i) = face_out(j) {
            if is_bev(i) {
                dists.push(slide_dist(dir_of(j), dir_of(i), w));
            }
        }
        if dists.is_empty() {
            continue;
        }
        let len = pv.distance(pos[others[j] as usize]);
        let d = (dists.iter().sum::<f32>() / dists.len() as f32).min(0.5 * len);
        slide[j] = mesh.add_vertex(pv + dir_of(j) * d);
        cut.ends.insert((v, others[j]), slide[j]);
    }
    // each face's corner: between two bevelled edges a point `w` off both, else the slid points
    for (i, &(f, c)) in fan.corners.iter().enumerate() {
        let (ei, eo) = (i, fan.out_edge(i));
        let list = match (is_bev(ei), is_bev(eo)) {
            (true, true) => {
                let nrm = orig.face_normal(f);
                let d1 = (pv - pos[others[ei] as usize]).normalize_or_zero();
                let p = pv + miter(nrm.cross(d1).normalize_or_zero(), nrm.cross(dir_of(eo)).normalize_or_zero()) * w;
                vec![mesh.add_vertex(p)]
            }
            (true, false) => vec![slide[eo]],
            (false, true) => vec![slide[ei]],
            (false, false) if slide[ei] == slide[eo] => vec![slide[ei]],
            (false, false) => vec![slide[ei], slide[eo]],
        };
        cut.repl.insert((f, c), list);
    }
    let bevelled: Vec<usize> = (0..m).filter(|&j| is_bev(j)).collect();
    for &j in &bevelled {
        let (Some(fo), Some(fi)) = (face_out(j), face_in(j)) else { continue };
        let l = cut.repl[&fan.corners[fo]].last().copied().unwrap_or(v);
        let r = cut.repl[&fan.corners[fi]].first().copied().unwrap_or(v);
        // two bevelled edges going straight through v share one arc
        let shared = bevelled.iter().filter(|&&k| k != j).find_map(|&k| profiles.get(&(v, others[k])).filter(|p| p.first() == Some(&r) && p.last() == Some(&l)).cloned());
        let list = match shared {
            Some(p) if bevelled.len() == 2 => p.into_iter().rev().collect(),
            _ => arc(mesh, l, r, pv, dir_of(j), segs),
        };
        profiles.insert((v, others[j]), list);
    }
    if !fan.closed {
        return;
    }
    // one bevelled edge at a vertex of three: its arc goes into the face across from it
    if let [j] = bevelled[..] {
        if n == 3 {
            let p = &profiles[&(v, others[j])];
            cut.repl.insert(fan.corners[(j + 1) % 3], p.iter().rev().copied().collect());
            return;
        }
    }
    // the hole left at v: each face's new corners in turn with the arcs between them
    let mut ring: Vec<u32> = Vec::new();
    for i in 0..n {
        ring.extend(&cut.repl[&fan.corners[i]]);
        let eo = fan.out_edge(i);
        if is_bev(eo) {
            if let Some(p) = profiles.get(&(v, others[eo])) {
                ring.extend(&p[1..p.len() - 1]);
            }
        }
    }
    let ring = dedup_corners(Face { verts: ring.clone(), uvs: vec![Vec2::ZERO; ring.len()], ..Default::default() }).verts;
    // where v itself stays (edges with no bevelled edge beside them), it splits the hole in pieces
    let pieces: Vec<Vec<u32>> = match ring.iter().position(|&x| x == v) {
        Some(at) if ring.iter().filter(|&&x| x == v).count() > 1 => {
            let rotated: Vec<u32> = ring[at..].iter().chain(&ring[..at]).copied().collect();
            let mut out: Vec<Vec<u32>> = Vec::new();
            for x in rotated {
                if x == v {
                    out.push(Vec::new());
                }
                if let Some(last) = out.last_mut() {
                    last.push(x);
                }
            }
            out
        }
        _ => vec![ring],
    };
    let like = &orig.faces[fan.corners[0].0];
    for mut p in pieces {
        let distinct: HashSet<u32> = p.iter().copied().collect();
        if p.len() >= 3 && distinct.len() == p.len() {
            p.reverse();
            cut.extra.push(planar_face(mesh, p, like));
        }
    }
}

fn bevel_vertices(mesh: &mut Mesh, sel: &Selection, width: f32) -> Selection {
    let orig = mesh.clone();
    let dir = directed(&orig);
    let vf = orig.vertex_faces();
    let mut cut = Cut::default();
    for v in sel_verts(&orig, sel) {
        let Some(fan) = fan(&orig, &dir, &vf, v) else { continue };
        let pv = orig.positions[v as usize];
        let pts: Vec<u32> = fan
            .others(&orig)
            .into_iter()
            .map(|o| {
                let d = orig.positions[o as usize] - pv;
                let p = mesh.add_vertex(pv + d.normalize_or_zero() * width.min(0.45 * d.length()));
                cut.ends.insert((v, o), p);
                p
            })
            .collect();
        for (i, &(f, c)) in fan.corners.iter().enumerate() {
            cut.repl.insert((f, c), vec![pts[i], pts[fan.out_edge(i)]]);
        }
        if fan.closed && pts.len() >= 3 {
            let verts: Vec<u32> = pts.iter().rev().copied().collect();
            cut.extra.push(planar_face(mesh, verts, &orig.faces[fan.corners[0].0]));
        }
    }
    finish_cut(mesh, &orig, cut, &BTreeSet::new())
}

/// Loop cut (Ctrl+R): `cuts` new edge loops across the ring of quads through `edge`, evenly
/// spaced; a single cut slides by `factor` (-1..1) toward either side. Faces where the ring
/// stops take the new vertices as extra corners. The new edges are selected after.
pub fn loop_cut(mesh: &mut Mesh, edge: (u32, u32), cuts: u32, factor: f32) -> Selection {
    let ef = mesh.edge_faces();
    let Some(ring) = ring(mesh, &ef, edge) else { return Selection::default() };
    if ring.faces.is_empty() {
        return Selection::default();
    }
    let cuts = cuts.clamp(1, 64);
    let ts: Vec<f32> = if cuts == 1 { vec![(0.5 + 0.5 * factor.clamp(-1.0, 1.0)).clamp(0.01, 0.99)] } else { (1..=cuts).map(|i| i as f32 / (cuts + 1) as f32).collect() };
    let mut split: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for &(a, b) in &ring.edges {
        let k = edge_key(a, b);
        if split.contains_key(&k) {
            continue;
        }
        let (pa, pb) = (mesh.positions[a as usize], mesh.positions[b as usize]);
        let mut pts: Vec<u32> = ts.iter().map(|&t| mesh.add_vertex(pa.lerp(pb, t))).collect();
        if k.0 != a {
            pts.reverse();
        }
        split.insert(k, pts);
    }
    // each quad of the ring with the ring edge it is entered by
    let entry: HashMap<usize, (u32, u32)> = ring.faces.iter().enumerate().map(|(k, &f)| (f, ring.edges[k])).collect();
    let mut faces = Vec::with_capacity(mesh.faces.len() + ring.faces.len() * cuts as usize);
    let mut new_edges = Vec::new();
    for (fi, f) in mesh.faces.iter().enumerate() {
        let v = &f.verts;
        let at = entry.get(&fi).and_then(|&(a, b)| (0..v.len()).find(|&i| edge_key(v[i], v[(i + 1) % v.len()]) == edge_key(a, b)));
        let Some(i) = at.filter(|_| v.len() == 4) else {
            let touched = f.edges().any(|(a, b)| split.contains_key(&edge_key(a, b)));
            faces.push(if touched { with_edge_points(&mesh.positions, f, &split) } else { f.clone() });
            continue;
        };
        let x = |k: usize| v[(i + k) % 4];
        let uv = |k: usize| f.uvs[(i + k) % 4];
        let side = |a: usize, b: usize| -> (Vec<u32>, Vec<Vec2>) {
            let mut pts = vec![x(a)];
            pts.extend(along(&split, x(a), x(b)));
            pts.push(x(b));
            let (pa, pb) = (mesh.positions[x(a) as usize], mesh.positions[x(b) as usize]);
            let uvs = pts.iter().map(|&p| uv(a).lerp(uv(b), param(mesh.positions[p as usize], pa, pb))).collect();
            (pts, uvs)
        };
        let ((p, puv), (q, quv)) = (side(0, 1), side(3, 2));
        if p.len() != q.len() {
            faces.push(with_edge_points(&mesh.positions, f, &split));
            continue;
        }
        for j in 0..p.len() - 1 {
            // (a side the ring crosses elsewhere takes its points too)
            let g = face_like(vec![p[j], p[j + 1], q[j + 1], q[j]], vec![puv[j], puv[j + 1], quv[j + 1], quv[j]], f);
            faces.push(with_edge_points(&mesh.positions, &g, &split));
            if j > 0 {
                new_edges.push((p[j], q[j]));
            }
        }
    }
    mesh.faces = faces;
    split_marks(mesh, &split);
    Selection::from_edges(mesh, new_edges)
}

/// Subdivide: each selected quad into a grid of (cuts + 1)², each triangle into (cuts + 1)²
/// triangles, larger polygons into a piece for each corner round a middle vertex. Every edge
/// of theirs is split, so unselected neighbours take the new vertices as extra corners and no
/// cracks open. The new faces are selected after.
pub fn subdivide(mesh: &mut Mesh, sel: &Selection, cuts: u32) -> Selection {
    let faces = sel_faces(mesh, sel);
    if faces.is_empty() {
        return kept(mesh, sel);
    }
    let cuts = cuts.clamp(1, 32) as usize;
    let mut split: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for &f in &faces {
        let edges: Vec<(u32, u32)> = mesh.faces[f].edges().collect();
        for (a, b) in edges {
            let k = edge_key(a, b);
            split.entry(k).or_insert_with(|| {
                let (pa, pb) = (mesh.positions[k.0 as usize], mesh.positions[k.1 as usize]);
                (1..=cuts).map(|i| mesh.add_vertex(pa.lerp(pb, i as f32 / (cuts + 1) as f32))).collect()
            });
        }
    }
    let old = std::mem::take(&mut mesh.faces);
    let mut made = Vec::new();
    for (fi, f) in old.iter().enumerate() {
        if !faces.contains(&fi) {
            let touched = f.edges().any(|(a, b)| split.contains_key(&edge_key(a, b)));
            let g = if touched { with_edge_points(&mesh.positions, f, &split) } else { f.clone() };
            mesh.faces.push(g);
            continue;
        }
        let pieces = match f.verts.len() {
            4 => grid_quad(mesh, f, &split, cuts),
            3 => grid_triangle(mesh, f, &split, cuts),
            _ => fan_polygon(mesh, f, &split, cuts),
        };
        for g in pieces {
            mesh.faces.push(g);
            made.push(mesh.faces.len() - 1);
        }
    }
    split_marks(mesh, &split);
    Selection::from_faces(mesh, made)
}

/// The points of a split edge from `a` to `b`, both ends included.
fn edge_row(split: &HashMap<(u32, u32), Vec<u32>>, a: u32, b: u32) -> Vec<u32> {
    std::iter::once(a).chain(along(split, a, b)).chain(std::iter::once(b)).collect()
}

fn grid_quad(mesh: &mut Mesh, f: &Face, split: &HashMap<(u32, u32), Vec<u32>>, cuts: usize) -> Vec<Face> {
    let x = &f.verts;
    let u = &f.uvs;
    let n = cuts + 1;
    let p = |k: usize, mesh: &Mesh| mesh.positions[x[k] as usize];
    let (bottom, right, top, left) = (edge_row(split, x[0], x[1]), edge_row(split, x[1], x[2]), edge_row(split, x[3], x[2]), edge_row(split, x[0], x[3]));
    let mut grid = vec![0u32; (n + 1) * (n + 1)];
    let mut uvs = vec![Vec2::ZERO; (n + 1) * (n + 1)];
    for j in 0..=n {
        for i in 0..=n {
            let (s, t) = (i as f32 / n as f32, j as f32 / n as f32);
            uvs[j * (n + 1) + i] = u[0].lerp(u[1], s).lerp(u[3].lerp(u[2], s), t);
            grid[j * (n + 1) + i] = if j == 0 {
                bottom[i]
            } else if j == n {
                top[i]
            } else if i == 0 {
                left[j]
            } else if i == n {
                right[j]
            } else {
                let q = p(0, mesh).lerp(p(1, mesh), s).lerp(p(3, mesh).lerp(p(2, mesh), s), t);
                mesh.add_vertex(q)
            };
        }
    }
    let at = |i: usize, j: usize| j * (n + 1) + i;
    let mut out = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let c = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            out.push(face_like(c.iter().map(|&k| grid[k]).collect(), c.iter().map(|&k| uvs[k]).collect(), f));
        }
    }
    out
}

fn grid_triangle(mesh: &mut Mesh, f: &Face, split: &HashMap<(u32, u32), Vec<u32>>, cuts: usize) -> Vec<Face> {
    let x = &f.verts;
    let u = &f.uvs;
    let n = cuts + 1;
    let (e01, e12, e02) = (edge_row(split, x[0], x[1]), edge_row(split, x[1], x[2]), edge_row(split, x[0], x[2]));
    let (p0, p1, p2) = (mesh.positions[x[0] as usize], mesh.positions[x[1] as usize], mesh.positions[x[2] as usize]);
    // the point i steps along the first edge and j along the last, on the edges where it can
    let on_edge = |i: usize, j: usize| match (i, j) {
        (_, 0) => Some(e01[i]),
        (0, _) => Some(e02[j]),
        _ if i + j == n => Some(e12[j]),
        _ => None,
    };
    let mut pts: HashMap<(usize, usize), (u32, Vec2)> = HashMap::new();
    for j in 0..=n {
        for i in 0..=n - j {
            let (a, b) = (i as f32 / n as f32, j as f32 / n as f32);
            let uv = u[0] + (u[1] - u[0]) * a + (u[2] - u[0]) * b;
            let v = on_edge(i, j).unwrap_or_else(|| mesh.add_vertex(p0 + (p1 - p0) * a + (p2 - p0) * b));
            pts.insert((i, j), (v, uv));
        }
    }
    let tri = |c: [(usize, usize); 3]| face_like(c.iter().map(|k| pts[k].0).collect(), c.iter().map(|k| pts[k].1).collect(), f);
    let mut out = Vec::new();
    for j in 0..n {
        for i in 0..n - j {
            out.push(tri([(i, j), (i + 1, j), (i, j + 1)]));
            if i + j + 1 < n {
                out.push(tri([(i + 1, j), (i + 1, j + 1), (i, j + 1)]));
            }
        }
    }
    out
}

fn fan_polygon(mesh: &mut Mesh, f: &Face, split: &HashMap<(u32, u32), Vec<u32>>, cuts: usize) -> Vec<Face> {
    let k = f.verts.len();
    let centre = f.verts.iter().map(|&v| mesh.positions[v as usize]).sum::<Vec3>() / k as f32;
    let c = mesh.add_vertex(centre);
    let cuv = f.uvs.iter().copied().sum::<Vec2>() / k as f32;
    // each edge's points with UVs, from its first corner
    let rows: Vec<Vec<(u32, Vec2)>> = (0..k)
        .map(|i| {
            let (a, b) = (f.verts[i], f.verts[(i + 1) % k]);
            let (pa, pb) = (mesh.positions[a as usize], mesh.positions[b as usize]);
            along(split, a, b).into_iter().map(|p| (p, f.uvs[i].lerp(f.uvs[(i + 1) % k], param(mesh.positions[p as usize], pa, pb)))).collect()
        })
        .collect();
    let h = (cuts - 1) / 2;
    (0..k)
        .map(|i| {
            let prev = &rows[(i + k - 1) % k];
            let mut corners: Vec<(u32, Vec2)> = vec![(c, cuv)];
            corners.extend_from_slice(&prev[h..]);
            corners.push((f.verts[i], f.uvs[i]));
            corners.extend_from_slice(&rows[i][..=h]);
            face_like(corners.iter().map(|x| x.0).collect(), corners.iter().map(|x| x.1).collect(), f)
        })
        .collect()
}

/// Bisect: cut the selected faces along the plane through `point` across `normal`. Edges it
/// crosses get a vertex (their other faces take it too); the new edges are selected after.
pub fn knife_plane(mesh: &mut Mesh, sel: &Selection, point: Vec3, normal: Vec3) -> Selection {
    let n = normal.normalize_or_zero();
    let faces = sel_faces(mesh, sel);
    if n == Vec3::ZERO || faces.is_empty() {
        return kept(mesh, sel);
    }
    let eps = 1e-5;
    let mut d: Vec<f32> = mesh.positions.iter().map(|&p| (p - point).dot(n)).collect();
    let mut split: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for &f in &faces {
        let edges: Vec<(u32, u32)> = mesh.faces[f].edges().collect();
        for (a, b) in edges {
            let k = edge_key(a, b);
            let (da, db) = (d[k.0 as usize], d[k.1 as usize]);
            if split.contains_key(&k) || !((da < -eps && db > eps) || (da > eps && db < -eps)) {
                continue;
            }
            let t = da / (da - db);
            let p = mesh.positions[k.0 as usize].lerp(mesh.positions[k.1 as usize], t);
            split.insert(k, vec![mesh.add_vertex(p)]);
            d.push(0.0);
        }
    }
    for f in 0..mesh.faces.len() {
        if mesh.faces[f].edges().any(|(a, b)| split.contains_key(&edge_key(a, b))) {
            mesh.faces[f] = with_edge_points(&mesh.positions, &mesh.faces[f], &split);
        }
    }
    split_marks(mesh, &split);
    // a cut never doubles an edge the mesh has
    let mut have: HashSet<(u32, u32)> = mesh.edges().into_iter().collect();
    let mut new_edges = Vec::new();
    for &f in &faces {
        let face = mesh.faces[f].clone();
        let on: Vec<u32> = face.verts.iter().copied().filter(|&v| d[v as usize].abs() <= eps).collect();
        new_edges.extend(face.edges().filter(|(a, b)| on.contains(a) && on.contains(b)));
        if on.len() < 2 {
            continue;
        }
        // pair the points on the plane in order along the cut
        let line = n.cross(mesh.face_normal(f));
        let mut on = on;
        on.sort_by(|a, b| mesh.positions[*a as usize].dot(line).total_cmp(&mesh.positions[*b as usize].dot(line)));
        let chords: Vec<(u32, u32)> = on.chunks_exact(2).map(|c| (c[0], c[1])).filter(|&(a, b)| !have.contains(&edge_key(a, b))).collect();
        have.extend(chords.iter().map(|&(a, b)| edge_key(a, b)));
        let pieces = split_by_chords(&face, &chords);
        if pieces.len() > 1 {
            new_edges.extend(chords);
        }
        let mut pieces = pieces.into_iter();
        if let Some(first) = pieces.next() {
            mesh.faces[f] = first;
        }
        mesh.faces.extend(pieces);
    }
    Selection::from_edges(mesh, new_edges)
}

/// Connect (J): split faces between the selected vertices they hold (in turn round the face),
/// where no edge joins them yet. The new edges are selected after.
pub fn connect(mesh: &mut Mesh, sel: &Selection) -> Selection {
    let verts = sel_verts(mesh, sel);
    let mut have: HashSet<(u32, u32)> = mesh.edges().into_iter().collect();
    let mut chords_made = Vec::new();
    for f in 0..mesh.faces.len() {
        let face = mesh.faces[f].clone();
        let picked: Vec<u32> = face.verts.iter().copied().filter(|v| verts.contains(v)).collect();
        if picked.len() < 2 {
            continue;
        }
        let pairs: Vec<(u32, u32)> = if picked.len() == 2 { vec![(picked[0], picked[1])] } else { (0..picked.len()).map(|i| (picked[i], picked[(i + 1) % picked.len()])).collect() };
        let chords: Vec<(u32, u32)> = pairs.into_iter().filter(|&(a, b)| !have.contains(&edge_key(a, b))).collect();
        let pieces = split_by_chords(&face, &chords);
        if pieces.len() < 2 {
            continue;
        }
        have.extend(chords.iter().map(|&(a, b)| edge_key(a, b)));
        chords_made.extend(chords);
        let mut pieces = pieces.into_iter();
        if let Some(first) = pieces.next() {
            mesh.faces[f] = first;
        }
        mesh.faces.extend(pieces);
    }
    if chords_made.is_empty() {
        return kept(mesh, sel);
    }
    Selection::from_edges(mesh, chords_made)
}

/// Where merged vertices go.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum MergeAt {
    /// The middle of the selected vertices.
    Center,
    /// The lowest-numbered selected vertex (the selection keeps no order).
    First,
    /// The highest-numbered selected vertex.
    Last,
    Cursor(Vec3),
    /// Each connected piece of the selection to its own middle.
    Collapse,
}

/// Faces cut where a vertex comes round twice (as merging leaves them), pieces of fewer than
/// three corners dropped.
fn split_repeats(f: Face) -> Vec<Face> {
    let f = dedup_corners(f);
    let n = f.verts.len();
    for i in 0..n {
        if let Some(j) = (i + 1..n).find(|&j| f.verts[j] == f.verts[i]) {
            let a = face_like(f.verts[i..j].to_vec(), f.uvs[i..j].to_vec(), &f);
            let b = face_like(f.verts[j..].iter().chain(&f.verts[..i]).copied().collect(), f.uvs[j..].iter().chain(&f.uvs[..i]).copied().collect(), &f);
            return split_repeats(a).into_iter().chain(split_repeats(b)).collect();
        }
    }
    if n >= 3 {
        vec![f]
    } else {
        Vec::new()
    }
}

/// A face's corners turned to start at its lowest vertex.
fn canonical(verts: &[u32]) -> Vec<u32> {
    let at = verts.iter().enumerate().min_by_key(|(_, v)| **v).map(|(i, _)| i).unwrap_or(0);
    verts[at..].iter().chain(&verts[..at]).copied().collect()
}

/// Renumber vertices by `map` (several onto one to merge them), then tidy the faces: corners
/// that come together go, faces that pinch are cut apart, faces left back to back on the same
/// corners go both, copies of a face go.
fn apply_vertex_map(mesh: &mut Mesh, map: &[u32]) {
    mesh.remap(map);
    let faces: Vec<Face> = std::mem::take(&mut mesh.faces).into_iter().flat_map(split_repeats).collect();
    let keys: Vec<Vec<u32>> = faces.iter().map(|f| canonical(&f.verts)).collect();
    let mut by_key: HashMap<Vec<u32>, Vec<usize>> = HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        by_key.entry(k.clone()).or_default().push(i);
    }
    let mut drop = vec![false; faces.len()];
    for (i, f) in faces.iter().enumerate() {
        if drop[i] {
            continue;
        }
        if by_key.get(&keys[i]).is_some_and(|l| l.iter().any(|&j| j < i && !drop[j])) {
            drop[i] = true;
            continue;
        }
        let rev: Vec<u32> = f.verts.iter().rev().copied().collect();
        if let Some(&j) = by_key.get(&canonical(&rev)).and_then(|l| l.iter().find(|&&j| j != i && !drop[j])) {
            drop[i] = true;
            drop[j] = true;
        }
    }
    mesh.faces = faces.into_iter().zip(drop).filter(|(_, d)| !d).map(|(f, _)| f).collect();
    prune_marks(mesh);
}

/// Merge (M): the selected vertices into one (each connected piece into one with
/// `Collapse`); the merged vertices are selected after.
pub fn merge(mesh: &mut Mesh, sel: &Selection, at: MergeAt) -> Selection {
    let verts = sel_verts(mesh, sel);
    if verts.is_empty() {
        return Selection::default();
    }
    let groups: Vec<Vec<u32>> = if at == MergeAt::Collapse {
        let mut root: HashMap<u32, u32> = verts.iter().map(|&v| (v, v)).collect();
        fn find(root: &mut HashMap<u32, u32>, v: u32) -> u32 {
            let r = root[&v];
            if r == v {
                return v;
            }
            let top = find(root, r);
            root.insert(v, top);
            top
        }
        for (a, b) in mesh.edges() {
            if verts.contains(&a) && verts.contains(&b) {
                let (ra, rb) = (find(&mut root, a), find(&mut root, b));
                root.insert(ra.max(rb), ra.min(rb));
            }
        }
        let mut groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for &v in &verts {
            let r = find(&mut root, v);
            groups.entry(r).or_default().push(v);
        }
        groups.into_values().collect()
    } else {
        vec![verts.iter().copied().collect()]
    };
    let mut map: Vec<u32> = (0..mesh.positions.len() as u32).collect();
    let mut targets = Vec::new();
    for g in groups {
        let mean = g.iter().map(|&v| mesh.positions[v as usize]).sum::<Vec3>() / g.len() as f32;
        let (first, last) = (g[0], g[g.len() - 1]);
        let (t, p) = match at {
            MergeAt::Center | MergeAt::Collapse => (first, mean),
            MergeAt::First => (first, mesh.positions[first as usize]),
            MergeAt::Last => (last, mesh.positions[last as usize]),
            MergeAt::Cursor(c) => (first, c),
        };
        mesh.positions[t as usize] = p;
        for v in g {
            map[v as usize] = t;
        }
        targets.push(t);
    }
    apply_vertex_map(mesh, &map);
    let map = compact(mesh);
    Selection::from_verts(mesh, targets.into_iter().filter_map(|t| mapped(&map, t)).collect::<Vec<_>>())
}

/// Merge by distance: selected vertices closer than `distance` to each other become one.
/// How many went (the vertices are renumbered: select again after).
pub fn merge_by_distance(mesh: &mut Mesh, sel: &Selection, distance: f32) -> usize {
    let verts: Vec<u32> = sel_verts(mesh, sel).into_iter().collect();
    if verts.len() < 2 {
        return 0;
    }
    let pts: Vec<Vec3> = verts.iter().map(|&v| mesh.positions[v as usize]).collect();
    let sub = crate::weld_map(&pts, distance.max(0.0));
    let mut map: Vec<u32> = (0..mesh.positions.len() as u32).collect();
    let mut gone = 0;
    for (i, &v) in verts.iter().enumerate() {
        map[v as usize] = verts[sub[i] as usize];
        gone += usize::from(sub[i] as usize != i);
    }
    if gone > 0 {
        apply_vertex_map(mesh, &map);
        compact(mesh);
    }
    gone
}

/// The faces round a vertex joined into one polygon without it (none when they do not make
/// one simple polygon).
fn joined_round(mesh: &Mesh, fan: &Fan) -> Option<Face> {
    let n = fan.corners.len();
    let mut verts = Vec::new();
    let mut uvs = Vec::new();
    for (k, i) in (0..n).rev().enumerate() {
        let (f, c) = fan.corners[i];
        let face = &mesh.faces[f];
        let m = face.verts.len();
        // the corners after the vertex round to the one before it; all but the last, which
        // starts the next face, except for the last face of an open fan
        let take = if !fan.closed && k == n - 1 { m - 1 } else { m - 2 };
        for s in 1..=take {
            verts.push(face.verts[(c + s) % m]);
            uvs.push(face.uvs[(c + s) % m]);
        }
    }
    let distinct: HashSet<u32> = verts.iter().copied().collect();
    (verts.len() >= 3 && distinct.len() == verts.len()).then(|| face_like(verts, uvs, &mesh.faces[fan.corners[0].0]))
}

/// The vertices joined to `v` through the faces round it.
fn face_neighbours(mesh: &Mesh, around: &[usize], v: u32) -> BTreeSet<u32> {
    around
        .iter()
        .flat_map(|&f| {
            let face = &mesh.faces[f];
            let c = face.verts.iter().position(|&x| x == v).unwrap_or(0);
            [prev_vert(face, c), next_vert(face, c)]
        })
        .collect()
}

/// Take out a vertex lying between just two edges (its two edges become one), unless a face
/// would drop below three corners or its two neighbours are joined already.
fn drop_between(mesh: &mut Mesh, around: &[usize], v: u32) {
    let nb = face_neighbours(mesh, around, v);
    let [a, b] = nb.iter().copied().collect::<Vec<_>>()[..] else { return };
    if around.is_empty() || around.iter().any(|&f| mesh.faces[f].verts.len() <= 3) || mesh.faces.iter().any(|f| f.edges().any(|e| e == (a, b) || e == (b, a))) {
        return;
    }
    for &f in around {
        let face = &mut mesh.faces[f];
        if let Some(c) = face.verts.iter().position(|&x| x == v) {
            face.verts.remove(c);
            face.uvs.remove(c);
        }
    }
}

/// Dissolve vertices: each selected vertex goes, the faces round it joined into one (a vertex
/// between just two edges only leaves the faces). The joined faces are selected after.
pub fn dissolve_vertices(mesh: &mut Mesh, sel: &Selection) -> Selection {
    let verts = sel_verts(mesh, sel);
    let mut made: Vec<bool> = vec![false; mesh.faces.len()];
    for v in verts {
        let vf = mesh.vertex_faces();
        let around = &vf[v as usize];
        if around.is_empty() {
            continue;
        }
        if face_neighbours(mesh, around, v).len() == 2 {
            drop_between(mesh, around, v);
            continue;
        }
        let dir = directed(mesh);
        let Some(fan) = fan(mesh, &dir, &vf, v) else { continue };
        let Some(joined) = joined_round(mesh, &fan) else { continue };
        let gone: HashSet<usize> = fan.corners.iter().map(|c| c.0).collect();
        let old = std::mem::take(&mut mesh.faces);
        let old_made = std::mem::take(&mut made);
        for (i, (f, m)) in old.into_iter().zip(old_made).enumerate() {
            if !gone.contains(&i) {
                mesh.faces.push(f);
                made.push(m);
            }
        }
        mesh.faces.push(joined);
        made.push(true);
    }
    compact(mesh);
    Selection::from_faces(mesh, made.iter().enumerate().filter(|(_, m)| **m).map(|(i, _)| i).collect::<Vec<_>>())
}

/// A polygon's spikes (a corner whose neighbours are the same vertex) and repeated corners
/// taken out.
fn despike(mut verts: Vec<u32>, mut uvs: Vec<Vec2>) -> (Vec<u32>, Vec<Vec2>) {
    loop {
        let n = verts.len();
        if n < 3 {
            return (verts, uvs);
        }
        let hit = (0..n).find(|&i| verts[i] == verts[(i + 1) % n]).map(|i| vec![(i + 1) % n]).or_else(|| (0..n).find(|&i| verts[(i + n - 1) % n] == verts[(i + 1) % n]).map(|i| vec![i, (i + 1) % n]));
        let Some(mut drop) = hit else { return (verts, uvs) };
        drop.sort_unstable_by(|a, b| b.cmp(a));
        for i in drop {
            verts.remove(i);
            uvs.remove(i);
        }
    }
}

/// Dissolve edges: the two faces on each selected edge joined into one; vertices left between
/// just two edges go too. The joined faces are selected after.
pub fn dissolve_edges(mesh: &mut Mesh, sel: &Selection) -> Selection {
    let edges = sel_edges(mesh, sel);
    let mut made: Vec<bool> = vec![false; mesh.faces.len()];
    let mut touched = BTreeSet::new();
    for &(a, b) in &edges {
        let dir = directed(mesh);
        let (Some(&fa), Some(&fb)) = (dir.get(&(a, b)), dir.get(&(b, a))) else { continue };
        if fa == fb {
            continue;
        }
        let (f, g) = (&mesh.faces[fa], &mesh.faces[fb]);
        let (Some(ia), Some(ib)) = (f.verts.iter().position(|&x| x == b), g.verts.iter().position(|&x| x == a)) else { continue };
        // fa from b round to a, then fb from a round to b, without the ends again
        let (n, m) = (f.verts.len(), g.verts.len());
        let mut verts: Vec<u32> = (0..n).map(|s| f.verts[(ia + s) % n]).collect();
        let mut uvs: Vec<Vec2> = (0..n).map(|s| f.uvs[(ia + s) % n]).collect();
        verts.extend((1..m - 1).map(|s| g.verts[(ib + s) % m]));
        uvs.extend((1..m - 1).map(|s| g.uvs[(ib + s) % m]));
        let (verts, uvs) = despike(verts, uvs);
        let distinct: HashSet<u32> = verts.iter().copied().collect();
        if verts.len() < 3 || distinct.len() != verts.len() {
            continue;
        }
        mesh.faces[fa] = face_like(verts, uvs, f);
        made[fa] = true;
        mesh.faces.remove(fb);
        made.remove(fb);
        touched.extend([a, b]);
    }
    for v in touched {
        let vf = mesh.vertex_faces();
        if let Some(around) = vf.get(v as usize) {
            drop_between(mesh, around, v);
        }
    }
    compact(mesh);
    Selection::from_faces(mesh, made.iter().enumerate().filter(|(_, m)| **m).map(|(i, _)| i).collect::<Vec<_>>())
}

/// Dissolve faces: each connected piece of selected faces joined into one polygon (pieces
/// with a hole or no boundary stay). The joined faces are selected after.
pub fn dissolve_faces(mesh: &mut Mesh, sel: &Selection) -> Selection {
    let faces = sel_faces(mesh, sel);
    let mut joined: Vec<(BTreeSet<usize>, Face)> = Vec::new();
    for piece in edge_pieces(mesh, &faces) {
        if piece.len() < 2 {
            continue;
        }
        let (boundary, _) = region_boundary(mesh, &piece);
        let edges: Vec<(u32, u32)> = boundary.iter().map(|&(a, b, _)| (a, b)).collect();
        let loops = chains(&edges);
        let [(ref lp, true)] = loops[..] else { continue };
        if lp.len() != edges.len() {
            continue;
        }
        let corner_uv: HashMap<u32, Vec2> = boundary
            .iter()
            .map(|&(a, _, f)| {
                let face = &mesh.faces[f];
                (a, face.uvs[face.verts.iter().position(|&x| x == a).unwrap_or(0)])
            })
            .collect();
        let first = *piece.iter().next().unwrap_or(&0);
        let uvs = lp.iter().map(|v| corner_uv.get(v).copied().unwrap_or_default()).collect();
        joined.push((piece, face_like(lp.clone(), uvs, &mesh.faces[first])));
    }
    let gone: HashSet<usize> = joined.iter().flat_map(|(p, _)| p.iter().copied()).collect();
    let old = std::mem::take(&mut mesh.faces);
    mesh.faces = old.into_iter().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, f)| f).collect();
    let first = mesh.faces.len();
    mesh.faces.extend(joined.into_iter().map(|(_, f)| f));
    let n = mesh.faces.len();
    compact(mesh);
    Selection::from_faces(mesh, first..n)
}

/// What [`delete`] takes away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Delete {
    /// The selected vertices and every face on them.
    Vertices,
    /// The selected edges and every face on them.
    Edges,
    /// The selected faces and the vertices only they used.
    Faces,
    /// The selected faces only: their vertices stay (selected).
    OnlyFaces,
}

/// Delete (X).
pub fn delete(mesh: &mut Mesh, sel: &Selection, what: Delete) -> Selection {
    let gone: Vec<bool> = match what {
        Delete::Vertices => {
            let verts = sel_verts(mesh, sel);
            mesh.faces.iter().map(|f| f.verts.iter().any(|v| verts.contains(v))).collect()
        }
        Delete::Edges => {
            let edges = sel_edges(mesh, sel);
            mesh.faces.iter().map(|f| f.edges().any(|(a, b)| edges.contains(&edge_key(a, b)))).collect()
        }
        Delete::Faces | Delete::OnlyFaces => {
            let faces = sel_faces(mesh, sel);
            (0..mesh.faces.len()).map(|f| faces.contains(&f)).collect()
        }
    };
    let mut freed = BTreeSet::new();
    let old = std::mem::take(&mut mesh.faces);
    for (f, g) in old.into_iter().zip(gone) {
        if g {
            freed.extend(f.verts.iter().copied());
        } else {
            mesh.faces.push(f);
        }
    }
    if what == Delete::OnlyFaces {
        prune_marks(mesh);
        return Selection::from_verts(mesh, freed);
    }
    compact(mesh);
    Selection::default()
}

/// The vertices among these on an open edge or on no face: those a new face can use.
fn open_verts(mesh: &Mesh, ef: &HashMap<(u32, u32), Vec<usize>>, verts: &BTreeSet<u32>) -> BTreeSet<u32> {
    let mut on_face = vec![false; mesh.positions.len()];
    let mut open = vec![false; mesh.positions.len()];
    for f in &mesh.faces {
        for (a, b) in f.edges() {
            on_face[a as usize] = true;
            if ef.get(&edge_key(a, b)).is_some_and(|l| l.len() == 1) {
                open[a as usize] = true;
                open[b as usize] = true;
            }
        }
    }
    verts.iter().copied().filter(|&v| open[v as usize] || !on_face[v as usize]).collect()
}

/// Fill (F): a face over each loop of selected boundary edges, or else over the selected
/// vertices on open edges (or on no face) in order round their middle; two vertices of one
/// face are joined across it instead. The new faces are selected after.
pub fn fill(mesh: &mut Mesh, sel: &Selection) -> Selection {
    let verts = sel_verts(mesh, sel);
    if verts.len() == 2 {
        let (a, b) = (verts.first().copied().unwrap_or(0), verts.last().copied().unwrap_or(0));
        if mesh.faces.iter().any(|f| f.verts.contains(&a) && f.verts.contains(&b)) {
            return connect(mesh, sel);
        }
    }
    let ef = mesh.edge_faces();
    let dir = directed(mesh);
    let rim: Vec<(u32, u32)> = sel_edges(mesh, sel).into_iter().filter(|e| ef.get(e).is_some_and(|l| l.len() == 1)).map(|(a, b)| if dir.contains_key(&(a, b)) { (b, a) } else { (a, b) }).collect();
    let loops: Vec<Vec<u32>> = chains(&rim).into_iter().filter(|(l, closed)| *closed && l.len() >= 3).map(|(l, _)| l).collect();
    let like = |v: u32| mesh.faces.iter().find(|f| f.verts.contains(&v)).cloned().unwrap_or_default();
    let open = open_verts(mesh, &ef, &verts);
    let mut new_faces = Vec::new();
    if !loops.is_empty() {
        for lp in loops {
            new_faces.push(planar_face(mesh, lp.clone(), &like(lp[0])));
        }
    } else if open.len() >= 3 {
        let verts = open;
        let pts: Vec<Vec3> = verts.iter().map(|&v| mesh.positions[v as usize]).collect();
        let c = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
        // the plane's normal: the largest cross product of two spokes from the middle
        let mut n = Vec3::ZERO;
        for i in 0..pts.len() {
            for j in i + 1..pts.len() {
                let x = (pts[i] - c).cross(pts[j] - c);
                if x.length_squared() > n.length_squared() {
                    n = x;
                }
            }
        }
        let n = if n == Vec3::ZERO { Vec3::Z } else { n.normalize() };
        let u = n.any_orthonormal_vector();
        let w = n.cross(u);
        let mut order: Vec<u32> = verts.iter().copied().collect();
        order.sort_by(|&a, &b| {
            let (pa, pb) = (mesh.positions[a as usize] - c, mesh.positions[b as usize] - c);
            pa.dot(w).atan2(pa.dot(u)).total_cmp(&pb.dot(w).atan2(pb.dot(u)))
        });
        // agree with faces along its edges, or else face away from the mesh's middle
        let k = order.len();
        let votes: i32 = (0..k).map(|i| (order[i], order[(i + 1) % k])).map(|(a, b)| i32::from(dir.contains_key(&(b, a))) - i32::from(dir.contains_key(&(a, b)))).sum();
        let mid = mesh.positions.iter().copied().sum::<Vec3>() / mesh.positions.len().max(1) as f32;
        if votes < 0 || (votes == 0 && n.dot(c - mid) < 0.0) {
            order.reverse();
        }
        new_faces.push(planar_face(mesh, order.clone(), &like(order[0])));
    }
    let first = mesh.faces.len();
    mesh.faces.extend(new_faces);
    Selection::from_faces(mesh, first..mesh.faces.len())
}

/// Bridge edge loops: two selected boundary loops (or open chains) joined by a band of quads,
/// with triangles where their vertex counts differ. Selected faces are deleted first and the
/// boundaries they leave bridged. The new faces are selected after.
pub fn bridge_loops(mesh: &mut Mesh, sel: &Selection) -> Selection {
    let faces = sel_faces(mesh, sel);
    // each loop runs its edges the way the faces beside it (that stay) do
    let rim: Vec<(u32, u32)> = if !faces.is_empty() {
        region_boundary(mesh, &faces).0.into_iter().map(|(a, b, _)| (b, a)).collect()
    } else {
        let ef = mesh.edge_faces();
        let dir = directed(mesh);
        sel_edges(mesh, sel).into_iter().filter(|e| ef.get(e).is_some_and(|l| l.len() == 1)).map(|(a, b)| if dir.contains_key(&(a, b)) { (a, b) } else { (b, a) }).collect()
    };
    let loops = chains(&rim);
    let [(a, closed), (b, closed_b)] = &loops[..] else { return kept(mesh, sel) };
    if closed != closed_b {
        return kept(mesh, sel);
    }
    let (a, closed) = (a.clone(), *closed);
    let like = faces.iter().next().map(|&f| mesh.faces[f].clone()).or_else(|| mesh.faces.iter().find(|f| f.verts.contains(&a[0])).cloned()).unwrap_or_default();
    if !faces.is_empty() {
        let old = std::mem::take(&mut mesh.faces);
        mesh.faces = old.into_iter().enumerate().filter(|(i, _)| !faces.contains(i)).map(|(_, f)| f).collect();
    }
    let p = |v: u32| mesh.positions[v as usize];
    let centre = |l: &[u32]| l.iter().map(|&v| p(v)).sum::<Vec3>() / l.len() as f32;
    let loop_normal = |l: &[u32]| polygon_normal(&l.iter().map(|&v| p(v)).collect::<Vec<_>>());
    // b runs against a when the two face each other; turn it to run alongside
    let mut bp: Vec<u32> = b.iter().rev().copied().collect();
    let mut turned = false;
    if closed {
        if loop_normal(&a).dot(loop_normal(&bp)) < 0.0 {
            bp.reverse();
            turned = true;
        }
        let (ca, cb) = (centre(&a), centre(&bp));
        let cost = |o: usize| -> f32 {
            if a.len() == bp.len() {
                (0..a.len()).map(|i| ((p(a[i]) - ca) - (p(bp[(i + o) % bp.len()]) - cb)).length_squared()).sum()
            } else {
                ((p(a[0]) - ca).normalize_or_zero() - (p(bp[o]) - cb).normalize_or_zero()).length_squared()
            }
        };
        let best = (0..bp.len()).min_by(|&x, &y| cost(x).total_cmp(&cost(y))).unwrap_or(0);
        bp.rotate_left(best);
    } else {
        let (a0, a1, b0, b1) = (p(a[0]), p(a[a.len() - 1]), p(bp[0]), p(bp[bp.len() - 1]));
        if a0.distance(b0) + a1.distance(b1) > a0.distance(b1) + a1.distance(b0) {
            bp.reverse();
            turned = true;
        }
    }
    // walk both by length along them, a quad where they step together, a triangle elsewhere
    let along_len = |l: &[u32]| -> Vec<f32> {
        let k = if closed { l.len() } else { l.len() - 1 };
        let mut t = vec![0.0];
        for i in 0..k {
            let d = p(l[i]).distance(p(l[(i + 1) % l.len()]));
            t.push(t[i] + d);
        }
        let total = t[k].max(1e-12);
        t.iter().map(|x| x / total).collect()
    };
    let (ta, tb) = (along_len(&a), along_len(&bp));
    let (ea, eb) = (ta.len() - 1, tb.len() - 1);
    let av = |i: usize| a[i % a.len()];
    let bv = |j: usize| bp[j % bp.len()];
    let (mut i, mut j) = (0, 0);
    let mut new_faces: Vec<Vec<u32>> = Vec::new();
    while i < ea || j < eb {
        let both = i < ea && j < eb;
        if both && (ea == eb || (ta[i + 1] - tb[j + 1]).abs() <= 0.25 * ((ta[i + 1] - ta[i]) + (tb[j + 1] - tb[j]))) {
            new_faces.push(vec![av(i + 1), av(i), bv(j), bv(j + 1)]);
            i += 1;
            j += 1;
        } else if j >= eb || (i < ea && ta[i + 1] <= tb[j + 1]) {
            new_faces.push(vec![av(i + 1), av(i), bv(j)]);
            i += 1;
        } else {
            new_faces.push(vec![av(i), bv(j), bv(j + 1)]);
            j += 1;
        }
    }
    let first = mesh.faces.len();
    for verts in new_faces {
        let distinct: HashSet<u32> = verts.iter().copied().collect();
        if distinct.len() == verts.len() {
            let f = planar_face(mesh, verts, &like);
            mesh.faces.push(f);
        }
    }
    let n = mesh.faces.len();
    // which way the band faces: along the faces beside it; between loose loops, away from the
    // line through their middles
    let ef = mesh.edge_faces();
    let has_faces = |l: &[u32]| (0..l.len()).any(|k| ef.get(&edge_key(l[k], l[(k + 1) % l.len()])).is_some_and(|f| f.iter().any(|&g| g < first)));
    let (fa, fb) = (has_faces(&a), has_faces(&bp));
    let flip = if !fa && fb {
        turned
    } else if !fa && !fb {
        let (ca, axis) = (centre(&a), (centre(&bp) - centre(&a)).normalize_or_zero());
        let out: f32 = (first..n)
            .map(|f| {
                let c = mesh.face_center(f) - ca;
                mesh.face_normal(f).dot(c - axis * c.dot(axis))
            })
            .sum();
        out < 0.0
    } else {
        false
    };
    if flip {
        for f in first..n {
            mesh.faces[f].reverse();
        }
    }
    compact(mesh);
    Selection::from_faces(mesh, first..n)
}

/// Spin: the selection swept `angle_deg` round the axis through `center`, in `steps` steps
/// (selected faces as a region, else selected edges into bands). A full turn closes on
/// itself. The last step's faces (or edges) are selected after; with a full turn, all new
/// faces.
pub fn spin(mesh: &mut Mesh, sel: &Selection, axis: Vec3, center: Vec3, angle_deg: f32, steps: u32) -> Selection {
    let axis = axis.normalize_or_zero();
    if axis == Vec3::ZERO || !angle_deg.is_finite() || angle_deg == 0.0 {
        return kept(mesh, sel);
    }
    let steps = steps.clamp(1, 1024);
    let full = angle_deg.abs() >= 359.999;
    let turn = |k: u32, p: Vec3| Quat::from_axis_angle(axis, (angle_deg * k as f32 / steps as f32).to_radians()) * (p - center) + center;
    let faces = sel_faces(mesh, sel);
    // the swept vertices: a copy for each step (the last is the first again on a full turn)
    let rings = |mesh: &mut Mesh, verts: &BTreeSet<u32>| -> Vec<BTreeMap<u32, u32>> {
        let mut rings = vec![verts.iter().map(|&v| (v, v)).collect::<BTreeMap<u32, u32>>()];
        for k in 1..=steps {
            if full && k == steps {
                rings.push(rings[0].clone());
            } else {
                rings.push(verts.iter().map(|&v| (v, mesh.add_vertex(turn(k, mesh.positions[v as usize])))).collect());
            }
        }
        rings
    };
    if !faces.is_empty() {
        let (boundary, attached) = region_boundary(mesh, &faces);
        let verts: BTreeSet<u32> = faces.iter().flat_map(|&f| mesh.faces[f].verts.clone()).collect();
        let normal = faces_normal(mesh, &faces);
        let mid = verts.iter().map(|&v| mesh.positions[v as usize]).sum::<Vec3>() / verts.len() as f32;
        let r = rings(mesh, &verts);
        let old = std::mem::take(&mut mesh.faces);
        let keep_start = !full && !attached;
        let mut start_caps = Vec::new();
        for (i, f) in old.iter().enumerate() {
            if !faces.contains(&i) {
                mesh.faces.push(f.clone());
            } else if keep_start {
                let mut g = f.clone();
                g.reverse();
                mesh.faces.push(g);
                start_caps.push(mesh.faces.len() - 1);
            }
        }
        let first = mesh.faces.len();
        for k in 0..steps as usize {
            for &(a, b, f) in &boundary {
                mesh.faces.push(face_like(vec![r[k][&a], r[k][&b], r[k + 1][&b], r[k + 1][&a]], wall_uvs(1.0), &old[f]));
            }
        }
        let walls_end = mesh.faces.len();
        let mut caps = Vec::new();
        if !full {
            for &f in &faces {
                let src = &old[f];
                mesh.faces.push(face_like(src.verts.iter().map(|v| r[steps as usize][v]).collect(), src.uvs.clone(), src));
                caps.push(mesh.faces.len() - 1);
            }
        }
        refresh_uvs(mesh, &(first..walls_end).collect::<Vec<_>>());
        // a region on its own swept against its normal would come out inside out
        let sweep = axis.cross(mid - center) * angle_deg.signum();
        if !attached && sweep.dot(normal) < 0.0 {
            for f in start_caps.iter().copied().chain(first..mesh.faces.len()) {
                mesh.faces[f].reverse();
            }
        }
        compact(mesh);
        return Selection::from_faces(mesh, if full { (first..walls_end).collect() } else { caps });
    }
    let dir = directed(mesh);
    let edges: Vec<(u32, u32, usize)> = sel_edges(mesh, sel)
        .into_iter()
        .filter_map(|(a, b)| match (dir.get(&(a, b)), dir.get(&(b, a))) {
            (Some(&f), _) => Some((a, b, f)),
            (None, Some(&f)) => Some((b, a, f)),
            _ => None,
        })
        .collect();
    if edges.is_empty() {
        return kept(mesh, sel);
    }
    let verts: BTreeSet<u32> = edges.iter().flat_map(|&(a, b, _)| [a, b]).collect();
    let r = rings(mesh, &verts);
    let first = mesh.faces.len();
    for k in 0..steps as usize {
        for &(a, b, f) in &edges {
            let like = mesh.faces[f].clone();
            mesh.faces.push(face_like(vec![r[k][&b], r[k][&a], r[k + 1][&a], r[k + 1][&b]], wall_uvs(1.0), &like));
        }
    }
    let n = mesh.faces.len();
    refresh_uvs(mesh, &(first..n).collect::<Vec<_>>());
    if full {
        Selection::from_faces(mesh, first..n)
    } else {
        let last = &r[steps as usize];
        Selection::from_edges(mesh, edges.iter().map(|&(a, b, _)| (last[&a], last[&b])).collect::<Vec<_>>())
    }
}

/// Duplicate (Shift+D): copies of the selected faces on new vertices, not joined to anything;
/// the copies are selected after.
pub fn duplicate(mesh: &mut Mesh, sel: &Selection) -> Selection {
    let faces = sel_faces(mesh, sel);
    if faces.is_empty() {
        return kept(mesh, sel);
    }
    let mut copy = BTreeMap::new();
    let mut edges = BTreeSet::new();
    let first = mesh.faces.len();
    for &f in &faces {
        let mut g = mesh.faces[f].clone();
        for i in 0..g.verts.len() {
            edges.insert(edge_key(g.verts[i], g.verts[(i + 1) % g.verts.len()]));
        }
        for v in &mut g.verts {
            let p = mesh.positions[*v as usize];
            *v = *copy.entry(*v).or_insert_with(|| {
                mesh.positions.push(p);
                (mesh.positions.len() - 1) as u32
            });
        }
        mesh.faces.push(g);
    }
    copy_marks(mesh, &copy, &edges);
    Selection::from_faces(mesh, first..mesh.faces.len())
}

/// Separate (P): the selected faces taken out of this mesh into a new one (with their seams
/// and sharp edges); vertices left unused go from both.
pub fn separate(mesh: &mut Mesh, sel: &Selection) -> Mesh {
    let faces = sel_faces(mesh, sel);
    let mut out = Mesh::default();
    if faces.is_empty() {
        return out;
    }
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut edges = BTreeSet::new();
    for &f in &faces {
        let mut g = mesh.faces[f].clone();
        for (a, b) in g.edges() {
            edges.insert(edge_key(a, b));
        }
        for v in &mut g.verts {
            *v = *map.entry(*v).or_insert_with(|| out.add_vertex(mesh.positions[*v as usize]));
        }
        out.faces.push(g);
    }
    for &(a, b) in &edges {
        if mesh.is_seam(a, b) {
            out.set_seam(map[&a], map[&b], true);
        }
        if mesh.is_sharp(a, b) {
            out.set_sharp(map[&a], map[&b], true);
        }
    }
    let old = std::mem::take(&mut mesh.faces);
    mesh.faces = old.into_iter().enumerate().filter(|(i, _)| !faces.contains(i)).map(|(_, f)| f).collect();
    compact(mesh);
    out
}

/// Turn the selected faces round.
pub fn flip_normals(mesh: &mut Mesh, sel: &Selection) {
    for f in sel_faces(mesh, sel) {
        mesh.faces[f].reverse();
    }
}

/// Recalculate outside (Shift+N): the selected faces turned to agree with each other and face
/// out of the pieces they make.
pub fn recalculate_outside(mesh: &mut Mesh, sel: &Selection) {
    let faces: Vec<usize> = sel_faces(mesh, sel).into_iter().collect();
    let mut sub = Mesh { positions: mesh.positions.clone(), faces: faces.iter().map(|&f| mesh.faces[f].clone()).collect(), ..Default::default() };
    sub.recalculate_normals();
    for (k, f) in faces.into_iter().enumerate() {
        mesh.faces[f] = std::mem::take(&mut sub.faces[k]);
    }
}

/// Triangulate (Ctrl+T): the selected faces cut into triangles, which are selected after.
pub fn triangulate(mesh: &mut Mesh, sel: &Selection) -> Selection {
    let faces = sel_faces(mesh, sel);
    let mut have: HashSet<(u32, u32)> = mesh.edges().into_iter().collect();
    let mut made = Vec::new();
    for &f in &faces {
        let mut tris = mesh.triangulate_face(f);
        let src = mesh.faces[f].clone();
        // a quad cut across the other way where its diagonal is an edge already
        if let ([a, b], [w, x, y, z]) = (&tris[..], &src.verts[..]) {
            let diagonal: Vec<usize> = a.iter().copied().filter(|c| b.contains(c)).collect();
            let taken = |i: usize, j: usize| diagonal.contains(&i) && diagonal.contains(&j);
            if taken(0, 2) && have.contains(&edge_key(*w, *y)) {
                tris = vec![[1, 2, 3], [1, 3, 0]];
            } else if taken(1, 3) && have.contains(&edge_key(*x, *z)) {
                tris = vec![[0, 1, 2], [0, 2, 3]];
            }
        }
        for t in &tris {
            for k in 0..3 {
                have.insert(edge_key(src.verts[t[k]], src.verts[t[(k + 1) % 3]]));
            }
        }
        for (k, t) in tris.iter().enumerate() {
            let g = face_like(t.iter().map(|&c| src.verts[c]).collect(), t.iter().map(|&c| src.uvs[c]).collect(), &src);
            if k == 0 {
                mesh.faces[f] = g;
                made.push(f);
            } else {
                mesh.faces.push(g);
                made.push(mesh.faces.len() - 1);
            }
        }
    }
    Selection::from_faces(mesh, made)
}

/// Triangles to quads (Alt+J): pairs of selected triangles joined where they lie within
/// `max_angle_deg` of flat and make a convex quad, squarest pairs first. The resulting faces
/// are selected after.
pub fn tris_to_quads(mesh: &mut Mesh, sel: &Selection, max_angle_deg: f32) -> Selection {
    let faces = sel_faces(mesh, sel);
    let tris: BTreeSet<usize> = faces.iter().copied().filter(|&f| mesh.faces[f].verts.len() == 3).collect();
    let ef = mesh.edge_faces();
    let limit = max_angle_deg.to_radians();
    let mut pairs: Vec<(f32, usize, usize, Face)> = Vec::new();
    for (&(a, b), list) in &ef {
        let [f, g] = list[..] else { continue };
        if f == g || !tris.contains(&f) || !tris.contains(&g) {
            continue;
        }
        let (f, g) = if mesh.faces[f].edges().any(|e| e == (a, b)) { (f, g) } else { (g, f) };
        let (ff, gg) = (&mesh.faces[f], &mesh.faces[g]);
        let corner = |face: &Face, v: u32| face.verts.iter().position(|&x| x == v).unwrap_or(0);
        let other = |face: &Face| (0..3).find(|&i| face.verts[i] != a && face.verts[i] != b);
        let (Some(cf), Some(cg)) = (other(ff), other(gg)) else { continue };
        if !gg.edges().any(|e| e == (b, a)) {
            continue;
        }
        // f runs a, b, its third; g runs b, a, its third: the quad is a, g's third, b, f's third
        let verts = vec![a, gg.verts[cg], b, ff.verts[cf]];
        let uvs = vec![ff.uvs[corner(ff, a)], gg.uvs[cg], ff.uvs[corner(ff, b)], ff.uvs[cf]];
        let bend = mesh.face_normal(f).angle_between(mesh.face_normal(g));
        if bend.is_nan() || bend > limit + 1e-5 {
            continue;
        }
        let pts: Vec<Vec3> = verts.iter().map(|&v| mesh.positions[v as usize]).collect();
        let n = polygon_normal(&pts);
        let mut square = 0.0;
        let mut convex = n != Vec3::ZERO;
        for i in 0..4 {
            let (p, q, r) = (pts[(i + 3) % 4], pts[i], pts[(i + 1) % 4]);
            convex &= (q - p).cross(r - q).dot(n) > 1e-9;
            square += ((p - q).angle_between(r - q) - std::f32::consts::FRAC_PI_2).abs();
        }
        if convex {
            pairs.push((bend + square / 4.0, f, g, face_like(verts, uvs, ff)));
        }
    }
    pairs.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
    let mut used = HashSet::new();
    let mut gone = HashSet::new();
    let mut joined: Vec<usize> = Vec::new();
    for (_, f, g, quad) in pairs {
        if used.contains(&f) || used.contains(&g) {
            continue;
        }
        used.extend([f, g]);
        mesh.faces[f] = quad;
        gone.insert(g);
        joined.push(f);
    }
    let keep: Vec<usize> = faces.iter().copied().filter(|f| !gone.contains(f)).collect();
    let mut new_index = vec![usize::MAX; mesh.faces.len()];
    let old = std::mem::take(&mut mesh.faces);
    for (i, f) in old.into_iter().enumerate() {
        if !gone.contains(&i) {
            new_index[i] = mesh.faces.len();
            mesh.faces.push(f);
        }
    }
    Selection::from_faces(mesh, keep.into_iter().map(|f| new_index[f]).collect::<Vec<_>>())
}

/// Poke: each selected face into a fan of triangles round a new middle vertex; the triangles
/// are selected after.
pub fn poke(mesh: &mut Mesh, sel: &Selection) -> Selection {
    let faces = sel_faces(mesh, sel);
    let mut made = Vec::new();
    for &f in &faces {
        let src = mesh.faces[f].clone();
        let k = src.verts.len();
        let c = mesh.add_vertex(mesh.face_center(f));
        let cuv = src.uvs.iter().copied().sum::<Vec2>() / k as f32;
        for i in 0..k {
            let j = (i + 1) % k;
            let g = face_like(vec![src.verts[i], src.verts[j], c], vec![src.uvs[i], src.uvs[j], cuv], &src);
            if i == 0 {
                mesh.faces[f] = g;
                made.push(f);
            } else {
                mesh.faces.push(g);
                made.push(mesh.faces.len() - 1);
            }
        }
    }
    Selection::from_faces(mesh, made)
}

/// Smooth vertices: each selected vertex moved `factor` of the way to the middle of its
/// neighbours, `repeat` times (vertices on an open edge only along it).
pub fn smooth_vertices(mesh: &mut Mesh, sel: &Selection, factor: f32, repeat: u32) {
    let verts = sel_verts(mesh, sel);
    let ef = mesh.edge_faces();
    let nb = neighbours(mesh);
    let rim = |a: u32, b: u32| ef.get(&edge_key(a, b)).is_some_and(|l| l.len() == 1);
    let pull: Vec<(u32, Vec<u32>)> = verts
        .iter()
        .map(|&v| {
            let all = &nb[v as usize];
            let edge: Vec<u32> = all.iter().copied().filter(|&w| rim(v, w)).collect();
            (v, if edge.is_empty() { all.clone() } else { edge })
        })
        .filter(|(_, n)| !n.is_empty())
        .collect();
    for _ in 0..repeat.min(1000) {
        let old = mesh.positions.clone();
        for (v, n) in &pull {
            let mid = n.iter().map(|&w| old[w as usize]).sum::<Vec3>() / n.len() as f32;
            mesh.positions[*v as usize] = old[*v as usize].lerp(mid, factor);
        }
    }
}

/// Mirror (Ctrl+M): the selected vertices reflected through the plane through `center` across
/// `axis`; faces wholly selected turned round so they still face out.
pub fn mirror(mesh: &mut Mesh, sel: &Selection, axis: Axis, center: Vec3) {
    let m = Mat4::from_translation(center) * Mat4::from_scale(Vec3::ONE - 2.0 * axis.vec()) * Mat4::from_translation(-center);
    transform_selection(mesh, sel, m);
}

/// Shade the selected faces smooth or flat.
pub fn set_smooth(mesh: &mut Mesh, sel: &Selection, smooth: bool) {
    for f in sel_faces(mesh, sel) {
        mesh.faces[f].smooth = smooth;
    }
}

/// Give the selected faces a material.
pub fn set_material(mesh: &mut Mesh, sel: &Selection, material: u32) {
    for f in sel_faces(mesh, sel) {
        mesh.faces[f].material = material;
    }
}

/// Mark or clear the selected edges as UV seams.
pub fn mark_seam(mesh: &mut Mesh, sel: &Selection, on: bool) {
    for (a, b) in sel_edges(mesh, sel) {
        mesh.set_seam(a, b, on);
    }
}

/// Mark or clear the selected edges as sharp.
pub fn mark_sharp(mesh: &mut Mesh, sel: &Selection, on: bool) {
    for (a, b) in sel_edges(mesh, sel) {
        mesh.set_sharp(a, b, on);
    }
}

/// Split (Y): the selected faces let go of the rest, on vertices of their own where they
/// touched it; they stay selected.
pub fn split(mesh: &mut Mesh, sel: &Selection) -> Selection {
    let faces = sel_faces(mesh, sel);
    let shared: HashSet<u32> = mesh.faces.iter().enumerate().filter(|(i, _)| !faces.contains(i)).flat_map(|(_, f)| f.verts.iter().copied()).collect();
    let mut copy = BTreeMap::new();
    let mut edges = BTreeSet::new();
    for &f in &faces {
        for (a, b) in mesh.faces[f].edges() {
            edges.insert(edge_key(a, b));
        }
        for c in 0..mesh.faces[f].verts.len() {
            let v = mesh.faces[f].verts[c];
            if shared.contains(&v) {
                let p = mesh.positions[v as usize];
                let n = *copy.entry(v).or_insert_with(|| mesh.add_vertex(p));
                mesh.faces[f].verts[c] = n;
            }
        }
    }
    copy_marks(mesh, &copy, &edges);
    Selection::from_faces(mesh, faces)
}

/// Move the selected vertices by `m`; with a mirroring matrix the faces wholly selected are
/// turned round so they still face out.
pub fn transform_selection(mesh: &mut Mesh, sel: &Selection, m: Mat4) {
    let verts = sel_verts(mesh, sel);
    for &v in &verts {
        mesh.positions[v as usize] = m.transform_point3(mesh.positions[v as usize]);
    }
    if m.determinant() < 0.0 {
        for f in &mut mesh.faces {
            if f.verts.iter().all(|v| verts.contains(v)) {
                f.reverse();
            }
        }
    }
}

/// The middle of the selected vertices.
pub fn selection_center(mesh: &Mesh, sel: &Selection) -> Option<Vec3> {
    let verts = sel_verts(mesh, sel);
    (!verts.is_empty()).then(|| verts.iter().map(|&v| mesh.positions[v as usize]).sum::<Vec3>() / verts.len() as f32)
}

/// Which way the selection faces: its faces' area-weighted normal, or that of the faces round
/// its vertices; up when there is none.
pub fn selection_normal(mesh: &Mesh, sel: &Selection) -> Vec3 {
    let faces = sel_faces(mesh, sel);
    if !faces.is_empty() {
        return faces_normal(mesh, &faces);
    }
    let verts = sel_verts(mesh, sel);
    let vf = mesh.vertex_faces();
    let around: BTreeSet<usize> = verts.iter().flat_map(|&v| vf[v as usize].iter().copied()).collect();
    faces_normal(mesh, &around)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;

    type Op<'a> = Box<dyn Fn(&mut Mesh) -> Selection + 'a>;
    type Edit = dyn Fn(&mut Mesh);

    fn cube() -> Mesh {
        Primitive::Cube { size: Vec3::ONE }.build()
    }

    fn grid(n: u32) -> Mesh {
        Primitive::Grid { size: Vec2::splat(n as f32), x_div: n, y_div: n }.build()
    }

    fn volume(m: &Mesh) -> f32 {
        let t = m.triangles(30.0);
        t.indices.chunks(3).map(|c| t.positions[c[0] as usize].dot(t.positions[c[1] as usize].cross(t.positions[c[2] as usize]))).sum::<f32>() / 6.0
    }

    /// Closed, valid, every face agreeing with its neighbours (each edge run once each way).
    fn sound(m: &Mesh) -> bool {
        let mut runs: HashMap<(u32, u32), i32> = HashMap::new();
        for f in &m.faces {
            for e in f.edges() {
                *runs.entry(e).or_default() += 1;
            }
        }
        m.is_valid() && m.is_closed() && runs.iter().all(|(&(a, b), &k)| k == 1 && runs.get(&(b, a)) == Some(&1))
    }

    /// Valid, and no edge run twice the same way.
    fn consistent(m: &Mesh) -> bool {
        let mut seen = HashSet::new();
        m.is_valid() && m.faces.iter().all(|f| f.edges().all(|e| seen.insert(e)))
    }

    fn face_where(m: &Mesh, test: impl Fn(Vec3, Vec3) -> bool) -> usize {
        (0..m.faces.len()).find(|&f| test(m.face_center(f), m.face_normal(f))).unwrap()
    }

    fn top(m: &Mesh) -> usize {
        face_where(m, |_, n| n.z > 0.99)
    }

    fn verts_where(m: &Mesh, test: impl Fn(Vec3) -> bool) -> Vec<u32> {
        (0..m.positions.len() as u32).filter(|&v| test(m.positions[v as usize])).collect()
    }

    #[test]
    fn selections_follow_each_other() {
        let m = cube();
        let t = top(&m);
        let s = Selection::from_faces(&m, [t]);
        assert_eq!((s.verts.len(), s.edges.len(), s.faces.len()), (4, 4, 1));
        assert_eq!(Selection::from_verts(&m, s.verts.clone()), s);
        assert_eq!(Selection::from_edges(&m, s.edges.clone()), s);
        let all = Selection::all(&m);
        assert_eq!((all.verts.len(), all.edges.len(), all.faces.len()), (8, 12, 6));
        let mut s = Selection::default();
        s.toggle_face(&m, t);
        assert_eq!(s.faces.len(), 1);
        s.toggle_face(&m, t);
        assert!(s.is_empty());
        let e = m.edges()[0];
        s.toggle_edge(&m, e);
        assert_eq!(s.vertices(), vec![e.0, e.1]);
        s.toggle_vert(&m, e.0);
        assert!(s.edges.is_empty());
        let odd = Selection { verts: [99].into(), edges: [(0, 99)].into(), faces: [42].into() };
        assert!(Selection::from_verts(&m, odd.verts.clone()).is_empty());
        assert!(kept(&m, &odd).is_empty());
    }

    #[test]
    fn linked_more_less_and_invert() {
        let mut m = cube();
        let mut b = cube();
        b.transform(Mat4::from_translation(Vec3::X * 3.0));
        m.append(&b, 0);
        let s = select_linked(&m, &Selection::from_verts(&m, [0]));
        assert_eq!((s.verts.len(), s.faces.len()), (8, 6));
        assert!(s.verts.iter().all(|&v| v < 8));
        let g = grid(4);
        let centre = verts_where(&g, |p| p.length() < 1e-4);
        let more = select_more(&g, &Selection::from_verts(&g, centre.clone()));
        assert_eq!((more.verts.len(), more.faces.len()), (9, 4));
        let less = select_less(&g, &more);
        assert_eq!(less.vertices(), centre);
        let inv = invert(&g, &more, SelectMode::Face);
        assert_eq!(inv.faces.len(), 12);
        let inv = invert(&g, &more, SelectMode::Vertex);
        assert_eq!(inv.verts.len(), 25 - 9);
    }

    #[test]
    fn edge_loops_and_rings_on_a_grid() {
        let g = grid(4);
        let at = |x: f32, y: f32| verts_where(&g, |p| (p.x - x).abs() < 1e-4 && (p.y - y).abs() < 1e-4)[0];
        // a line across the middle: four edges from side to side
        let l = edge_loop(&g, (at(0.0, 0.0), at(1.0, 0.0)));
        assert_eq!(l.edges.len(), 4);
        assert!(l.verts.iter().all(|&v| g.positions[v as usize].y.abs() < 1e-4));
        // a boundary edge follows the whole boundary
        let l = edge_loop(&g, (at(-2.0, -2.0), at(-1.0, -2.0)));
        assert_eq!(l.edges.len(), 16);
        // the ring through a middle edge: the edges parallel to it up the column of quads
        let r = edge_ring(&g, (at(0.0, 0.0), at(1.0, 0.0)));
        assert_eq!(r.edges.len(), 5);
        assert!(r.edges.iter().all(|&(a, b)| (g.positions[a as usize].y - g.positions[b as usize].y).abs() < 1e-4));
    }

    #[test]
    fn a_loop_cut_round_a_cylinder_is_a_loop() {
        let mut m = Primitive::Cylinder { radius: 1.0, height: 2.0, segments: 16 }.build();
        let up = m.edges().into_iter().find(|&(a, b)| (m.positions[a as usize].z - m.positions[b as usize].z).abs() > 1.0).unwrap();
        let s = loop_cut(&mut m, up, 1, 0.0);
        assert_eq!(s.edges.len(), 16);
        assert!(sound(&m));
        assert!((volume(&m) - volume(&Primitive::Cylinder { radius: 1.0, height: 2.0, segments: 16 }.build())).abs() < 1e-4);
        let e = *s.edges.iter().next().unwrap();
        let l = edge_loop(&m, e);
        assert_eq!(l.edges.len(), 16);
        assert!(l.verts.iter().all(|&v| (m.positions[v as usize].z - 1.0).abs() < 1e-4));
        // the ring up across the two side quads stops at the caps
        let ring = edge_ring(&m, e);
        assert_eq!(ring.edges.len(), 3);
    }

    #[test]
    fn similar_material_and_path() {
        let mut m = cube();
        let t = top(&m);
        m.faces[t].material = 3;
        assert_eq!(select_by_material(&m, 3).faces, BTreeSet::from([t]));
        let s = select_similar_normal(&m, &Selection::from_faces(&m, [t]), 10.0);
        assert_eq!(s.faces.len(), 1);
        assert_eq!(select_similar_normal(&m, &Selection::from_faces(&m, [t]), 91.0).faces.len(), 5);
        let g = grid(4);
        let a = verts_where(&g, |p| (p - Vec3::new(-2.0, -2.0, 0.0)).length() < 1e-4)[0];
        let b = verts_where(&g, |p| (p - Vec3::new(2.0, -2.0, 0.0)).length() < 1e-4)[0];
        let p = shortest_path(&g, a, b);
        assert_eq!((p.verts.len(), p.edges.len()), (5, 4));
        assert_eq!(shortest_path(&g, a, 999), Selection::default());
    }

    #[test]
    fn extruding_a_cube_top_by_one_doubles_it() {
        let mut m = cube();
        let sel = Selection::from_faces(&m, [top(&m)]);
        let s = extrude(&mut m, &sel, SelectMode::Face);
        assert_eq!(m.faces.len(), 10);
        assert_eq!(s.faces.len(), 1);
        assert!(sound(&m));
        transform_selection(&mut m, &s, Mat4::from_translation(Vec3::Z));
        assert!((volume(&m) - 2.0).abs() < 1e-5);
        assert!(sound(&m));
        let mut n = cube();
        let sel = Selection::from_faces(&n, [top(&n)]);
        let s = extrude_along_normals(&mut n, &sel, 1.0);
        assert!((volume(&n) - 2.0).abs() < 1e-5 && sound(&n));
        assert!((selection_center(&n, &s).unwrap().z - 2.0).abs() < 1e-5);
        // the walls' UVs are a metre square each
        let wall = face_where(&n, |c, nn| nn.y < -0.99 && c.z > 1.0);
        let uv = &n.faces[wall].uvs;
        let span = uv.iter().fold(Vec2::ZERO, |a, b| a.max(*b)) - uv.iter().fold(Vec2::splat(9.0), |a, b| a.min(*b));
        assert!((span - Vec2::ONE).length() < 1e-4);
    }

    #[test]
    fn extruding_a_lone_plane_makes_a_box_and_regions_stay_one() {
        let mut m = Primitive::Plane { size: Vec2::ONE }.build();
        let sel = Selection::all(&m);
        let s = extrude_along_normals(&mut m, &sel, 1.0);
        assert_eq!(m.faces.len(), 6);
        assert!(sound(&m));
        assert!((volume(&m) - 1.0).abs() < 1e-5);
        assert_eq!(s.faces.len(), 1);
        // two neighbouring faces of a grid extrude as one region: 6 walls, 2 caps
        let mut g = grid(2);
        let s = Selection::from_faces(&g, [0, 1]);
        let out = extrude(&mut g, &s, SelectMode::Face);
        assert_eq!(g.faces.len(), 4 - 2 + 6 + 2);
        assert_eq!(out.faces.len(), 2);
        assert!(consistent(&g));
    }

    #[test]
    fn edge_and_individual_extrusions() {
        let mut m = Primitive::Plane { size: Vec2::ONE }.build();
        let e = m.edges()[0];
        let sel = Selection::from_edges(&m, [e]);
        let s = extrude(&mut m, &sel, SelectMode::Edge);
        assert_eq!(m.faces.len(), 2);
        assert_eq!(s.edges.len(), 1);
        assert!(consistent(&m));
        // vertices: only a boundary chain extrudes; a lone vertex stays
        let mut g = grid(2);
        let before = g.faces.len();
        let sel = Selection::from_verts(&g, [0]);
        let s = extrude(&mut g, &sel, SelectMode::Vertex);
        assert_eq!((g.faces.len(), s.vertices()), (before, vec![0]));
        let sel = Selection::from_verts(&g, [0, 1, 2]);
        let s = extrude(&mut g, &sel, SelectMode::Vertex);
        assert_eq!((g.faces.len(), s.edges.len()), (before + 2, 2));
        assert!(consistent(&g));
        // two faces of a cube each pushed out half a metre
        let mut c = cube();
        let front = face_where(&c, |_, n| n.y < -0.99);
        let sel = Selection::from_faces(&c, [top(&c), front]);
        let s = extrude_individual(&mut c, &sel, 0.5);
        assert_eq!(s.faces.len(), 2);
        assert!(sound(&c));
        assert!((volume(&c) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn inset_makes_a_ring_inside() {
        let mut m = Primitive::Plane { size: Vec2::ONE }.build();
        let sel = Selection::all(&m);
        let s = inset(&mut m, &sel, 0.1, 0.0, false);
        assert_eq!(m.faces.len(), 5);
        let inner = *s.faces.iter().next().unwrap();
        assert!((m.face_area(inner) - 0.64).abs() < 1e-5);
        assert!(consistent(&m));
        // UVs of the inner face continue the plane's
        let f = &m.faces[inner];
        for (v, uv) in f.verts.iter().zip(&f.uvs) {
            let p = m.positions[*v as usize];
            assert!((*uv - Vec2::new(p.x + 0.5, 0.5 - p.y)).length() < 1e-4);
        }
        // a region of two faces: one ring round both
        let mut g = grid(2);
        let sel = Selection::from_faces(&g, [0, 1]);
        let s = inset(&mut g, &sel, 0.1, 0.0, false);
        assert_eq!((g.faces.len(), s.faces.len()), (4 + 6, 2));
        assert!(consistent(&g));
        let mut g = grid(2);
        let sel = Selection::from_faces(&g, [0, 1]);
        inset(&mut g, &sel, 0.1, 0.0, true);
        assert_eq!(g.faces.len(), 4 + 8);
        assert!(consistent(&g));
        // the cube's top inset and pushed in stays closed
        let mut c = cube();
        let sel = Selection::from_faces(&c, [top(&c)]);
        let s = inset(&mut c, &sel, 0.1, -0.2, false);
        assert!(sound(&c));
        // a frustum 0.2 deep from the 1 x 1 top down to the 0.8 x 0.8 inner face taken out
        assert!((volume(&c) - (1.0 - 0.2 / 3.0 * (1.0 + 0.64 + 0.8))).abs() < 1e-4);
        assert!((selection_center(&c, &s).unwrap().z - 0.8).abs() < 1e-5);
    }

    #[test]
    fn bevelling_a_cube_edge_stays_closed() {
        for segs in 1..=8 {
            let mut m = cube();
            let e = m.edges().into_iter().find(|&(a, b)| {
                let (p, q) = (m.positions[a as usize], m.positions[b as usize]);
                p.z > 0.5 && q.z > 0.5 && p.y < 0.0 && q.y < 0.0
            });
            let sel = Selection::from_edges(&m, e);
            let s = bevel(&mut m, &sel, SelectMode::Edge, 0.2, segs);
            assert!(sound(&m), "segments {segs}");
            assert_eq!(m.faces.len(), 6 + segs as usize, "segments {segs}");
            assert_eq!(s.faces.len(), segs as usize);
            let v = volume(&m);
            assert!(v < 1.0 && v > 1.0 - 0.02 - 1e-5, "segments {segs}: {v}");
            if segs == 8 {
                // a round profile cuts off close to a quarter circle's corner
                assert!((v - (1.0 - 0.04 * (1.0 - std::f32::consts::FRAC_PI_4))).abs() < 1e-3, "{v}");
            }
        }
    }

    #[test]
    fn bevelling_every_cube_edge_and_corners() {
        for segs in 1..=4 {
            let mut m = cube();
            let sel = Selection::all(&m);
            let s = bevel(&mut m, &sel, SelectMode::Edge, 0.1, segs);
            assert!(sound(&m), "segments {segs}");
            assert_eq!(m.faces.len(), 6 + 12 * segs as usize + 8, "segments {segs}");
            assert_eq!(s.faces.len(), 12 * segs as usize + 8);
            assert!(volume(&m) < 1.0 && volume(&m) > 0.9);
        }
        // two edges of a corner
        let mut m = cube();
        let c = verts_where(&m, |p| p.z > 0.5 && p.x > 0.0 && p.y > 0.0)[0];
        let edges: Vec<(u32, u32)> = m.edges().into_iter().filter(|&(a, b)| (a == c || b == c) && (m.positions[a as usize].z - m.positions[b as usize].z).abs() < 0.5).collect();
        let sel = Selection::from_edges(&m, edges);
        bevel(&mut m, &sel, SelectMode::Edge, 0.1, 3);
        assert!(sound(&m));
        // a vertex cut off flat
        let mut m = cube();
        let sel = Selection::from_verts(&m, [c]);
        let s = bevel(&mut m, &sel, SelectMode::Vertex, 0.25, 1);
        assert!(sound(&m));
        assert_eq!((m.faces.len(), s.faces.len()), (7, 1));
        assert!((volume(&m) - (1.0 - 0.25f32.powi(3) / 6.0)).abs() < 1e-5);
        // a grid's boundary edge cannot be bevelled; nothing happens
        let mut g = grid(2);
        let before = g.clone();
        let sel = Selection::from_edges(&g, [g.edges()[0]]);
        bevel(&mut g, &sel, SelectMode::Edge, 0.1, 2);
        assert_eq!(g, before);
    }

    #[test]
    fn bevelling_an_edge_loop_of_a_grid() {
        let mut g = grid(4);
        let mid: Vec<(u32, u32)> = g.edges().into_iter().filter(|&(a, b)| g.positions[a as usize].y.abs() < 1e-4 && g.positions[b as usize].y.abs() < 1e-4).collect();
        let sel = Selection::from_edges(&g, mid);
        let s = bevel(&mut g, &sel, SelectMode::Edge, 0.2, 2);
        assert_eq!(s.faces.len(), 8);
        assert!(consistent(&g));
        let area: f32 = (0..g.faces.len()).map(|f| g.face_area(f)).sum();
        assert!((area - 16.0).abs() < 1e-4, "{area}");
    }

    #[test]
    fn loop_cuts_on_a_cube() {
        for cuts in 1..=3 {
            let mut m = cube();
            let e = m.edges()[0];
            let s = loop_cut(&mut m, e, cuts, 0.0);
            assert_eq!(m.faces.len(), 6 + 4 * cuts as usize);
            assert_eq!(s.edges.len(), 4 * cuts as usize);
            assert!(sound(&m));
            assert!((volume(&m) - 1.0).abs() < 1e-5);
        }
        // a slid cut on a strip of two quads; the triangle beyond it takes the new vertex
        let mut m = Mesh::default();
        for p in [[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [0.0, 1.0], [1.0, 1.0], [2.0, 1.0], [1.0, 2.0]] {
            m.add_vertex(Vec3::new(p[0], p[1], 0.0));
        }
        m.add_face(vec![0, 1, 4, 3], vec![Vec2::ZERO; 4], 0);
        m.add_face(vec![1, 2, 5, 4], vec![Vec2::ZERO; 4], 0);
        m.add_face(vec![3, 4, 6], vec![Vec2::ZERO; 3], 0);
        let strip = m.clone();
        let s = loop_cut(&mut m, (0, 1), 1, 0.5);
        assert_eq!(s.edges.len(), 1);
        assert!(s.verts.iter().all(|&v| (m.positions[v as usize].x - 0.75).abs() < 1e-5));
        assert_eq!(m.faces.iter().find(|f| f.verts.contains(&6)).unwrap().verts.len(), 4);
        assert!(consistent(&m));
        // across both quads
        let mut m = strip;
        let s = loop_cut(&mut m, (0, 3), 1, -0.5);
        assert_eq!((s.edges.len(), m.faces.len()), (2, 5));
        assert!(s.verts.iter().all(|&v| (m.positions[v as usize].y - 0.25).abs() < 1e-5));
        assert!(consistent(&m));
    }

    #[test]
    fn subdividing_keeps_neighbours_whole() {
        let mut m = cube();
        let sel = Selection::from_faces(&m, [top(&m)]);
        let s = subdivide(&mut m, &sel, 1);
        assert_eq!((m.faces.len(), s.faces.len()), (9, 4));
        assert!(sound(&m));
        assert!((volume(&m) - 1.0).abs() < 1e-5);
        assert_eq!(m.faces.iter().filter(|f| f.verts.len() == 5).count(), 4);
        let mut m = cube();
        let sel = Selection::all(&m);
        subdivide(&mut m, &sel, 2);
        assert_eq!(m.faces.len(), 54);
        assert!(sound(&m));
        // triangles and a polygon of more corners
        let mut p = Primitive::Pyramid { width: 1.0, depth: 1.0, height: 1.0 }.build();
        let sel = Selection::all(&p);
        let s = subdivide(&mut p, &sel, 2);
        assert_eq!(s.faces.len(), 4 * 9 + 9);
        assert!(sound(&p));
        let mut c = Primitive::Cylinder { radius: 1.0, height: 1.0, segments: 8 }.build();
        let cap = top(&c);
        let v = volume(&c);
        let sel = Selection::from_faces(&c, [cap]);
        subdivide(&mut c, &sel, 3);
        assert!(sound(&c));
        assert!((volume(&c) - v).abs() < 1e-4);
    }

    /// Whether every corner of a flat grid of `size` metres still has the UV of where it lies.
    fn uvs_follow_ground(m: &Mesh, size: f32) -> bool {
        m.faces.iter().all(|f| {
            f.verts.iter().zip(&f.uvs).all(|(&v, uv)| {
                let p = m.positions[v as usize];
                (*uv - Vec2::new(p.x + size / 2.0, size / 2.0 - p.y)).length() < 1e-4
            })
        })
    }

    #[test]
    fn new_corners_take_uvs_from_the_faces_they_split() {
        let mut g = grid(2);
        let sel = Selection::from_faces(&g, [0, 3]);
        subdivide(&mut g, &sel, 2);
        assert!(uvs_follow_ground(&g, 2.0));
        let mut g = grid(2);
        let sel = Selection::all(&g);
        knife_plane(&mut g, &sel, Vec3::new(0.3, 0.0, 0.0), Vec3::new(1.0, 0.4, 0.0));
        assert!(uvs_follow_ground(&g, 2.0));
        let mut g = grid(2);
        let e = g.edges()[0];
        loop_cut(&mut g, e, 3, 0.0);
        assert!(uvs_follow_ground(&g, 2.0));
        let mut g = grid(2);
        let sel = Selection::all(&g);
        poke(&mut g, &sel);
        let sel = Selection::from_faces(&g, [0, 1, 2]);
        inset(&mut g, &sel, 0.05, 0.0, false);
        assert!(uvs_follow_ground(&g, 2.0));
        let mut g = grid(2);
        let c = verts_where(&g, |p| p.length() < 1e-4);
        let sel = Selection::from_verts(&g, c);
        let cap = bevel(&mut g, &sel, SelectMode::Vertex, 0.3, 1);
        assert_eq!((g.faces.len(), cap.faces.len()), (5, 1));
        g.faces.retain(|f| f.verts.len() == 5);
        assert!(uvs_follow_ground(&g, 2.0));
        let mut g = grid(2);
        let s = Selection::from_verts(&g, [0, 4]);
        connect(&mut g, &s);
        assert!(uvs_follow_ground(&g, 2.0));
    }

    #[test]
    fn merging() {
        let mut m = cube();
        let tops = verts_where(&m, |p| p.z > 0.5);
        let sel = Selection::from_verts(&m, tops);
        let s = merge(&mut m, &sel, MergeAt::Center);
        assert_eq!((m.faces.len(), m.positions.len(), s.verts.len()), (5, 5, 1));
        assert!(sound(&m));
        assert!((volume(&m) - 1.0 / 3.0).abs() < 1e-5);
        // two edges far apart collapse to a vertex each
        let mut g = grid(4);
        let a = verts_where(&g, |p| p.y < -1.9 && p.x < -0.9);
        let b = verts_where(&g, |p| p.y > 1.9 && p.x > 0.9);
        assert_eq!((a.len(), b.len()), (2, 2));
        let sel = Selection::from_verts(&g, a.into_iter().chain(b));
        let s = merge(&mut g, &sel, MergeAt::Collapse);
        assert_eq!((s.verts.len(), g.positions.len()), (2, 23));
        assert!(consistent(&g));
        let mut g = grid(2);
        let sel = Selection::from_verts(&g, [0, 1]);
        let s = merge(&mut g, &sel, MergeAt::Cursor(Vec3::new(5.0, 5.0, 5.0)));
        assert_eq!(g.positions[s.vertices()[0] as usize], Vec3::splat(5.0));
        // two quads side by side, not joined, welded where they touch
        let mut m = Primitive::Plane { size: Vec2::ONE }.build();
        let mut b = m.clone();
        b.transform(Mat4::from_translation(Vec3::X));
        m.append(&b, 0);
        let sel = Selection::all(&m);
        assert_eq!(merge_by_distance(&mut m, &sel, 1e-4), 2);
        assert_eq!((m.positions.len(), m.edges().len()), (6, 7));
        assert!(consistent(&m));
    }

    #[test]
    fn dissolving() {
        let mut g = grid(2);
        let c = verts_where(&g, |p| p.length() < 1e-4);
        let sel = Selection::from_verts(&g, c);
        let s = dissolve_vertices(&mut g, &sel);
        assert_eq!((g.faces.len(), g.faces[0].verts.len(), s.faces.len()), (1, 8, 1));
        assert!(g.face_normal(0).z > 0.99);
        // a vertex on the boundary joins its two faces
        let mut g = grid(2);
        let side = verts_where(&g, |p| p.x.abs() < 1e-4 && p.y < -0.9);
        let sel = Selection::from_verts(&g, side);
        dissolve_vertices(&mut g, &sel);
        assert_eq!((g.faces.len(), g.positions.len()), (3, 8));
        assert!(consistent(&g));
        // a vertex between just two edges only leaves the face
        let mut q = grid(1);
        let mid = q.add_vertex(Vec3::new(0.0, -0.5, 0.0));
        q.faces[0].verts.insert(1, mid);
        q.faces[0].uvs.insert(1, Vec2::ZERO);
        let sel = Selection::from_verts(&q, [mid]);
        dissolve_vertices(&mut q, &sel);
        assert_eq!((q.faces[0].verts.len(), q.positions.len()), (4, 4));
        // a cube's edge: its two faces become one, its ends go
        let mut m = cube();
        let e = m.edges()[0];
        let sel = Selection::from_edges(&m, [e]);
        let s = dissolve_edges(&mut m, &sel);
        assert_eq!((m.faces.len(), m.positions.len(), s.faces.len()), (5, 6, 1));
        assert!(sound(&m));
        let mut g = grid(2);
        let mid: Vec<(u32, u32)> = g.edges().into_iter().filter(|&(a, b)| g.positions[a as usize].x.abs() < 1e-4 && g.positions[b as usize].x.abs() < 1e-4).collect();
        let sel = Selection::from_edges(&g, mid);
        dissolve_edges(&mut g, &sel);
        assert_eq!(g.faces.len(), 2);
        assert!(consistent(&g));
        // all faces of a grid into one
        let mut g = grid(3);
        let sel = Selection::all(&g);
        let s = dissolve_faces(&mut g, &sel);
        assert_eq!((g.faces.len(), g.faces[0].verts.len(), s.faces.len()), (1, 12, 1));
        assert!(g.face_normal(0).z > 0.99);
        assert!((g.face_area(0) - 9.0).abs() < 1e-4);
        // the cube's top and front into one bent face
        let mut m = cube();
        let front = face_where(&m, |_, n| n.y < -0.99);
        let sel = Selection::from_faces(&m, [top(&m), front]);
        dissolve_faces(&mut m, &sel);
        assert_eq!(m.faces.len(), 5);
        assert!(sound(&m));
    }

    #[test]
    fn deleting() {
        let mut m = cube();
        let sel = Selection::from_faces(&m, [top(&m)]);
        assert!(delete(&mut m, &sel, Delete::Faces).is_empty());
        assert_eq!((m.faces.len(), m.positions.len()), (5, 8));
        let mut m = cube();
        let sel = Selection::from_verts(&m, [0]);
        delete(&mut m, &sel, Delete::Vertices);
        assert_eq!((m.faces.len(), m.positions.len()), (3, 7));
        assert!(consistent(&m));
        let mut m = cube();
        let sel = Selection::from_edges(&m, [m.edges()[0]]);
        delete(&mut m, &sel, Delete::Edges);
        assert_eq!((m.faces.len(), m.positions.len()), (4, 8));
        let mut g = grid(1);
        let sel = Selection::all(&g);
        let s = delete(&mut g, &sel, Delete::OnlyFaces);
        assert_eq!((g.faces.len(), g.positions.len(), s.verts.len()), (0, 4, 4));
    }

    #[test]
    fn filling_closes_a_hole() {
        let mut m = cube();
        let sel = Selection::from_faces(&m, [top(&m)]);
        delete(&mut m, &sel, Delete::Faces);
        assert!(!m.is_closed());
        let rim = verts_where(&m, |p| p.z > 0.5);
        let sel = Selection::from_verts(&m, rim.clone());
        let s = fill(&mut m, &sel);
        assert_eq!(s.faces.len(), 1);
        assert!(sound(&m));
        assert!((volume(&m) - 1.0).abs() < 1e-5);
        // the same from the vertices alone (no edges between them selected)
        let mut m = cube();
        let sel = Selection::from_faces(&m, [top(&m)]);
        delete(&mut m, &sel, Delete::Faces);
        let sel = Selection { verts: rim.into_iter().collect(), ..Default::default() };
        let mut sel = sel;
        sel.edges.clear();
        fill(&mut m, &Selection { edges: [(99, 100)].into(), ..sel });
        assert!(sound(&m));
        // two vertices of one face: joined across it
        let mut g = grid(1);
        let sel = Selection::from_verts(&g, [0, 3]);
        fill(&mut g, &sel);
        assert_eq!(g.faces.len(), 2);
    }

    #[test]
    fn connecting_corners() {
        let mut g = grid(1);
        let sel = Selection::from_verts(&g, [0, 3]);
        let s = connect(&mut g, &sel);
        assert_eq!((g.faces.len(), s.edges.len()), (2, 1));
        assert!(g.faces.iter().all(|f| f.verts.len() == 3));
        assert!(consistent(&g));
        let mut m = cube();
        let before = m.clone();
        let sel = Selection::from_faces(&m, [top(&m)]);
        connect(&mut m, &sel);
        assert_eq!(m, before);
    }

    #[test]
    fn bridging_two_circles_makes_a_tube() {
        let mut m = Primitive::Circle { radius: 1.0, segments: 16 }.build();
        let mut b = m.clone();
        b.transform(Mat4::from_translation(Vec3::Z * 2.0));
        m.append(&b, 0);
        let sel = Selection::all(&m);
        let s = bridge_loops(&mut m, &sel);
        assert_eq!((m.faces.len(), s.faces.len()), (16, 16));
        assert!(m.faces.iter().all(|f| f.verts.len() == 4));
        assert!(consistent(&m));
        for f in 0..m.faces.len() {
            let c = m.face_center(f);
            assert!(m.face_normal(f).dot(Vec3::new(c.x, c.y, 0.0)) > 0.9, "face {f} looks in");
        }
        // two cubes' facing sides: one closed shape
        let mut m = cube();
        let mut b = cube();
        b.transform(Mat4::from_translation(Vec3::Z * 2.0));
        m.append(&b, 0);
        let up = face_where(&m, |c, n| n.z > 0.99 && c.z < 1.5);
        let down = face_where(&m, |c, n| n.z < -0.99 && c.z > 1.5);
        let sel = Selection::from_faces(&m, [up, down]);
        let s = bridge_loops(&mut m, &sel);
        assert_eq!(s.faces.len(), 4);
        assert!(sound(&m));
        assert!((volume(&m) - 3.0).abs() < 1e-5);
        // loops of 8 and 4 vertices: quads and triangles
        let mut m = Primitive::Circle { radius: 1.0, segments: 8 }.build();
        let mut b = Primitive::Circle { radius: 0.5, segments: 4 }.build();
        b.transform(Mat4::from_translation(Vec3::Z));
        m.append(&b, 0);
        let sel = Selection::all(&m);
        let s = bridge_loops(&mut m, &sel);
        assert!(!s.faces.is_empty());
        assert!(consistent(&m));
        assert_eq!(m.edge_faces().values().filter(|l| l.len() == 1).count(), 12);
    }

    #[test]
    fn bisecting_a_cube() {
        let mut m = cube();
        let sel = Selection::all(&m);
        let s = knife_plane(&mut m, &sel, Vec3::new(0.0, 0.0, 0.5), Vec3::Z);
        assert_eq!((m.faces.len(), s.edges.len()), (10, 4));
        assert!(s.verts.iter().all(|&v| (m.positions[v as usize].z - 0.5).abs() < 1e-5));
        assert!(sound(&m));
        assert!((volume(&m) - 1.0).abs() < 1e-5);
        // only the top face selected: its neighbours take the new vertices, nothing cracks
        let mut m = cube();
        let sel = Selection::from_faces(&m, [top(&m)]);
        let s = knife_plane(&mut m, &sel, Vec3::ZERO, Vec3::X);
        assert_eq!((m.faces.len(), s.edges.len()), (7, 1));
        assert!(sound(&m));
    }

    #[test]
    fn spinning_a_profile() {
        // a square in the xz plane facing -y, spun round z
        let mut m = Mesh::default();
        for p in [[1.0, 0.0], [2.0, 0.0], [2.0, 1.0], [1.0, 1.0]] {
            m.add_vertex(Vec3::new(p[0], 0.0, p[1]));
        }
        m.add_face(vec![0, 1, 2, 3], vec![Vec2::ZERO; 4], 0);
        let profile = m.clone();
        let sel = Selection::all(&m);
        let s = spin(&mut m, &sel, Vec3::Z, Vec3::ZERO, 360.0, 16);
        assert_eq!((m.faces.len(), m.positions.len(), s.faces.len()), (64, 64, 64));
        assert!(sound(&m));
        let ring = 16.0 / 2.0 * (std::f32::consts::TAU / 16.0).sin();
        assert!((volume(&m) - ring * (4.0 - 1.0)).abs() < 1e-3, "{}", volume(&m));
        // a quarter turn the other way: capped at both ends
        let mut m = profile.clone();
        let sel = Selection::all(&m);
        let s = spin(&mut m, &sel, Vec3::Z, Vec3::ZERO, -90.0, 4);
        assert_eq!(s.faces.len(), 1);
        assert!(sound(&m));
        assert!(volume(&m) > 0.0);
        // edges: a band
        let mut m = profile;
        let e = m.edges()[0];
        let sel = Selection::from_edges(&m, [e]);
        let s = spin(&mut m, &sel, Vec3::Z, Vec3::ZERO, 90.0, 3);
        assert_eq!((m.faces.len(), s.edges.len()), (4, 1));
        assert!(consistent(&m));
    }

    #[test]
    fn duplicate_separate_and_split() {
        let mut m = cube();
        let t = top(&m);
        let sel = Selection::from_faces(&m, [t]);
        let s = duplicate(&mut m, &sel);
        assert_eq!((m.faces.len(), m.positions.len(), s.verts.len()), (7, 12, 4));
        assert!(s.verts.iter().all(|&v| v >= 8));
        let mut m = cube();
        let mut b = cube();
        b.transform(Mat4::from_translation(Vec3::X * 3.0));
        m.append(&b, 0);
        m.set_seam(8, 9, true);
        let far = select_linked(&m, &Selection::from_verts(&m, [8]));
        let out = separate(&mut m, &far);
        assert_eq!((m.faces.len(), m.positions.len(), out.faces.len(), out.positions.len()), (6, 8, 6, 8));
        assert!(sound(&m) && sound(&out));
        assert!(m.seams.is_empty() && out.seams.len() == 1);
        let mut m = cube();
        let t = top(&m);
        let sel = Selection::from_faces(&m, [t]);
        let s = split(&mut m, &sel);
        assert_eq!((m.positions.len(), s.faces.len()), (12, 1));
        assert!(!m.is_closed());
        assert!(consistent(&m));
    }

    #[test]
    fn triangles_and_quads_round_trip() {
        let mut g = grid(3);
        let sel = Selection::all(&g);
        let s = triangulate(&mut g, &sel);
        assert_eq!((g.faces.len(), s.faces.len()), (18, 18));
        assert!(consistent(&g));
        let sel = Selection::all(&g);
        let s = tris_to_quads(&mut g, &sel, 40.0);
        assert_eq!((g.faces.len(), s.faces.len()), (9, 9));
        assert!(g.faces.iter().all(|f| f.verts.len() == 4));
        assert!(consistent(&g));
        assert!((0..9).all(|f| (g.face_area(f) - 1.0).abs() < 1e-5));
        // a cube round trip stays closed
        let mut m = cube();
        let sel = Selection::all(&m);
        triangulate(&mut m, &sel);
        let sel = Selection::all(&m);
        tris_to_quads(&mut m, &sel, 40.0);
        assert_eq!(m.faces.len(), 6);
        assert!(sound(&m));
    }

    #[test]
    fn poke_smooth_flip_and_recalculate() {
        let mut m = cube();
        let sel = Selection::from_faces(&m, [top(&m)]);
        let s = poke(&mut m, &sel);
        assert_eq!((m.faces.len(), s.faces.len()), (9, 4));
        assert!(sound(&m));
        let mut g = grid(2);
        let c = verts_where(&g, |p| p.length() < 1e-4)[0];
        g.positions[c as usize].z = 1.0;
        let sel = Selection::from_verts(&g, [c]);
        smooth_vertices(&mut g, &sel, 0.5, 2);
        assert!((g.positions[c as usize].z - 0.25).abs() < 1e-5);
        let mut m = cube();
        let sel = Selection::from_faces(&m, [0, 2]);
        flip_normals(&mut m, &sel);
        assert!(!sound(&m));
        let sel = Selection::all(&m);
        recalculate_outside(&mut m, &sel);
        assert!(sound(&m) && volume(&m) > 0.0);
    }

    #[test]
    fn mirror_attributes_and_marks() {
        let mut m = cube();
        let sel = Selection::all(&m);
        mirror(&mut m, &sel, Axis::X, Vec3::new(2.0, 0.0, 0.0));
        let (lo, hi) = m.bounds().unwrap();
        assert!((lo.x - 3.5).abs() < 1e-5 && (hi.x - 4.5).abs() < 1e-5);
        assert!(sound(&m) && (volume(&m) - 1.0).abs() < 1e-5);
        let s = Selection::from_faces(&m, [top(&m)]);
        set_smooth(&mut m, &s, true);
        set_material(&mut m, &s, 2);
        assert!(m.faces[top(&m)].smooth && m.faces[top(&m)].material == 2);
        mark_seam(&mut m, &s, true);
        mark_sharp(&mut m, &s, true);
        assert_eq!((m.seams.len(), m.sharp.len()), (4, 4));
        // marks carried through a loop cut across them
        let e = *s.edges.iter().next().unwrap();
        loop_cut(&mut m, e, 1, 0.0);
        assert_eq!(m.seams.len(), 6);
        let sel = Selection::all(&m);
        mark_seam(&mut m, &sel, false);
        assert!(m.seams.is_empty());
        assert!(selection_normal(&m, &Selection::from_faces(&m, [top(&m)])).z > 0.99);
        assert_eq!(selection_center(&m, &Selection::default()), None);
        assert_eq!(selection_normal(&m, &Selection::default()), Vec3::Z);
    }

    #[test]
    fn nothing_breaks_on_any_selection() {
        let odd = Selection { verts: [0, 3, 1000].into(), edges: [(0, 1000), (2, 1)].into(), faces: [1, 77].into() };
        let picks = [Selection::default(), odd];
        let shapes = [
            cube(),
            grid(2),
            Primitive::IcoSphere { radius: 1.0, subdivisions: 1 }.build(),
            Primitive::Torus { major: 1.0, minor: 0.3, major_segments: 8, minor_segments: 6 }.build(),
            Mesh::default(),
        ];
        for shape in &shapes {
            for sel in &picks {
                let ops: Vec<Op> = vec![
                    Box::new(|m| extrude(m, sel, SelectMode::Vertex)),
                    Box::new(|m| extrude(m, sel, SelectMode::Edge)),
                    Box::new(|m| extrude_along_normals(m, sel, 0.3)),
                    Box::new(|m| extrude_individual(m, sel, 0.3)),
                    Box::new(|m| inset(m, sel, 0.05, 0.1, false)),
                    Box::new(|m| inset(m, sel, 0.05, 0.0, true)),
                    Box::new(|m| bevel(m, sel, SelectMode::Edge, 0.1, 3)),
                    Box::new(|m| bevel(m, sel, SelectMode::Vertex, 0.1, 1)),
                    Box::new(|m| loop_cut(m, (0, 1), 2, 0.0)),
                    Box::new(|m| loop_cut(m, (5000, 1), 1, 0.0)),
                    Box::new(|m| subdivide(m, sel, 2)),
                    Box::new(|m| merge(m, sel, MergeAt::Center)),
                    Box::new(|m| merge(m, sel, MergeAt::Collapse)),
                    Box::new(|m| {
                        merge_by_distance(m, sel, 0.5);
                        Selection::default()
                    }),
                    Box::new(|m| dissolve_vertices(m, sel)),
                    Box::new(|m| dissolve_edges(m, sel)),
                    Box::new(|m| dissolve_faces(m, sel)),
                    Box::new(|m| delete(m, sel, Delete::Vertices)),
                    Box::new(|m| delete(m, sel, Delete::OnlyFaces)),
                    Box::new(|m| fill(m, sel)),
                    Box::new(|m| connect(m, sel)),
                    Box::new(|m| bridge_loops(m, sel)),
                    Box::new(|m| knife_plane(m, sel, Vec3::new(0.1, 0.2, 0.3), Vec3::new(1.0, 1.0, 0.5))),
                    Box::new(|m| spin(m, sel, Vec3::Z, Vec3::ZERO, 120.0, 3)),
                    Box::new(|m| duplicate(m, sel)),
                    Box::new(|m| {
                        separate(m, sel);
                        Selection::default()
                    }),
                    Box::new(|m| triangulate(m, sel)),
                    Box::new(|m| tris_to_quads(m, sel, 40.0)),
                    Box::new(|m| poke(m, sel)),
                    Box::new(|m| split(m, sel)),
                    Box::new(|m| select_linked(m, sel)),
                    Box::new(|m| select_more(m, sel)),
                    Box::new(|m| select_less(m, sel)),
                    Box::new(|m| invert(m, sel, SelectMode::Edge)),
                    Box::new(|m| edge_loop(m, (0, 1))),
                    Box::new(|m| edge_ring(m, (0, 1))),
                    Box::new(|m| shortest_path(m, 0, 3)),
                    Box::new(|m| {
                        smooth_vertices(m, sel, 0.5, 3);
                        mirror(m, sel, Axis::Y, Vec3::ZERO);
                        flip_normals(m, sel);
                        recalculate_outside(m, sel);
                        mark_seam(m, sel, true);
                        Selection::default()
                    }),
                ];
                for (k, op) in ops.iter().enumerate() {
                    let mut m = shape.clone();
                    let out = op(&mut m);
                    assert!(m.is_valid(), "op {k}");
                    assert_eq!(kept(&m, &out), out, "op {k} selects what is not there");
                }
            }
        }
    }

    #[test]
    fn operations_on_whole_solids_keep_them_sound() {
        let shapes = [
            cube(),
            Primitive::IcoSphere { radius: 1.0, subdivisions: 1 }.build(),
            Primitive::Torus { major: 1.0, minor: 0.3, major_segments: 8, minor_segments: 6 }.build(),
            Primitive::Cylinder { radius: 0.5, height: 1.0, segments: 8 }.build(),
        ];
        for shape in &shapes {
            let ops: Vec<Box<Edit>> = vec![
                Box::new(|m| {
                    let sel = Selection::from_faces(m, [0, 1]);
                    extrude_along_normals(m, &sel, 0.2);
                }),
                Box::new(|m| {
                    let sel = Selection::from_faces(m, [0, 1]);
                    extrude_individual(m, &sel, 0.2);
                }),
                Box::new(|m| {
                    let sel = Selection::from_faces(m, [0, 1]);
                    inset(m, &sel, 0.02, 0.05, false);
                }),
                Box::new(|m| {
                    let sel = Selection::from_faces(m, [0]);
                    inset(m, &sel, 0.02, 0.05, true);
                }),
                Box::new(|m| {
                    let sel = Selection::from_faces(m, [0]);
                    bevel(m, &sel, SelectMode::Edge, 0.05, 2);
                }),
                Box::new(|m| {
                    let sel = Selection::from_verts(m, [0, 1]);
                    bevel(m, &sel, SelectMode::Vertex, 0.05, 1);
                }),
                Box::new(|m| {
                    let sel = Selection::from_faces(m, [0, 1]);
                    subdivide(m, &sel, 2);
                }),
                Box::new(|m| {
                    let sel = Selection::all(m);
                    knife_plane(m, &sel, Vec3::new(0.01, 0.02, 0.4), Vec3::new(0.3, 0.2, 1.0));
                }),
                Box::new(|m| {
                    let sel = Selection::all(m);
                    triangulate(m, &sel);
                    let sel = Selection::all(m);
                    tris_to_quads(m, &sel, 40.0);
                }),
                Box::new(|m| {
                    let sel = Selection::from_faces(m, [0]);
                    poke(m, &sel);
                }),
                Box::new(|m| {
                    let e = m.edges()[0];
                    loop_cut(m, e, 2, 0.0);
                }),
            ];
            for (k, op) in ops.iter().enumerate() {
                let mut m = shape.clone();
                op(&mut m);
                assert!(sound(&m), "op {k} on a shape of {} faces", shape.faces.len());
                assert!(volume(&m) > 0.0, "op {k}");
            }
        }
    }

    /// Whether a shape is free of the oddities that operations cannot be expected to keep
    /// sound: faces of no area, two faces on the same corners or sharing more than one edge,
    /// faces folded back onto each other.
    fn plain(m: &Mesh) -> bool {
        let ef = m.edge_faces();
        let mut pairs = HashSet::new();
        let corners: HashSet<Vec<u32>> = m
            .faces
            .iter()
            .map(|f| {
                let mut v = f.verts.clone();
                v.sort_unstable();
                v
            })
            .collect();
        corners.len() == m.faces.len()
            && (0..m.faces.len()).all(|f| m.face_area(f) > 1e-6)
            && ef.values().all(|l| l.len() <= 2)
            && ef.values().filter(|l| l.len() == 2).all(|l| pairs.insert((l[0].min(l[1]), l[0].max(l[1]))) && m.face_normal(l[0]).dot(m.face_normal(l[1])) > -0.9)
    }

    #[test]
    fn random_edits_keep_meshes_sound() {
        let mut seed = 777u64;
        let mut rnd = move |n: usize| -> usize {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % n.max(1)
        };
        let shapes = [
            cube(),
            grid(3),
            Primitive::IcoSphere { radius: 1.0, subdivisions: 1 }.build(),
            Primitive::Torus { major: 1.0, minor: 0.3, major_segments: 8, minor_segments: 6 }.build(),
            Primitive::Cylinder { radius: 0.5, height: 1.0, segments: 8 }.build(),
            Primitive::Tube { outer: 1.0, inner: 0.5, height: 1.0, segments: 6 }.build(),
            Primitive::Stairs { width: 1.2, depth: 2.4, height: 1.6, steps: 3 }.build(),
            Primitive::Arch { width: 2.0, height: 2.5, depth: 0.5, thickness: 0.3, segments: 4 }.build(),
            Primitive::Pyramid { width: 1.0, depth: 1.0, height: 1.0 }.build(),
        ];
        for run in 0..1500 {
            let mut m = shapes[rnd(shapes.len())].clone();
            for step in 0..3 {
                let edges = m.edges();
                let sel = match rnd(4) {
                    0 => Selection::from_faces(&m, (0..1 + rnd(3)).map(|_| rnd(m.faces.len())).collect::<Vec<_>>()),
                    1 => Selection::from_verts(&m, (0..1 + rnd(4)).map(|_| rnd(m.positions.len()) as u32).collect::<Vec<_>>()),
                    2 => Selection::from_edges(&m, (0..1 + rnd(4)).filter_map(|_| edges.get(rnd(edges.len())).copied()).collect::<Vec<_>>()),
                    _ => edges.get(rnd(edges.len())).map(|&e| edge_loop(&m, e)).unwrap_or_default(),
                };
                let ordinary = plain(&m);
                let (agreed, closed) = (consistent(&m), sound(&m));
                let w = [0.05, 0.1, 0.3][rnd(3)];
                let op = rnd(24);
                let out = match op {
                    0 => {
                        let s = extrude(&mut m, &sel, SelectMode::Face);
                        transform_selection(&mut m, &s, Mat4::from_translation(Vec3::new(0.1, 0.2, 0.3)));
                        s
                    }
                    1 => extrude_along_normals(&mut m, &sel, w),
                    2 => extrude_individual(&mut m, &sel, w),
                    3 => inset(&mut m, &sel, w * 0.3, w, false),
                    4 => inset(&mut m, &sel, w * 0.3, 0.0, true),
                    5 => bevel(&mut m, &sel, SelectMode::Edge, w, 1 + rnd(5) as u32),
                    6 => bevel(&mut m, &sel, SelectMode::Vertex, w, 1),
                    7 => match edges.get(rnd(edges.len())) {
                        Some(&e) => loop_cut(&mut m, e, 1 + rnd(3) as u32, 0.3),
                        None => Selection::default(),
                    },
                    8 => subdivide(&mut m, &sel, 1 + rnd(3) as u32),
                    9 => merge(&mut m, &sel, MergeAt::Center),
                    10 => merge(&mut m, &sel, MergeAt::Collapse),
                    11 => dissolve_vertices(&mut m, &sel),
                    12 => dissolve_edges(&mut m, &sel),
                    13 => dissolve_faces(&mut m, &sel),
                    14 => knife_plane(&mut m, &sel, Vec3::new(0.01, 0.02, 0.33), Vec3::new(0.2, 0.1, 1.0)),
                    15 => triangulate(&mut m, &sel),
                    16 => tris_to_quads(&mut m, &sel, 40.0),
                    17 => poke(&mut m, &sel),
                    18 => connect(&mut m, &sel),
                    19 => split(&mut m, &sel),
                    20 => spin(&mut m, &sel, Vec3::Z, Vec3::new(3.0, 0.0, 0.0), 90.0, 3),
                    21 => {
                        let s = delete(&mut m, &sel, Delete::Faces);
                        let all = Selection::all(&m);
                        fill(&mut m, &all);
                        s
                    }
                    22 => bridge_loops(&mut m, &sel),
                    _ => duplicate(&mut m, &sel),
                };
                let at = format!("run {run} step {step} op {op}");
                assert!(m.is_valid(), "{at}");
                assert_eq!(kept(&m, &out), out, "{at}");
                // (edges pulled out of a closed surface make fins; a polygon far from flat may
                // triangulate across an edge it has)
                let by_faces = !sel.faces.is_empty();
                if ordinary && agreed && (matches!(op, 2..=8 | 11..=14 | 16..=19 | 21 | 23) || (op < 2 && by_faces)) {
                    assert!(consistent(&m), "{at} turned faces against each other");
                }
                if ordinary && closed && (matches!(op, 2..=8 | 14 | 16..=18) || (op < 2 && by_faces)) {
                    assert!(sound(&m), "{at} opened a closed shape");
                }
            }
        }
    }
}

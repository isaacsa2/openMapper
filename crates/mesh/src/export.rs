//! Writing meshes out: OMSI's own `.x` and `.o3d`, and the formats other tools read (OBJ,
//! glTF / GLB, FBX, Collada, USD, STL, PLY, 3MF, OFF).
//!
//! Every format gets the objects where their transforms put them, in metres. OBJ and glTF are
//! y-up, so the map's frame is turned for them (x, z, -y); the others keep z up and say so
//! where the format can; `.x` and `.o3d` are in Direct3D's frame as OMSI reads them.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use glam::{Mat4, Vec2, Vec3};
use serde_json::{json, Value};

use crate::{edge_key, Material, Mesh, TriMesh};

/// How far apart smooth faces may turn and still share their normals (degrees).
const SMOOTH_ANGLE: f32 = 60.0;

/// The maker the files name.
const MAKER: &str = "openMapper";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Obj,
    Gltf,
    Glb,
    Fbx,
    Dae,
    Usda,
    Stl,
    Ply,
    ThreeMf,
    Off,
    X,
    O3d,
}

impl Format {
    pub const ALL: [Format; 12] = [Format::Obj, Format::Gltf, Format::Glb, Format::Fbx, Format::Dae, Format::Usda, Format::Stl, Format::Ply, Format::ThreeMf, Format::Off, Format::X, Format::O3d];

    pub fn extension(self) -> &'static str {
        match self {
            Format::Obj => "obj",
            Format::Gltf => "gltf",
            Format::Glb => "glb",
            Format::Fbx => "fbx",
            Format::Dae => "dae",
            Format::Usda => "usda",
            Format::Stl => "stl",
            Format::Ply => "ply",
            Format::ThreeMf => "3mf",
            Format::Off => "off",
            Format::X => "x",
            Format::O3d => "o3d",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Obj => "Wavefront OBJ (.obj + .mtl)",
            Format::Gltf => "glTF 2.0 (.gltf + .bin)",
            Format::Glb => "glTF 2.0 binary (.glb)",
            Format::Fbx => "Autodesk FBX (.fbx)",
            Format::Dae => "Collada (.dae)",
            Format::Usda => "Universal Scene Description (.usda)",
            Format::Stl => "STL (.stl)",
            Format::Ply => "Stanford PLY (.ply)",
            Format::ThreeMf => "3D Manufacturing (.3mf)",
            Format::Off => "Object File Format (.off)",
            Format::X => "DirectX (.x, OMSI)",
            Format::O3d => "OMSI o3d (.o3d)",
        }
    }
}

/// A mesh to write and where it stands: `transform` takes its frame to the scene's (the map's:
/// x east, y north, z up, metres).
#[derive(Clone, Debug)]
pub struct ExportObject {
    pub name: String,
    pub mesh: Mesh,
    pub transform: Mat4,
}

/// Meshes and the materials their faces name.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub objects: Vec<ExportObject>,
    pub materials: Vec<Material>,
}

/// Write `scene` to `path` in `format`; the files written: the file itself, what goes with it
/// (the `.mtl`, the `.bin`), then the textures copied.
///
/// Each object's `transform` is applied to its vertices (a mirroring one turns its faces round
/// so they still face out) and each object keeps its name. The textures the materials name
/// (colour, and normal and ORM maps where the format has them) are found with `texture_path`
/// and copied beside the file under their own file names, never onto themselves, or packed
/// into it for `.glb` and `.3mf`; a texture that is not found is left out. glTF, GLB, USD and
/// 3MF take only PNG and JPEG pictures: the caller converts other textures (`.dds`, `.bmp`,
/// `.tga`...) to PNG and names the PNGs in the materials before calling, others are left out
/// of those formats. `.x` and `.o3d` keep each material's texture name as it is, as OMSI looks
/// it up itself; they hold one mesh, so all objects go into it.
pub fn export(scene: &Scene, format: Format, path: &Path, texture_path: &dyn Fn(&str) -> Option<PathBuf>) -> io::Result<Vec<PathBuf>> {
    let objs = place(scene);
    let mats = Mats::new(scene, &objs);
    let mut out = Out::new(path, texture_path);
    match format {
        Format::Obj => write_obj(&objs, &mats, &mut out)?,
        Format::Gltf => write_gltf(&objs, &mats, &mut out, false)?,
        Format::Glb => write_gltf(&objs, &mats, &mut out, true)?,
        Format::Fbx => {
            let bytes = fbx(&objs, &mats, &mut out);
            out.file(path, &bytes)?;
        }
        Format::Dae => {
            let text = dae(&objs, &mats, &mut out);
            out.file(path, text.as_bytes())?;
        }
        Format::Usda => {
            let text = usda(&objs, &mats, &mut out);
            out.file(path, text.as_bytes())?;
        }
        Format::Stl => out.file(path, &stl(&objs))?,
        Format::Ply => out.file(path, &ply(&objs))?,
        Format::ThreeMf => write_3mf(&objs, &mats, &mut out)?,
        Format::Off => out.file(path, off(&objs).as_bytes())?,
        Format::X | Format::O3d => {
            let mut merged = Mesh::default();
            for o in &objs {
                merged.append(&o.mesh, 0);
            }
            let (bytes, used) = if format == Format::X {
                let (x, used) = to_x(&merged, &scene.materials);
                (x.into_bytes(), used)
            } else {
                to_o3d(&merged, &scene.materials)
            };
            out.file(path, &bytes)?;
            for i in used {
                if let Some(m) = scene.materials.get(i as usize) {
                    out.texture(m.texture.as_ref(), false);
                }
            }
        }
    }
    out.finish()
}

/// An object ready to write: its mesh in the scene's frame (faces that cannot be drawn left
/// out), its triangles, and the normal at each corner of each face.
struct Placed {
    name: String,
    mesh: Mesh,
    tri: TriMesh,
    normals: Vec<Vec<Vec3>>,
}

/// The scene's objects with their transforms applied; those without a face to draw are left
/// out.
fn place(scene: &Scene) -> Vec<Placed> {
    let mut out = Vec::new();
    for o in &scene.objects {
        let mut mesh = o.mesh.clone();
        let n = mesh.positions.len() as u32;
        mesh.faces.retain(|f| f.verts.len() >= 3 && f.verts.iter().all(|&v| v < n));
        if mesh.faces.is_empty() {
            continue;
        }
        for f in &mut mesh.faces {
            f.uvs.resize(f.verts.len(), Vec2::ZERO);
        }
        mesh.transform(o.transform);
        let mut tri = mesh.triangles(SMOOTH_ANGLE);
        for n in &mut tri.normals {
            *n = usable(*n);
        }
        let mut normals: Vec<Vec<Vec3>> = (0..mesh.faces.len()).map(|f| vec![usable(mesh.face_normal(f)); mesh.faces[f].verts.len()]).collect();
        for (c, &fi) in tri.indices.chunks(3).zip(&tri.tri_face) {
            let face = &mesh.faces[fi as usize];
            for &k in c {
                let k = k as usize;
                for (ci, (&v, &uv)) in face.verts.iter().zip(&face.uvs).enumerate() {
                    if v == tri.source_vertex[k] && uv == tri.uvs[k] {
                        normals[fi as usize][ci] = tri.normals[k];
                    }
                }
            }
        }
        let name = if o.name.trim().is_empty() { "Object".to_string() } else { o.name.trim().to_string() };
        out.push(Placed { name, mesh, tri, normals });
    }
    out
}

/// A normal fit to write: a degenerate face's zero normal becomes straight up.
fn usable(n: Vec3) -> Vec3 {
    if n.is_finite() && n.length_squared() > 0.25 {
        n
    } else {
        Vec3::Z
    }
}

/// The materials the faces use, in order of first use.
struct Mats {
    /// Their numbers in the scene.
    ids: Vec<u32>,
    /// The materials (the default one for a number the scene does not have).
    list: Vec<Material>,
}

impl Mats {
    fn new(scene: &Scene, objs: &[Placed]) -> Mats {
        let mut ids = Vec::new();
        for o in objs {
            for m in o.tri.materials_used() {
                if !ids.contains(&m) {
                    ids.push(m);
                }
            }
        }
        let list = ids.iter().map(|&i| scene.materials.get(i as usize).cloned().unwrap_or_default()).collect();
        Mats { ids, list }
    }

    /// The place in the list of the scene's material `id`.
    fn slot(&self, id: u32) -> usize {
        self.ids.iter().position(|&i| i == id).unwrap_or(0)
    }
}

/// Where the files go and what has been written.
struct Out<'a> {
    path: &'a Path,
    dir: PathBuf,
    texture_path: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// Textures to copy beside the file: file name and source.
    copies: Vec<(String, PathBuf)>,
    written: Vec<PathBuf>,
}

impl<'a> Out<'a> {
    fn new(path: &'a Path, texture_path: &'a dyn Fn(&str) -> Option<PathBuf>) -> Out<'a> {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
        Out { path, dir, texture_path, copies: Vec::new(), written: Vec::new() }
    }

    fn file(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        fs::write(path, bytes)?;
        self.written.push(path.to_path_buf());
        Ok(())
    }

    /// The file of the texture `name`, if it is there (and, with `web`, a PNG or JPEG): its
    /// file name and where it is.
    fn find(&self, name: Option<&String>, web: bool) -> Option<(String, PathBuf)> {
        let name = name?;
        let file = Path::new(name).file_name()?.to_str()?.to_string();
        if web && !is_web_image(&file) {
            return None;
        }
        let src = (self.texture_path)(name)?;
        src.is_file().then_some((file, src))
    }

    /// As [`Out::find`], the texture to be copied beside the file: the file name to refer to.
    fn texture(&mut self, name: Option<&String>, web: bool) -> Option<String> {
        let (file, src) = self.find(name, web)?;
        if !self.copies.iter().any(|(f, _)| *f == file) {
            self.copies.push((file.clone(), src));
        }
        Some(file)
    }

    /// Copy the textures beside the file (not one onto itself); the files written.
    fn finish(mut self) -> io::Result<Vec<PathBuf>> {
        for (file, src) in std::mem::take(&mut self.copies) {
            let dst = self.dir.join(&file);
            let same = matches!((fs::canonicalize(&src), fs::canonicalize(&dst)), (Ok(a), Ok(b)) if a == b);
            if !same {
                fs::copy(&src, &dst)?;
                self.written.push(dst);
            }
        }
        Ok(self.written)
    }
}

fn is_web_image(file: &str) -> bool {
    let ext = Path::new(file).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg")
}

fn mime(file: &str) -> &'static str {
    if file.to_ascii_lowercase().ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    }
}

/// A file name as a URI reference: everything but letters, digits and `-._~` escaped.
fn uri(file: &str) -> String {
    let mut s = String::new();
    for b in file.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            s.push(b as char);
        } else {
            let _ = write!(s, "%{b:02X}");
        }
    }
    s
}

/// Text made safe inside XML (elements and attributes).
fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// `names` with repeats (case aside) numbered: `name`, `name.001`... with `sep` before the
/// number.
fn unique(names: impl IntoIterator<Item = String>, sep: char) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let mut name = n.clone();
        let mut i = 0;
        while out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            i += 1;
            name = format!("{n}{sep}{i:03}");
        }
        out.push(name);
    }
    out
}

/// From the map's frame to the y-up one of OBJ and glTF (x, z, -y): a turn, so the winding
/// stays.
fn y_up(p: Vec3) -> Vec3 {
    Vec3::new(p.x, p.z, -p.y)
}

/// The colour a material is written with: white where its texture is drawn (the texture shows
/// as it is, as in OMSI), and its opacity.
fn base_color(m: &Material, textured: bool) -> [f32; 4] {
    let c = if textured { [1.0; 3] } else { [m.color[0], m.color[1], m.color[2]] };
    [c[0], c[1], c[2], opacity(m)]
}

/// How opaque a material is: fully for an opaque one whatever its colour's alpha.
fn opacity(m: &Material) -> f32 {
    if m.alpha == 0 {
        1.0
    } else {
        m.color[3].clamp(0.0, 1.0)
    }
}

/// Roughness for a Phong power (the usual sqrt(2 / (n + 2))).
fn roughness(m: &Material) -> f32 {
    (2.0 / (m.power.max(0.0) + 2.0)).sqrt().clamp(0.0, 1.0)
}

/// Values numbered in order of first appearance (UVs, normals), each kept once.
#[derive(Default)]
struct Table<const N: usize> {
    list: Vec<[f32; N]>,
    seen: HashMap<[u32; N], usize>,
}

impl<const N: usize> Table<N> {
    fn index(&mut self, v: [f32; N]) -> usize {
        *self.seen.entry(v.map(f32::to_bits)).or_insert_with(|| {
            self.list.push(v);
            self.list.len() - 1
        })
    }
}

fn put_f32s(buf: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        buf.extend_from_slice(&v.to_le_bytes());
    }
}

// ---- OBJ ----

/// A name OBJ and MTL can hold: no spaces.
fn obj_name(s: &str) -> String {
    s.chars().map(|c| if c.is_whitespace() || c.is_control() { '_' } else { c }).collect()
}

/// Wavefront OBJ (y up, polygons kept) and its MTL.
fn write_obj(objs: &[Placed], mats: &Mats, out: &mut Out) -> io::Result<()> {
    let path = out.path;
    let mtl_path = path.with_extension("mtl");
    let mtl_file = mtl_path.file_name().and_then(|n| n.to_str()).unwrap_or("scene.mtl").to_string();
    let mat_names = unique(mats.list.iter().map(|m| obj_name(&m.name)), '.');
    let mut s = format!("# {MAKER}\nmtllib {mtl_file}\n");
    let (mut base_v, mut base_t, mut base_n) = (1, 1, 1);
    for (o, name) in objs.iter().zip(unique(objs.iter().map(|o| obj_name(&o.name)), '.')) {
        let _ = writeln!(s, "o {name}");
        for p in &o.mesh.positions {
            let p = y_up(*p);
            let _ = writeln!(s, "v {} {} {}", p.x, p.y, p.z);
        }
        let mut uvs = Table::<2>::default();
        let mut normals = Table::<3>::default();
        let mut faces = String::new();
        let mut current = None;
        for (fi, f) in o.mesh.faces.iter().enumerate() {
            if current != Some(f.material) {
                current = Some(f.material);
                let _ = writeln!(faces, "usemtl {}", mat_names[mats.slot(f.material)]);
            }
            faces.push('f');
            for (ci, &v) in f.verts.iter().enumerate() {
                let t = uvs.index([f.uvs[ci].x, 1.0 - f.uvs[ci].y]);
                let n = normals.index(y_up(o.normals[fi][ci]).to_array());
                let _ = write!(faces, " {}/{}/{}", base_v + v as usize, base_t + t, base_n + n);
            }
            faces.push('\n');
        }
        for t in &uvs.list {
            let _ = writeln!(s, "vt {} {}", t[0], t[1]);
        }
        for n in &normals.list {
            let _ = writeln!(s, "vn {} {} {}", n[0], n[1], n[2]);
        }
        s.push_str(&faces);
        base_v += o.mesh.positions.len();
        base_t += uvs.list.len();
        base_n += normals.list.len();
    }
    let mut mtl = format!("# {MAKER}\n");
    for (m, name) in mats.list.iter().zip(&mat_names) {
        let tex = out.texture(m.texture.as_ref(), false);
        let normal = out.texture(m.normal_map.as_ref(), false);
        let c = base_color(m, tex.is_some());
        let _ = writeln!(mtl, "\nnewmtl {name}");
        let _ = writeln!(mtl, "Kd {} {} {}", c[0], c[1], c[2]);
        let _ = writeln!(mtl, "Ks {} {} {}", m.specular[0], m.specular[1], m.specular[2]);
        let _ = writeln!(mtl, "Ns {}", m.power);
        let _ = writeln!(mtl, "Ke {} {} {}", m.emissive[0], m.emissive[1], m.emissive[2]);
        let _ = writeln!(mtl, "d {}\nTr {}\nillum 2", c[3], 1.0 - c[3]);
        if let Some(t) = &tex {
            let _ = writeln!(mtl, "map_Kd {t}");
            if m.alpha != 0 {
                let _ = writeln!(mtl, "map_d {t}");
            }
        }
        if let Some(t) = &normal {
            let _ = writeln!(mtl, "map_Bump {t}\nnorm {t}");
        }
    }
    out.file(path, s.as_bytes())?;
    out.file(&mtl_path, mtl.as_bytes())
}

// ---- glTF ----

const GL_FLOAT: u32 = 5126;
const GL_ARRAY_BUFFER: u32 = 34962;
const GL_ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// A number for JSON as short as the `f32` it comes from.
fn js(x: f32) -> Value {
    json!(x.to_string().parse::<f64>().unwrap_or(0.0))
}

fn js_list(v: &[f32]) -> Value {
    Value::Array(v.iter().map(|&x| js(x)).collect())
}

/// What a glTF file is being made of: its binary buffer and the lists that point into it.
#[derive(Default)]
struct Gltf {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    images: Vec<Value>,
    textures: Vec<Value>,
    /// Texture numbers by file name.
    by_file: HashMap<String, usize>,
}

impl Gltf {
    /// Add bytes to the buffer (on a 4-byte boundary): the buffer view's number.
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    /// Add an accessor of `count` elements of `kind` over new bytes: its number.
    fn accessor(&mut self, bytes: &[u8], target: u32, component: u32, count: usize, kind: &str) -> usize {
        let view = self.view(bytes, Some(target));
        self.accessors.push(json!({"bufferView": view, "componentType": component, "count": count, "type": kind}));
        self.accessors.len() - 1
    }

    /// The texture of the picture `name` names, if there is one to use: its number. The
    /// picture goes into the buffer (`glb`) or beside the file.
    fn texture(&mut self, name: Option<&String>, out: &mut Out, glb: bool) -> io::Result<Option<usize>> {
        let Some((file, src)) = out.find(name, true) else { return Ok(None) };
        if let Some(&i) = self.by_file.get(&file) {
            return Ok(Some(i));
        }
        let image = if glb {
            let bytes = fs::read(&src)?;
            let view = self.view(&bytes, None);
            json!({"name": file, "bufferView": view, "mimeType": mime(&file)})
        } else {
            out.texture(name, true);
            json!({"name": file, "uri": uri(&file)})
        };
        self.images.push(image);
        self.textures.push(json!({"sampler": 0, "source": self.images.len() - 1}));
        let i = self.textures.len() - 1;
        self.by_file.insert(file, i);
        Ok(Some(i))
    }
}

/// glTF 2.0 (y up): a `.gltf` with its `.bin` and the textures beside it, or a `.glb` holding
/// them all.
fn write_gltf(objs: &[Placed], mats: &Mats, out: &mut Out, glb: bool) -> io::Result<()> {
    let path = out.path;
    let mut g = Gltf::default();
    let mut materials = Vec::new();
    for m in &mats.list {
        let color = g.texture(m.texture.as_ref(), out, glb)?;
        let normal = g.texture(m.normal_map.as_ref(), out, glb)?;
        let orm = g.texture(m.orm_map.as_ref(), out, glb)?;
        let mut pbr = json!({"baseColorFactor": js_list(&base_color(m, color.is_some())), "metallicFactor": 0.0, "roughnessFactor": js(roughness(m))});
        if let Some(t) = color {
            pbr["baseColorTexture"] = json!({"index": t});
        }
        let mut mat = json!({"name": m.name});
        if let Some(t) = orm {
            // occlusion, roughness and metalness in the red, green and blue, as glTF has them
            pbr["metallicRoughnessTexture"] = json!({"index": t});
            pbr["metallicFactor"] = json!(1.0);
            pbr["roughnessFactor"] = json!(1.0);
            mat["occlusionTexture"] = json!({"index": t});
        }
        mat["pbrMetallicRoughness"] = pbr;
        if let Some(t) = normal {
            mat["normalTexture"] = json!({"index": t});
        }
        match m.alpha {
            0 => mat["alphaMode"] = json!("OPAQUE"),
            1 => {
                mat["alphaMode"] = json!("MASK");
                mat["alphaCutoff"] = json!(0.5);
            }
            _ => mat["alphaMode"] = json!("BLEND"),
        }
        if m.emissive.iter().any(|&e| e > 0.0) {
            mat["emissiveFactor"] = js_list(&m.emissive.map(|e| e.clamp(0.0, 1.0)));
        }
        materials.push(mat);
    }
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    for (k, o) in objs.iter().enumerate() {
        let t = &o.tri;
        let n = t.positions.len();
        let pos: Vec<Vec3> = t.positions.iter().map(|&p| y_up(p)).collect();
        let (lo, hi) = pos.iter().fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(lo, hi), &p| (lo.min(p), hi.max(p)));
        let mut bytes = Vec::with_capacity(n * 12);
        put_f32s(&mut bytes, &pos.iter().flat_map(|p| p.to_array()).collect::<Vec<_>>());
        let pa = g.accessor(&bytes, GL_ARRAY_BUFFER, GL_FLOAT, n, "VEC3");
        g.accessors[pa]["min"] = js_list(&lo.to_array());
        g.accessors[pa]["max"] = js_list(&hi.to_array());
        bytes.clear();
        put_f32s(&mut bytes, &t.normals.iter().flat_map(|&v| y_up(v).to_array()).collect::<Vec<_>>());
        let na = g.accessor(&bytes, GL_ARRAY_BUFFER, GL_FLOAT, n, "VEC3");
        bytes.clear();
        put_f32s(&mut bytes, &t.uvs.iter().flat_map(|v| v.to_array()).collect::<Vec<_>>());
        let ta = g.accessor(&bytes, GL_ARRAY_BUFFER, GL_FLOAT, n, "VEC2");
        // (16-bit indices only while they stay below 65535, which glTF keeps for restarts)
        let wide = n > 65535;
        let mut primitives = Vec::new();
        for id in t.materials_used() {
            let idx: Vec<u32> = t.indices.chunks(3).zip(&t.tri_material).filter(|(_, m)| **m == id).flat_map(|(c, _)| c.iter().copied()).collect();
            bytes.clear();
            for &i in &idx {
                if wide {
                    bytes.extend_from_slice(&i.to_le_bytes());
                } else {
                    bytes.extend_from_slice(&(i as u16).to_le_bytes());
                }
            }
            let ia = g.accessor(&bytes, GL_ELEMENT_ARRAY_BUFFER, if wide { 5125 } else { 5123 }, idx.len(), "SCALAR");
            primitives.push(json!({"attributes": {"POSITION": pa, "NORMAL": na, "TEXCOORD_0": ta}, "indices": ia, "material": mats.slot(id), "mode": 4}));
        }
        meshes.push(json!({"name": o.name, "primitives": primitives}));
        nodes.push(json!({"name": o.name, "mesh": k}));
    }
    let mut root = serde_json::Map::new();
    root.insert("asset".into(), json!({"version": "2.0", "generator": MAKER}));
    if !nodes.is_empty() {
        root.insert("scene".into(), json!(0));
        root.insert("scenes".into(), json!([{"name": "Scene", "nodes": (0..nodes.len()).collect::<Vec<_>>()}]));
    }
    if !g.textures.is_empty() {
        root.insert("samplers".into(), json!([{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}]));
    }
    let bin_path = path.with_extension("bin");
    if !g.bin.is_empty() {
        let mut buffer = json!({"byteLength": g.bin.len()});
        if !glb {
            let bin_file = bin_path.file_name().and_then(|n| n.to_str()).unwrap_or("scene.bin");
            buffer["uri"] = json!(uri(bin_file));
        }
        root.insert("buffers".into(), json!([buffer]));
    }
    for (key, list) in [("nodes", nodes), ("meshes", meshes), ("materials", materials), ("textures", g.textures), ("images", g.images), ("accessors", g.accessors), ("bufferViews", g.views)] {
        if !list.is_empty() {
            root.insert(key.into(), Value::Array(list));
        }
    }
    let root = Value::Object(root);
    if glb {
        let mut text = serde_json::to_vec(&root).map_err(io::Error::other)?;
        while !text.len().is_multiple_of(4) {
            text.push(b' ');
        }
        let mut bin = g.bin;
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let total = 12 + 8 + text.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
        let mut b = Vec::with_capacity(total);
        b.extend_from_slice(b"glTF");
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&(total as u32).to_le_bytes());
        b.extend_from_slice(&(text.len() as u32).to_le_bytes());
        b.extend_from_slice(b"JSON");
        b.extend_from_slice(&text);
        if !bin.is_empty() {
            b.extend_from_slice(&(bin.len() as u32).to_le_bytes());
            b.extend_from_slice(b"BIN\0");
            b.extend_from_slice(&bin);
        }
        out.file(path, &b)
    } else {
        let text = serde_json::to_vec_pretty(&root).map_err(io::Error::other)?;
        out.file(path, &text)?;
        if !g.bin.is_empty() {
            out.file(&bin_path, &g.bin)?;
        }
        Ok(())
    }
}

// ---- FBX ----

/// The file id, creation time and footer id that go together in binary FBX files that
/// Autodesk's tools and Blender read.
const FBX_FILE_ID: [u8; 16] = [0x28, 0xb3, 0x2a, 0xeb, 0xb6, 0x24, 0xcc, 0xc2, 0xbf, 0xc8, 0xb0, 0x2a, 0xa9, 0x2b, 0xfc, 0xf1];
const FBX_TIME: &str = "1970-01-01 10:00:00:000";
const FBX_FOOTER_ID: [u8; 16] = [0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26, 0x7e];
const FBX_FOOTER_MAGIC: [u8; 16] = [0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29, 0x0b];
const FBX_VERSION: u32 = 7400;

/// A property of a binary FBX record.
enum Fp {
    C(bool),
    I(i32),
    L(i64),
    D(f64),
    S(String),
    R(Vec<u8>),
    Ints(Vec<i32>),
    Doubles(Vec<f64>),
}

/// A binary FBX record: its name, properties and the records inside it.
struct Rec {
    name: &'static str,
    props: Vec<Fp>,
    kids: Vec<Rec>,
}

fn rec(name: &'static str, props: Vec<Fp>, kids: Vec<Rec>) -> Rec {
    Rec { name, props, kids }
}

fn leaf(name: &'static str, prop: Fp) -> Rec {
    rec(name, vec![prop], Vec::new())
}

fn string(s: &str) -> Fp {
    Fp::S(s.to_string())
}

/// A `P` record of a `Properties70`: name, type, label, flags and the values.
fn p(name: &str, ty: &str, label: &str, flags: &str, values: Vec<Fp>) -> Rec {
    let mut props = vec![string(name), string(ty), string(label), string(flags)];
    props.extend(values);
    rec("P", props, Vec::new())
}

fn p_int(name: &str, v: i32) -> Rec {
    p(name, "int", "Integer", "", vec![Fp::I(v)])
}

fn p_double(name: &str, v: f64) -> Rec {
    p(name, "double", "Number", "", vec![Fp::D(v)])
}

fn p_number(name: &str, v: f64) -> Rec {
    p(name, "Number", "", "A", vec![Fp::D(v)])
}

fn p_string(name: &str, v: &str) -> Rec {
    p(name, "KString", "", "", vec![string(v)])
}

fn p_color(name: &str, c: [f32; 3]) -> Rec {
    p(name, "Color", "", "A", Vec::from(c.map(|x| Fp::D(x as f64))))
}

fn p_vector(name: &str, v: [f64; 3]) -> Rec {
    p(name, name, "", "A", Vec::from(v.map(Fp::D)))
}

fn props70(kids: Vec<Rec>) -> Rec {
    rec("Properties70", Vec::new(), kids)
}

/// An object's name with its class, as FBX keeps them ("name\0\x01Class").
fn fbx_class(name: &str, class: &str) -> Fp {
    let clean: String = name.chars().map(|c| if c.is_control() { '_' } else { c }).collect();
    Fp::S(format!("{clean}\0\x01{class}"))
}

/// Write a record: offsets of 32 bits (FBX 7.4), and the null record that ends the records
/// inside one (and, as the SDK writes them, one without properties that is not the last).
fn put_rec(out: &mut Vec<u8>, r: &Rec, last: bool) {
    let start = out.len();
    out.extend_from_slice(&[0; 12]);
    out.push(r.name.len() as u8);
    out.extend_from_slice(r.name.as_bytes());
    let props_start = out.len();
    for prop in &r.props {
        match prop {
            Fp::C(v) => out.extend_from_slice(&[b'C', *v as u8]),
            Fp::I(v) => {
                out.push(b'I');
                out.extend_from_slice(&v.to_le_bytes());
            }
            Fp::L(v) => {
                out.push(b'L');
                out.extend_from_slice(&v.to_le_bytes());
            }
            Fp::D(v) => {
                out.push(b'D');
                out.extend_from_slice(&v.to_le_bytes());
            }
            Fp::S(s) => {
                out.push(b'S');
                out.extend_from_slice(&(s.len() as u32).to_le_bytes());
                out.extend_from_slice(s.as_bytes());
            }
            Fp::R(b) => {
                out.push(b'R');
                out.extend_from_slice(&(b.len() as u32).to_le_bytes());
                out.extend_from_slice(b);
            }
            Fp::Ints(v) => {
                out.push(b'i');
                for n in [v.len(), 0, v.len() * 4] {
                    out.extend_from_slice(&(n as u32).to_le_bytes());
                }
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            Fp::Doubles(v) => {
                out.push(b'd');
                for n in [v.len(), 0, v.len() * 8] {
                    out.extend_from_slice(&(n as u32).to_le_bytes());
                }
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
    }
    let props_len = out.len() - props_start;
    for (i, k) in r.kids.iter().enumerate() {
        put_rec(out, k, i + 1 == r.kids.len());
    }
    if !r.kids.is_empty() || (r.props.is_empty() && !last) {
        out.extend_from_slice(&[0; 13]);
    }
    let end = out.len() as u32;
    out[start..start + 4].copy_from_slice(&end.to_le_bytes());
    out[start + 4..start + 8].copy_from_slice(&(r.props.len() as u32).to_le_bytes());
    out[start + 8..start + 12].copy_from_slice(&(props_len as u32).to_le_bytes());
}

/// Binary FBX 7.4, z up, a unit a metre; polygons kept.
fn fbx(objs: &[Placed], mats: &Mats, out: &mut Out) -> Vec<u8> {
    let file = out.path.file_name().and_then(|n| n.to_str()).unwrap_or("scene.fbx").to_string();
    let mut next = 1_000_000_i64;
    let mut id = move || {
        next += 1;
        next
    };
    let mut objects = Vec::new();
    let mut links = Vec::new();
    let oo = |child: i64, parent: i64| rec("C", vec![string("OO"), Fp::L(child), Fp::L(parent)], Vec::new());
    let op = |child: i64, parent: i64, prop: &str| rec("C", vec![string("OP"), Fp::L(child), Fp::L(parent), string(prop)], Vec::new());
    let mut mat_ids = Vec::new();
    let mut tex_ids: HashMap<String, i64> = HashMap::new();
    for (m, name) in mats.list.iter().zip(unique(mats.list.iter().map(|m| m.name.clone()), '.')) {
        let mid = id();
        mat_ids.push(mid);
        let color = out.texture(m.texture.as_ref(), false);
        let normal = out.texture(m.normal_map.as_ref(), false);
        objects.push(fbx_material(mid, &name, m, color.is_some()));
        // the colour texture gives the alpha too where the material has any
        let mut maps = Vec::new();
        if let Some(t) = color {
            if m.alpha != 0 {
                maps.push((t.clone(), "TransparentColor"));
            }
            maps.push((t, "DiffuseColor"));
        }
        if let Some(t) = normal {
            maps.push((t, "NormalMap"));
        }
        for (tex, slot) in maps {
            let tid = match tex_ids.get(&tex) {
                Some(&t) => t,
                None => {
                    let (tid, vid) = (id(), id());
                    let abs = std::path::absolute(out.dir.join(&tex)).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| tex.clone());
                    objects.push(fbx_texture(tid, &tex, &abs));
                    objects.push(fbx_video(vid, &tex, &abs));
                    links.push(oo(vid, tid));
                    tex_ids.insert(tex, tid);
                    tid
                }
            };
            links.push(op(tid, mid, slot));
        }
    }
    for (o, name) in objs.iter().zip(unique(objs.iter().map(|o| o.name.clone()), '.')) {
        let (gid, mid) = (id(), id());
        let local = o.tri.materials_used();
        objects.push(fbx_geometry(gid, &name, o, &local));
        objects.push(fbx_model(mid, &name));
        links.push(oo(mid, 0));
        links.push(oo(gid, mid));
        for &m in &local {
            links.push(oo(mat_ids[mats.slot(m)], mid));
        }
    }
    let counts = [("GlobalSettings", 1), ("Model", objs.len()), ("Geometry", objs.len()), ("Material", mats.list.len()), ("Texture", tex_ids.len()), ("Video", tex_ids.len())];
    let mut defs = vec![leaf("Version", Fp::I(100)), leaf("Count", Fp::I(counts.iter().map(|c| c.1 as i32).sum()))];
    for (ty, n) in counts {
        if n > 0 {
            defs.push(rec("ObjectType", vec![string(ty)], vec![leaf("Count", Fp::I(n as i32))]));
        }
    }
    let mut top = fbx_header(&file);
    top.push(rec(
        "GlobalSettings",
        Vec::new(),
        vec![
            leaf("Version", Fp::I(1000)),
            props70(vec![
                p_int("UpAxis", 2),
                p_int("UpAxisSign", 1),
                p_int("FrontAxis", 1),
                p_int("FrontAxisSign", -1),
                p_int("CoordAxis", 0),
                p_int("CoordAxisSign", 1),
                p_int("OriginalUpAxis", 2),
                p_int("OriginalUpAxisSign", 1),
                // centimetres to the unit: a unit is a metre
                p_double("UnitScaleFactor", 100.0),
                p_double("OriginalUnitScaleFactor", 100.0),
                p("AmbientColor", "ColorRGB", "Color", "", vec![Fp::D(0.0), Fp::D(0.0), Fp::D(0.0)]),
                p_string("DefaultCamera", "Producer Perspective"),
                p("TimeMode", "enum", "", "", vec![Fp::I(6)]),
                p("TimeSpanStart", "KTime", "Time", "", vec![Fp::L(0)]),
                p("TimeSpanStop", "KTime", "Time", "", vec![Fp::L(46_186_158_000)]),
                p_double("CustomFrameRate", 30.0),
            ]),
        ],
    ));
    let doc = id();
    top.push(rec(
        "Documents",
        Vec::new(),
        vec![
            leaf("Count", Fp::I(1)),
            rec("Document", vec![Fp::L(doc), string("Scene"), string("Scene")], vec![props70(vec![p("SourceObject", "object", "", "", Vec::new()), p_string("ActiveAnimStackName", "")]), leaf("RootNode", Fp::L(0))]),
        ],
    ));
    top.push(rec("References", Vec::new(), Vec::new()));
    top.push(rec("Definitions", Vec::new(), defs));
    top.push(rec("Objects", Vec::new(), objects));
    top.push(rec("Connections", Vec::new(), links));
    top.push(rec("Takes", Vec::new(), vec![leaf("Current", string(""))]));

    let mut b = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
    b.extend_from_slice(&FBX_VERSION.to_le_bytes());
    for (i, r) in top.iter().enumerate() {
        put_rec(&mut b, r, i + 1 == top.len());
    }
    b.extend_from_slice(&[0; 13]);
    b.extend_from_slice(&FBX_FOOTER_ID);
    b.extend_from_slice(&[0; 4]);
    let pad = 16 - b.len() % 16;
    b.resize(b.len() + pad, 0);
    b.extend_from_slice(&FBX_VERSION.to_le_bytes());
    b.extend_from_slice(&[0; 120]);
    b.extend_from_slice(&FBX_FOOTER_MAGIC);
    b
}

/// The header extension and the file's id, time and maker.
fn fbx_header(file: &str) -> Vec<Rec> {
    let stamp = [("Version", 1000), ("Year", 1970), ("Month", 1), ("Day", 1), ("Hour", 10), ("Minute", 0), ("Second", 0), ("Millisecond", 0)].map(|(k, v)| leaf(k, Fp::I(v)));
    let mut meta = vec![leaf("Version", Fp::I(100))];
    meta.extend(["Title", "Subject", "Author", "Keywords", "Revision", "Comment"].map(|k| leaf(k, string(""))));
    let mut info = vec![p("DocumentUrl", "KString", "Url", "", vec![string(file)]), p("SrcDocumentUrl", "KString", "Url", "", vec![string(file)])];
    for group in ["Original", "LastSaved"] {
        info.push(p(group, "Compound", "", "", Vec::new()));
        info.push(p_string(&format!("{group}|ApplicationVendor"), MAKER));
        info.push(p_string(&format!("{group}|ApplicationName"), MAKER));
        info.push(p(&format!("{group}|DateTime_GMT"), "DateTime", "", "", vec![string("01/01/1970 00:00:00.000")]));
    }
    info.push(p_string("Original|FileName", file));
    let scene_info = rec("SceneInfo", vec![fbx_class("GlobalInfo", "SceneInfo"), string("UserData")], vec![leaf("Type", string("UserData")), leaf("Version", Fp::I(100)), rec("MetaData", Vec::new(), meta), props70(info)]);
    vec![
        rec(
            "FBXHeaderExtension",
            Vec::new(),
            vec![
                leaf("FBXHeaderVersion", Fp::I(1003)),
                leaf("FBXVersion", Fp::I(FBX_VERSION as i32)),
                leaf("EncryptionType", Fp::I(0)),
                rec("CreationTimeStamp", Vec::new(), Vec::from(stamp)),
                leaf("Creator", string(MAKER)),
                scene_info,
            ],
        ),
        leaf("FileId", Fp::R(FBX_FILE_ID.to_vec())),
        leaf("CreationTime", string(FBX_TIME)),
        leaf("Creator", string(MAKER)),
    ]
}

/// The mesh's polygons (the last corner of each written as -(i + 1)), with normals and UVs (v
/// up) at each corner and each polygon's material among the model's `local` ones.
fn fbx_geometry(gid: i64, name: &str, o: &Placed, local: &[u32]) -> Rec {
    let m = &o.mesh;
    let verts: Vec<f64> = m.positions.iter().flat_map(|p| [p.x as f64, p.y as f64, p.z as f64]).collect();
    let mut index = Vec::new();
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    let mut normals = Vec::new();
    let mut uvs = Table::<2>::default();
    let mut uv_index = Vec::new();
    let mut materials = Vec::new();
    for (fi, f) in m.faces.iter().enumerate() {
        let n = f.verts.len();
        for (ci, &v) in f.verts.iter().enumerate() {
            // an edge is named by the corner it starts from
            if seen.insert(edge_key(v, f.verts[(ci + 1) % n])) {
                edges.push(index.len() as i32);
            }
            index.push(if ci + 1 == n { !(v as i32) } else { v as i32 });
            let nn = o.normals[fi][ci];
            normals.extend([nn.x as f64, nn.y as f64, nn.z as f64]);
            uv_index.push(uvs.index([f.uvs[ci].x, 1.0 - f.uvs[ci].y]) as i32);
        }
        materials.push(local.iter().position(|&x| x == f.material).unwrap_or(0) as i32);
    }
    let uv: Vec<f64> = uvs.list.iter().flat_map(|t| [t[0] as f64, t[1] as f64]).collect();
    let layer = |ty: &str| rec("LayerElement", Vec::new(), vec![leaf("Type", string(ty)), leaf("TypedIndex", Fp::I(0))]);
    rec(
        "Geometry",
        vec![Fp::L(gid), fbx_class(name, "Geometry"), string("Mesh")],
        vec![
            leaf("GeometryVersion", Fp::I(124)),
            leaf("Vertices", Fp::Doubles(verts)),
            leaf("PolygonVertexIndex", Fp::Ints(index)),
            leaf("Edges", Fp::Ints(edges)),
            rec(
                "LayerElementNormal",
                vec![Fp::I(0)],
                vec![leaf("Version", Fp::I(101)), leaf("Name", string("")), leaf("MappingInformationType", string("ByPolygonVertex")), leaf("ReferenceInformationType", string("Direct")), leaf("Normals", Fp::Doubles(normals))],
            ),
            rec(
                "LayerElementUV",
                vec![Fp::I(0)],
                vec![
                    leaf("Version", Fp::I(101)),
                    leaf("Name", string("UVMap")),
                    leaf("MappingInformationType", string("ByPolygonVertex")),
                    leaf("ReferenceInformationType", string("IndexToDirect")),
                    leaf("UV", Fp::Doubles(uv)),
                    leaf("UVIndex", Fp::Ints(uv_index)),
                ],
            ),
            rec(
                "LayerElementMaterial",
                vec![Fp::I(0)],
                vec![leaf("Version", Fp::I(101)), leaf("Name", string("")), leaf("MappingInformationType", string("ByPolygon")), leaf("ReferenceInformationType", string("IndexToDirect")), leaf("Materials", Fp::Ints(materials))],
            ),
            rec("Layer", vec![Fp::I(0)], vec![leaf("Version", Fp::I(100)), layer("LayerElementNormal"), layer("LayerElementUV"), layer("LayerElementMaterial")]),
        ],
    )
}

fn fbx_model(mid: i64, name: &str) -> Rec {
    rec(
        "Model",
        vec![Fp::L(mid), fbx_class(name, "Model"), string("Mesh")],
        vec![
            leaf("Version", Fp::I(232)),
            props70(vec![
                p_vector("Lcl Translation", [0.0; 3]),
                p_vector("Lcl Rotation", [0.0; 3]),
                p_vector("Lcl Scaling", [1.0; 3]),
                p_int("DefaultAttributeIndex", 0),
                p("InheritType", "enum", "", "", vec![Fp::I(1)]),
            ]),
            leaf("MultiLayer", Fp::I(0)),
            leaf("MultiTake", Fp::I(0)),
            leaf("Shading", Fp::C(true)),
            leaf("Culling", string("CullingOff")),
        ],
    )
}

/// A Phong material. The specular colour goes as its brightest channel (the factor) times a
/// colour, so that tools that read only the factor get it right.
fn fbx_material(mid: i64, name: &str, m: &Material, textured: bool) -> Rec {
    let c = base_color(m, textured);
    let spec = m.specular.iter().fold(0.0f32, |a, &b| a.max(b));
    let spec_color = if spec > 0.0 { m.specular.map(|s| s / spec) } else { [1.0; 3] };
    rec(
        "Material",
        vec![Fp::L(mid), fbx_class(name, "Material"), string("")],
        vec![
            leaf("Version", Fp::I(102)),
            leaf("ShadingModel", string("Phong")),
            leaf("MultiLayer", Fp::I(0)),
            props70(vec![
                p_string("ShadingModel", "Phong"),
                p_color("DiffuseColor", [c[0], c[1], c[2]]),
                p_number("DiffuseFactor", 1.0),
                p_color("EmissiveColor", m.emissive),
                p_number("EmissiveFactor", 1.0),
                p_color("AmbientColor", [0.0; 3]),
                p_color("SpecularColor", spec_color),
                p_number("SpecularFactor", spec as f64),
                p_number("Shininess", m.power as f64),
                p_number("ShininessExponent", m.power as f64),
                p_color("TransparentColor", [1.0; 3]),
                p_number("TransparencyFactor", 1.0 - c[3] as f64),
                p_number("Opacity", c[3] as f64),
            ]),
        ],
    )
}

fn fbx_texture(tid: i64, file: &str, abs: &str) -> Rec {
    rec(
        "Texture",
        vec![Fp::L(tid), fbx_class(file, "Texture"), string("")],
        vec![
            leaf("Type", string("TextureVideoClip")),
            leaf("Version", Fp::I(202)),
            leaf("TextureName", fbx_class(file, "Texture")),
            props70(vec![p("UseMaterial", "bool", "", "", vec![Fp::I(1)])]),
            leaf("Media", fbx_class(file, "Video")),
            leaf("FileName", string(abs)),
            leaf("RelativeFilename", string(file)),
            rec("ModelUVTranslation", vec![Fp::D(0.0), Fp::D(0.0)], Vec::new()),
            rec("ModelUVScaling", vec![Fp::D(1.0), Fp::D(1.0)], Vec::new()),
            leaf("Texture_Alpha_Source", string("None")),
            rec("Cropping", vec![Fp::I(0), Fp::I(0), Fp::I(0), Fp::I(0)], Vec::new()),
        ],
    )
}

fn fbx_video(vid: i64, file: &str, abs: &str) -> Rec {
    rec(
        "Video",
        vec![Fp::L(vid), fbx_class(file, "Video"), string("Clip")],
        vec![
            leaf("Type", string("Clip")),
            props70(vec![p("Path", "KString", "XRefUrl", "", vec![string(abs)])]),
            leaf("UseMipMap", Fp::I(0)),
            leaf("Filename", string(abs)),
            leaf("RelativeFilename", string(file)),
        ],
    )
}

// ---- Collada ----

fn join<T: std::fmt::Display>(items: impl IntoIterator<Item = T>) -> String {
    let mut s = String::new();
    for (i, x) in items.into_iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{x}");
    }
    s
}

/// A `<source>` of `stride` floats per element named by `params`.
fn dae_source(s: &mut String, id: &str, values: &[f32], params: &[&str]) {
    let stride = params.len();
    let _ = writeln!(s, "        <source id=\"{id}\">\n          <float_array id=\"{id}-array\" count=\"{}\">{}</float_array>", values.len(), join(values));
    let _ = writeln!(s, "          <technique_common>\n            <accessor source=\"#{id}-array\" count=\"{}\" stride=\"{stride}\">", values.len() / stride);
    for p in params {
        let _ = writeln!(s, "              <param name=\"{p}\" type=\"float\"/>");
    }
    s.push_str("            </accessor>\n          </technique_common>\n        </source>\n");
}

/// Collada 1.4.1: metres, z up, polygons kept.
fn dae(objs: &[Placed], mats: &Mats, out: &mut Out) -> String {
    let tex: Vec<Option<String>> = mats.list.iter().map(|m| out.texture(m.texture.as_ref(), false)).collect();
    let mut images: Vec<&String> = Vec::new();
    for t in tex.iter().flatten() {
        if !images.contains(&t) {
            images.push(t);
        }
    }
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<COLLADA xmlns=\"http://www.collada.org/2005/11/COLLADASchema\" version=\"1.4.1\">\n");
    let _ = writeln!(
        s,
        "  <asset>\n    <contributor>\n      <authoring_tool>{MAKER}</authoring_tool>\n    </contributor>\n    <created>1970-01-01T00:00:00</created>\n    <modified>1970-01-01T00:00:00</modified>\n    <unit name=\"meter\" meter=\"1\"/>\n    <up_axis>Z_UP</up_axis>\n  </asset>"
    );
    if !images.is_empty() {
        s.push_str("  <library_images>\n");
        for (i, f) in images.iter().enumerate() {
            let _ = writeln!(s, "    <image id=\"image{i}\" name=\"{}\">\n      <init_from>{}</init_from>\n    </image>", xml(f), xml(&uri(f)));
        }
        s.push_str("  </library_images>\n");
    }
    if !mats.list.is_empty() {
        s.push_str("  <library_effects>\n");
        for (i, m) in mats.list.iter().enumerate() {
            let img = tex[i].as_ref().and_then(|t| images.iter().position(|x| *x == t));
            let c = base_color(m, img.is_some());
            let _ = writeln!(s, "    <effect id=\"effect{i}\">\n      <profile_COMMON>");
            if let Some(k) = img {
                let _ = writeln!(s, "        <newparam sid=\"image{k}-surface\">\n          <surface type=\"2D\">\n            <init_from>image{k}</init_from>\n          </surface>\n        </newparam>");
                let _ = writeln!(s, "        <newparam sid=\"image{k}-sampler\">\n          <sampler2D>\n            <source>image{k}-surface</source>\n          </sampler2D>\n        </newparam>");
            }
            let sampler = img.map(|k| format!("<texture texture=\"image{k}-sampler\" texcoord=\"UVMap\"/>"));
            let _ = writeln!(s, "        <technique sid=\"common\">\n          <phong>");
            let _ = writeln!(s, "            <emission>\n              <color sid=\"emission\">{} {} {} 1</color>\n            </emission>", m.emissive[0], m.emissive[1], m.emissive[2]);
            let diffuse = sampler.clone().unwrap_or_else(|| format!("<color sid=\"diffuse\">{} {} {} {}</color>", c[0], c[1], c[2], c[3]));
            let _ = writeln!(s, "            <diffuse>\n              {diffuse}\n            </diffuse>");
            let _ = writeln!(s, "            <specular>\n              <color sid=\"specular\">{} {} {} 1</color>\n            </specular>", m.specular[0], m.specular[1], m.specular[2]);
            let _ = writeln!(s, "            <shininess>\n              <float sid=\"shininess\">{}</float>\n            </shininess>", m.power);
            if m.alpha != 0 {
                // A_ONE: opacity is the alpha of this times the transparency
                let from = sampler.unwrap_or_else(|| "<color>1 1 1 1</color>".to_string());
                let _ = writeln!(s, "            <transparent opaque=\"A_ONE\">\n              {from}\n            </transparent>");
                let _ = writeln!(s, "            <transparency>\n              <float sid=\"transparency\">{}</float>\n            </transparency>", c[3]);
            }
            s.push_str("          </phong>\n        </technique>\n      </profile_COMMON>\n    </effect>\n");
        }
        s.push_str("  </library_effects>\n  <library_materials>\n");
        for (i, m) in mats.list.iter().enumerate() {
            let _ = writeln!(s, "    <material id=\"material{i}\" name=\"{}\">\n      <instance_effect url=\"#effect{i}\"/>\n    </material>", xml(&m.name));
        }
        s.push_str("  </library_materials>\n");
    }
    if !objs.is_empty() {
        s.push_str("  <library_geometries>\n");
        for (k, o) in objs.iter().enumerate() {
            let m = &o.mesh;
            let mut normals = Table::<3>::default();
            let mut uvs = Table::<2>::default();
            let corners: Vec<Vec<[usize; 3]>> = m
                .faces
                .iter()
                .enumerate()
                .map(|(fi, f)| f.verts.iter().enumerate().map(|(ci, &v)| [v as usize, normals.index(o.normals[fi][ci].to_array()), uvs.index([f.uvs[ci].x, 1.0 - f.uvs[ci].y])]).collect())
                .collect();
            let _ = writeln!(s, "    <geometry id=\"object{k}-mesh\" name=\"{}\">\n      <mesh>", xml(&o.name));
            dae_source(&mut s, &format!("object{k}-positions"), &m.positions.iter().flat_map(|p| p.to_array()).collect::<Vec<_>>(), &["X", "Y", "Z"]);
            dae_source(&mut s, &format!("object{k}-normals"), &normals.list.concat(), &["X", "Y", "Z"]);
            dae_source(&mut s, &format!("object{k}-uvs"), &uvs.list.concat(), &["S", "T"]);
            let _ = writeln!(s, "        <vertices id=\"object{k}-vertices\">\n          <input semantic=\"POSITION\" source=\"#object{k}-positions\"/>\n        </vertices>");
            for id in o.tri.materials_used() {
                let faces: Vec<usize> = (0..m.faces.len()).filter(|&f| m.faces[f].material == id).collect();
                let _ = writeln!(s, "        <polylist material=\"material{}\" count=\"{}\">", mats.slot(id), faces.len());
                let _ = writeln!(s, "          <input semantic=\"VERTEX\" source=\"#object{k}-vertices\" offset=\"0\"/>\n          <input semantic=\"NORMAL\" source=\"#object{k}-normals\" offset=\"1\"/>\n          <input semantic=\"TEXCOORD\" source=\"#object{k}-uvs\" offset=\"2\" set=\"0\"/>");
                let _ = writeln!(s, "          <vcount>{}</vcount>", join(faces.iter().map(|&f| corners[f].len())));
                let _ = writeln!(s, "          <p>{}</p>\n        </polylist>", join(faces.iter().flat_map(|&f| corners[f].iter().flatten())));
            }
            s.push_str("      </mesh>\n    </geometry>\n");
        }
        s.push_str("  </library_geometries>\n");
    }
    s.push_str("  <library_visual_scenes>\n    <visual_scene id=\"Scene\" name=\"Scene\">\n");
    for (k, o) in objs.iter().enumerate() {
        let _ = writeln!(s, "      <node id=\"object{k}\" name=\"{0}\" type=\"NODE\">\n        <matrix sid=\"transform\">1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</matrix>\n        <instance_geometry url=\"#object{k}-mesh\" name=\"{0}\">", xml(&o.name));
        s.push_str("          <bind_material>\n            <technique_common>\n");
        for id in o.tri.materials_used() {
            let slot = mats.slot(id);
            let _ = writeln!(s, "              <instance_material symbol=\"material{slot}\" target=\"#material{slot}\">\n                <bind_vertex_input semantic=\"UVMap\" input_semantic=\"TEXCOORD\" input_set=\"0\"/>\n              </instance_material>");
        }
        s.push_str("            </technique_common>\n          </bind_material>\n        </instance_geometry>\n      </node>\n");
    }
    s.push_str("    </visual_scene>\n  </library_visual_scenes>\n  <scene>\n    <instance_visual_scene url=\"#Scene\"/>\n  </scene>\n</COLLADA>\n");
    s
}

// ---- USD ----

/// A name USD takes for a prim: letters, digits and `_`, not starting with a digit.
fn usd_name(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn usd_tuple(v: &[f32]) -> String {
    let mut s = String::from("(");
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        let _ = write!(s, "{x}");
    }
    s.push(')');
    s
}

fn usd_list(items: impl IntoIterator<Item = String>) -> String {
    let mut s = String::from("[");
    for (i, x) in items.into_iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&x);
    }
    s.push(']');
    s
}

/// A UsdPreviewSurface material with its textures.
fn usd_material(s: &mut String, m: &Material, name: &str, out: &mut Out) {
    let path = format!("/World/Looks/{name}");
    let color = out.texture(m.texture.as_ref(), true);
    let normal = out.texture(m.normal_map.as_ref(), true);
    let orm = out.texture(m.orm_map.as_ref(), true);
    let c = base_color(m, color.is_some());
    let _ = writeln!(s, "        def Material \"{name}\"\n        {{\n            token outputs:surface.connect = <{path}/Surface.outputs:surface>\n");
    s.push_str("            def Shader \"Surface\"\n            {\n                uniform token info:id = \"UsdPreviewSurface\"\n");
    match color {
        Some(_) => {
            let _ = writeln!(s, "                color3f inputs:diffuseColor.connect = <{path}/Color.outputs:rgb>");
        }
        None => {
            let _ = writeln!(s, "                color3f inputs:diffuseColor = {}", usd_tuple(&c[..3]));
        }
    }
    if color.is_some() && m.alpha != 0 {
        let _ = writeln!(s, "                float inputs:opacity.connect = <{path}/Color.outputs:a>");
    } else {
        let _ = writeln!(s, "                float inputs:opacity = {}", c[3]);
    }
    if m.alpha == 1 {
        s.push_str("                float inputs:opacityThreshold = 0.5\n");
    }
    let _ = writeln!(s, "                color3f inputs:emissiveColor = {}", usd_tuple(&m.emissive));
    if orm.is_some() {
        let _ = writeln!(s, "                float inputs:occlusion.connect = <{path}/Orm.outputs:r>\n                float inputs:roughness.connect = <{path}/Orm.outputs:g>\n                float inputs:metallic.connect = <{path}/Orm.outputs:b>");
    } else {
        let _ = writeln!(s, "                float inputs:roughness = {}\n                float inputs:metallic = 0", roughness(m));
    }
    if normal.is_some() {
        let _ = writeln!(s, "                normal3f inputs:normal.connect = <{path}/Normal.outputs:rgb>");
    }
    s.push_str("                token outputs:surface\n            }\n");
    if color.is_some() || normal.is_some() || orm.is_some() {
        s.push_str("\n            def Shader \"St\"\n            {\n                uniform token info:id = \"UsdPrimvarReader_float2\"\n                string inputs:varname = \"st\"\n                float2 outputs:result\n            }\n");
    }
    let maps = [
        (color, "Color", "sRGB", "", "float3 outputs:rgb\n                float outputs:a"),
        (normal, "Normal", "raw", "float4 inputs:scale = (2, 2, 2, 1)\n                float4 inputs:bias = (-1, -1, -1, 0)\n                ", "float3 outputs:rgb"),
        (orm, "Orm", "raw", "", "float outputs:r\n                float outputs:g\n                float outputs:b"),
    ];
    for (file, prim, space, extra, outputs) in maps {
        let Some(file) = file else { continue };
        let _ = writeln!(
            s,
            "\n            def Shader \"{prim}\"\n            {{\n                uniform token info:id = \"UsdUVTexture\"\n                asset inputs:file = @./{file}@\n                float2 inputs:st.connect = <{path}/St.outputs:result>\n                token inputs:sourceColorSpace = \"{space}\"\n                token inputs:wrapS = \"repeat\"\n                token inputs:wrapT = \"repeat\"\n                {extra}{outputs}\n            }}"
        );
    }
    s.push_str("        }\n");
}

/// USD as text: metres, z up, an Xform with a Mesh for each object (polygons kept), materials
/// as UsdPreviewSurface, one subset of faces per material where a mesh has several.
fn usda(objs: &[Placed], mats: &Mats, out: &mut Out) -> String {
    let mut s = format!("#usda 1.0\n(\n    defaultPrim = \"World\"\n    doc = \"{MAKER}\"\n    metersPerUnit = 1\n    upAxis = \"Z\"\n)\n\ndef Xform \"World\"\n{{\n");
    let mat_names = unique(mats.list.iter().map(|m| usd_name(&m.name)), '_');
    if !mats.list.is_empty() {
        s.push_str("    def Scope \"Looks\"\n    {\n");
        for (i, (m, name)) in mats.list.iter().zip(&mat_names).enumerate() {
            if i > 0 {
                s.push('\n');
            }
            usd_material(&mut s, m, name, out);
        }
        s.push_str("    }\n");
    }
    // (the materials' scope takes its name first)
    let names = unique(std::iter::once("Looks".to_string()).chain(objs.iter().map(|o| usd_name(&o.name))), '_');
    let binding = "(\n            prepend apiSchemas = [\"MaterialBindingAPI\"]\n        )";
    for (o, name) in objs.iter().zip(names.iter().skip(1)) {
        let m = &o.mesh;
        let used = o.tri.materials_used();
        let (lo, hi) = m.bounds().unwrap_or_default();
        // (the mesh named as its Xform: importers that fold the two keep the object's name)
        let _ = writeln!(s, "\n    def Xform \"{name}\"\n    {{\n        def Mesh \"{name}\" {binding}\n        {{");
        let _ = writeln!(s, "            float3[] extent = [{}, {}]", usd_tuple(&lo.to_array()), usd_tuple(&hi.to_array()));
        let _ = writeln!(s, "            point3f[] points = {}", usd_list(m.positions.iter().map(|p| usd_tuple(&p.to_array()))));
        let _ = writeln!(s, "            int[] faceVertexCounts = {}", usd_list(m.faces.iter().map(|f| f.verts.len().to_string())));
        let _ = writeln!(s, "            int[] faceVertexIndices = {}", usd_list(m.faces.iter().flat_map(|f| f.verts.iter().map(|v| v.to_string()))));
        let _ = writeln!(s, "            normal3f[] normals = {} (\n                interpolation = \"faceVarying\"\n            )", usd_list(o.normals.iter().flatten().map(|n| usd_tuple(&n.to_array()))));
        let _ = writeln!(s, "            texCoord2f[] primvars:st = {} (\n                interpolation = \"faceVarying\"\n            )", usd_list(m.faces.iter().flat_map(|f| f.uvs.iter().map(|uv| usd_tuple(&[uv.x, 1.0 - uv.y])))));
        s.push_str("            uniform token orientation = \"rightHanded\"\n            uniform token subdivisionScheme = \"none\"\n");
        if let [only] = used[..] {
            let _ = writeln!(s, "            rel material:binding = </World/Looks/{}>", mat_names[mats.slot(only)]);
        } else {
            s.push_str("            uniform token subsetFamily:materialBind:familyType = \"partition\"\n");
            for id in used {
                let mat = &mat_names[mats.slot(id)];
                let faces = (0..m.faces.len()).filter(|&f| m.faces[f].material == id).map(|f| f.to_string());
                let _ = writeln!(
                    s,
                    "\n            def GeomSubset \"{mat}\" (\n                prepend apiSchemas = [\"MaterialBindingAPI\"]\n            )\n            {{\n                uniform token elementType = \"face\"\n                uniform token familyName = \"materialBind\"\n                int[] indices = {}\n                rel material:binding = </World/Looks/{mat}>\n            }}",
                    usd_list(faces)
                );
            }
        }
        s.push_str("        }\n    }\n");
    }
    s.push_str("}\n");
    s
}

// ---- STL, PLY, OFF ----

/// Binary STL: every triangle with its facet normal, in metres, z up.
fn stl(objs: &[Placed]) -> Vec<u8> {
    let n: usize = objs.iter().map(|o| o.tri.triangle_count()).sum();
    let mut b = Vec::with_capacity(84 + 50 * n);
    let mut header = [b' '; 80];
    let title = format!("{MAKER}, metres, z up");
    header[..title.len()].copy_from_slice(title.as_bytes());
    b.extend_from_slice(&header);
    b.extend_from_slice(&(n as u32).to_le_bytes());
    for o in objs {
        let t = &o.tri;
        for c in t.indices.chunks(3) {
            let p = [c[0], c[1], c[2]].map(|i| t.positions[i as usize]);
            let normal = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
            for v in [normal, p[0], p[1], p[2]] {
                put_f32s(&mut b, &v.to_array());
            }
            b.extend_from_slice(&[0, 0]);
        }
    }
    b
}

/// Binary PLY: a vertex for each distinct corner (position, normal, UV with v up) and the
/// triangles.
fn ply(objs: &[Placed]) -> Vec<u8> {
    let nv: usize = objs.iter().map(|o| o.tri.positions.len()).sum();
    let nf: usize = objs.iter().map(|o| o.tri.triangle_count()).sum();
    let mut b = format!(
        "ply\nformat binary_little_endian 1.0\ncomment {MAKER}, metres, z up\nelement vertex {nv}\nproperty float x\nproperty float y\nproperty float z\nproperty float nx\nproperty float ny\nproperty float nz\nproperty float s\nproperty float t\nelement face {nf}\nproperty list uchar int vertex_indices\nend_header\n"
    )
    .into_bytes();
    for o in objs {
        let t = &o.tri;
        for i in 0..t.positions.len() {
            let (p, n, uv) = (t.positions[i], t.normals[i], t.uvs[i]);
            put_f32s(&mut b, &[p.x, p.y, p.z, n.x, n.y, n.z, uv.x, 1.0 - uv.y]);
        }
    }
    let mut base = 0;
    for o in objs {
        for c in o.tri.indices.chunks(3) {
            b.push(3);
            for &i in c {
                b.extend_from_slice(&((base + i as usize) as i32).to_le_bytes());
            }
        }
        base += o.tri.positions.len();
    }
    b
}

/// OFF: positions and polygons.
fn off(objs: &[Placed]) -> String {
    let nv: usize = objs.iter().map(|o| o.mesh.positions.len()).sum();
    let nf: usize = objs.iter().map(|o| o.mesh.faces.len()).sum();
    let mut s = format!("OFF\n{nv} {nf} 0\n");
    for o in objs {
        for p in &o.mesh.positions {
            let _ = writeln!(s, "{} {} {}", p.x, p.y, p.z);
        }
    }
    let mut base = 0;
    for o in objs {
        for f in &o.mesh.faces {
            let _ = writeln!(s, "{} {}", f.verts.len(), join(f.verts.iter().map(|&v| base + v as usize)));
        }
        base += o.mesh.positions.len();
    }
    s
}

// ---- 3MF ----

/// A file name safe as a part of a package: letters, digits and `-._` only.
fn part_name(file: &str) -> String {
    file.chars().map(|c| if c.is_ascii_alphanumeric() || "-._".contains(c) { c } else { '_' }).collect()
}

/// 3MF: an object of triangles for each, in metres, its materials as base materials with
/// their colours and PNG / JPEG textures packed in (with the UVs of the triangles that use
/// them).
fn write_3mf(objs: &[Placed], mats: &Mats, out: &mut Out) -> io::Result<()> {
    let mut res = String::new();
    let mut next = 1u32;
    let base_id = next;
    next += 1;
    if !mats.list.is_empty() {
        let _ = writeln!(res, "    <basematerials id=\"{base_id}\">");
        for m in &mats.list {
            let c = m.color.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
            let _ = writeln!(res, "      <base name=\"{}\" displaycolor=\"#{:02X}{:02X}{:02X}{:02X}\"/>", xml(&m.name), c[0], c[1], c[2], c[3]);
        }
        res.push_str("    </basematerials>\n");
    }
    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut tex_of: Vec<Option<u32>> = vec![None; mats.list.len()];
    let mut tex_ids: HashMap<String, u32> = HashMap::new();
    for (i, m) in mats.list.iter().enumerate() {
        let Some((file, src)) = out.find(m.texture.as_ref(), true) else { continue };
        let part = format!("3D/Textures/{}", part_name(&file));
        let tid = match tex_ids.get(&part) {
            Some(&t) => t,
            None => {
                let t = next;
                next += 1;
                let _ = writeln!(res, "    <m:texture2d id=\"{t}\" path=\"/{part}\" contenttype=\"{}\" tilestyleu=\"wrap\" tilestylev=\"wrap\"/>", mime(&file));
                parts.push((part.clone(), fs::read(&src)?));
                tex_ids.insert(part, t);
                t
            }
        };
        tex_of[i] = Some(tid);
    }
    let mut items = Vec::new();
    for (o, name) in objs.iter().zip(unique(objs.iter().map(|o| o.name.clone()), '.')) {
        let t = &o.tri;
        // a group of texture coordinates for each textured material: slot, id, texture, UVs
        let mut groups: Vec<(usize, u32, u32, Table<2>)> = Vec::new();
        let mut tris = String::new();
        for (c, &mid) in t.indices.chunks(3).zip(&t.tri_material) {
            let slot = mats.slot(mid);
            let v = [c[0], c[1], c[2]].map(|i| t.source_vertex[i as usize]);
            match tex_of[slot] {
                Some(tex) => {
                    let g = match groups.iter().position(|g| g.0 == slot) {
                        Some(g) => g,
                        None => {
                            groups.push((slot, next, tex, Table::default()));
                            next += 1;
                            groups.len() - 1
                        }
                    };
                    let p = [c[0], c[1], c[2]].map(|i| {
                        let uv = t.uvs[i as usize];
                        groups[g].3.index([uv.x, 1.0 - uv.y])
                    });
                    let _ = writeln!(tris, "          <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" pid=\"{}\" p1=\"{}\" p2=\"{}\" p3=\"{}\"/>", v[0], v[1], v[2], groups[g].1, p[0], p[1], p[2]);
                }
                None => {
                    let _ = writeln!(tris, "          <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" pid=\"{base_id}\" p1=\"{slot}\"/>", v[0], v[1], v[2]);
                }
            }
        }
        for (_, gid, tex, uvs) in &groups {
            let _ = writeln!(res, "    <m:texture2dgroup id=\"{gid}\" texid=\"{tex}\">");
            for uv in &uvs.list {
                let _ = writeln!(res, "      <m:tex2coord u=\"{}\" v=\"{}\"/>", uv[0], uv[1]);
            }
            res.push_str("    </m:texture2dgroup>\n");
        }
        let oid = next;
        next += 1;
        let first = t.tri_material.first().map_or(0, |&m| mats.slot(m));
        let _ = writeln!(res, "    <object id=\"{oid}\" type=\"model\" name=\"{}\" pid=\"{base_id}\" pindex=\"{first}\">\n      <mesh>\n        <vertices>", xml(&name));
        for p in &o.mesh.positions {
            let _ = writeln!(res, "          <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", p.x, p.y, p.z);
        }
        let _ = writeln!(res, "        </vertices>\n        <triangles>\n{tris}        </triangles>\n      </mesh>\n    </object>");
        items.push(oid);
    }
    let mut model = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"meter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" xmlns:m=\"http://schemas.microsoft.com/3dmanufacturing/material/2015/02\">\n");
    let _ = writeln!(model, "  <metadata name=\"Application\">{MAKER}</metadata>\n  <resources>\n{res}  </resources>\n  <build>");
    for oid in items {
        let _ = writeln!(model, "    <item objectid=\"{oid}\"/>");
    }
    model.push_str("  </build>\n</model>\n");

    let mut types = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n  <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n  <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n");
    let mut exts: Vec<String> = Vec::new();
    for (part, _) in &parts {
        let ext = Path::new(part).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !exts.contains(&ext) {
            let _ = writeln!(types, "  <Default Extension=\"{ext}\" ContentType=\"{}\"/>", mime(part));
            exts.push(ext);
        }
    }
    types.push_str("</Types>\n");
    let rels_head = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n";
    let rels = format!("{rels_head}  <Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/>\n</Relationships>\n");
    let mut entries: Vec<(String, Vec<u8>)> = vec![("[Content_Types].xml".into(), types.into_bytes()), ("_rels/.rels".into(), rels.into_bytes()), ("3D/3dmodel.model".into(), model.into_bytes())];
    if !parts.is_empty() {
        let mut model_rels = String::from(rels_head);
        for (i, (part, _)) in parts.iter().enumerate() {
            let _ = writeln!(model_rels, "  <Relationship Target=\"/{part}\" Id=\"tex{i}\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dtexture\"/>");
        }
        model_rels.push_str("</Relationships>\n");
        entries.push(("3D/_rels/3dmodel.model.rels".into(), model_rels.into_bytes()));
    }
    entries.extend(parts);

    let path = out.path;
    let mut zip = zip::ZipWriter::new(fs::File::create(path)?);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in &entries {
        zip.start_file(name.as_str(), options).map_err(io::Error::other)?;
        zip.write_all(data)?;
    }
    zip.finish().map_err(io::Error::other)?;
    out.written.push(path.to_path_buf());
    Ok(())
}

// ---- OMSI ----

/// From the map's frame (x east, y north, z up) to Direct3D's that OMSI's mesh files use (x
/// right, y up, z forward): y and z change places, which mirrors, so the winding turns too.
fn to_d3d(p: glam::Vec3) -> glam::Vec3 {
    glam::Vec3::new(p.x, p.z, p.y)
}

/// The mesh as a DirectX `.x` text file that OMSI 2 and openOMSI read, with only the materials
/// its faces use; the numbers of those materials come back with it, in the order of the
/// file's material slots.
pub fn to_x(mesh: &Mesh, materials: &[Material]) -> (String, Vec<u32>) {
    let t = mesh.triangles(SMOOTH_ANGLE);
    let used = t.materials_used();
    let n = t.positions.len();
    let tris = t.triangle_count();
    let mut x = String::from("xof 0302txt 0032\n\nMesh {\n");
    let _ = writeln!(x, " {n};");
    for (i, p) in t.positions.iter().enumerate() {
        let p = to_d3d(*p);
        let _ = writeln!(x, " {:.6};{:.6};{:.6};{}", p.x, p.y, p.z, if i + 1 == n { ";" } else { "," });
    }
    let _ = writeln!(x, " {tris};");
    for (i, c) in t.indices.chunks(3).enumerate() {
        // (mirrored into Direct3D's frame, so the corners go the other way round)
        let _ = writeln!(x, " 3;{},{},{};{}", c[0], c[2], c[1], if i + 1 == tris { ";" } else { "," });
    }
    let _ = writeln!(x, " MeshNormals {{\n  {n};");
    for (i, nn) in t.normals.iter().enumerate() {
        let nn = to_d3d(*nn);
        let _ = writeln!(x, "  {:.6};{:.6};{:.6};{}", nn.x, nn.y, nn.z, if i + 1 == n { ";" } else { "," });
    }
    let _ = writeln!(x, "  {tris};");
    for (i, c) in t.indices.chunks(3).enumerate() {
        let _ = writeln!(x, "  3;{},{},{};{}", c[0], c[2], c[1], if i + 1 == tris { ";" } else { "," });
    }
    x.push_str(" }\n");
    let _ = writeln!(x, " MeshTextureCoords {{\n  {n};");
    for (i, uv) in t.uvs.iter().enumerate() {
        let _ = writeln!(x, "  {:.6};{:.6};{}", uv.x, uv.y, if i + 1 == n { ";" } else { "," });
    }
    x.push_str(" }\n");
    let _ = writeln!(x, " MeshMaterialList {{\n  {};\n  {tris};", used.len().max(1));
    for (i, m) in t.tri_material.iter().enumerate() {
        let slot = used.iter().position(|u| u == m).unwrap_or(0);
        let _ = writeln!(x, "  {slot}{}", if i + 1 == tris { ";" } else { "," });
    }
    let fallback = Material::default();
    for &i in if used.is_empty() { &[0u32][..] } else { &used[..] } {
        let mat = materials.get(i as usize).unwrap_or(&fallback);
        // (a textured material shows its texture as it is: OMSI multiplies by the colour)
        let c = if mat.texture.is_some() { [1.0, 1.0, 1.0, mat.color[3]] } else { mat.color };
        let _ = writeln!(
            x,
            "  Material {{\n   {:.4};{:.4};{:.4};{:.4};;\n   {:.4};\n   {:.4};{:.4};{:.4};;\n   {:.4};{:.4};{:.4};;",
            c[0], c[1], c[2], c[3], mat.power, mat.specular[0], mat.specular[1], mat.specular[2], mat.emissive[0], mat.emissive[1], mat.emissive[2]
        );
        if let Some(tex) = &mat.texture {
            let _ = writeln!(x, "   TextureFilename {{\n    \"{tex}\";\n   }}");
        }
        x.push_str("  }\n");
    }
    x.push_str(" }\n}\n");
    (x, used)
}

/// `s` in Windows-1252, as OMSI keeps names (what it cannot hold becomes `?`), at most 255
/// bytes.
fn cp1252(s: &str) -> Vec<u8> {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    s.chars()
        .map(|c| match c as u32 {
            0..=0x7f | 0xa0..=0xff => c as u32 as u8,
            _ => HIGH.iter().position(|&h| h == c).map_or(b'?', |i| 0x80 + i as u8),
        })
        .take(255)
        .collect()
}

/// The mesh as an OMSI `.o3d` (binary, version 3, not scrambled), with the materials its
/// faces use; their numbers as for [`to_x`]. Same frame, winding and material colours as the
/// `.x`; 32-bit vertex numbers when there are more than 65535 vertices.
pub fn to_o3d(mesh: &Mesh, materials: &[Material]) -> (Vec<u8>, Vec<u32>) {
    let t = mesh.triangles(SMOOTH_ANGLE);
    let used = t.materials_used();
    let n = t.positions.len();
    let long = n > 65535;
    let mut b = vec![0x84, 0x19, 3, long as u8];
    b.push(0x17);
    b.extend_from_slice(&(n as u32).to_le_bytes());
    for i in 0..n {
        let (p, nn, uv) = (to_d3d(t.positions[i]), to_d3d(t.normals[i]), t.uvs[i]);
        put_f32s(&mut b, &[p.x, p.y, p.z, nn.x, nn.y, nn.z, uv.x, uv.y]);
    }
    b.push(0x49);
    b.extend_from_slice(&(t.triangle_count() as u32).to_le_bytes());
    for (c, m) in t.indices.chunks(3).zip(&t.tri_material) {
        for i in [c[0], c[2], c[1]] {
            if long {
                b.extend_from_slice(&i.to_le_bytes());
            } else {
                b.extend_from_slice(&(i as u16).to_le_bytes());
            }
        }
        let slot = used.iter().position(|u| u == m).unwrap_or(0) as u16;
        b.extend_from_slice(&slot.to_le_bytes());
    }
    let fallback = Material::default();
    let list: &[u32] = if used.is_empty() { &[0] } else { &used };
    b.push(0x26);
    b.extend_from_slice(&(list.len() as u16).to_le_bytes());
    for &i in list {
        let mat = materials.get(i as usize).unwrap_or(&fallback);
        let c = if mat.texture.is_some() { [1.0, 1.0, 1.0, mat.color[3]] } else { mat.color };
        put_f32s(&mut b, &c);
        put_f32s(&mut b, &mat.specular);
        put_f32s(&mut b, &mat.emissive);
        put_f32s(&mut b, &[mat.power]);
        let name = cp1252(mat.texture.as_deref().unwrap_or(""));
        b.push(name.len() as u8);
        b.extend_from_slice(&name);
    }
    // the object's transform: none
    b.push(0x79);
    put_f32s(&mut b, &Mat4::IDENTITY.to_cols_array());
    (b, used)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use std::f32::consts::TAU;
    use std::io::Read as _;

    #[test]
    fn a_cube_becomes_an_x_file_of_twelve_triangles() {
        let m = Primitive::Cube { size: glam::Vec3::ONE }.build();
        let (x, used) = to_x(&m, &[Material::default()]);
        assert!(x.starts_with("xof 0302txt"));
        assert_eq!(used, vec![0]);
        assert!(x.contains(" 12;"));
    }

    /// A fresh folder for a test's files.
    fn folder(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("openmapper-mesh-export-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    const PICTURE: &[u8] = b"\x89PNG\r\n\x1a\nnot a real picture";

    /// Two objects: a box (its top glass, the rest textured paint) moved off the origin, and a
    /// sign of five corners mirrored (its face must be turned round to face south still).
    /// Textures are in `dir/src`.
    fn sample(dir: &Path) -> Scene {
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        for f in ["bus.png", "bus_n.png", "bus_orm.png"] {
            fs::write(src.join(f), PICTURE).unwrap();
        }
        let mut cube = Primitive::Cube { size: Vec3::ONE }.build();
        for f in &mut cube.faces {
            f.material = 0;
        }
        let top = (0..cube.faces.len()).find(|&f| cube.face_normal(f).z > 0.9).unwrap();
        cube.faces[top].material = 1;
        let mut sign = Mesh::default();
        let mut uvs = Vec::new();
        for i in 0..5 {
            let a = i as f32 * TAU / 5.0;
            sign.add_vertex(Vec3::new(a.cos(), 0.0, a.sin() + 2.0));
            uvs.push(Vec2::new(0.5 + 0.5 * a.cos(), 0.5 - 0.5 * a.sin()));
        }
        sign.add_face((0..5).collect(), uvs, 1);
        let paint = Material { name: "Paint".into(), texture: Some("bus.png".into()), normal_map: Some("bus_n.png".into()), orm_map: Some("bus_orm.png".into()), alpha: 1, ..Default::default() };
        let glass = Material { name: "Glass".into(), color: [0.2, 0.4, 0.6, 0.5], alpha: 2, emissive: [1.0, 0.5, 0.0], ..Default::default() };
        Scene {
            objects: vec![
                ExportObject { name: "Box".into(), mesh: cube, transform: Mat4::from_translation(Vec3::new(10.0, 20.0, 0.0)) },
                ExportObject { name: "Sign".into(), mesh: sign, transform: Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0)) * Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)) },
            ],
            materials: vec![paint, glass],
        }
    }

    /// Export the sample into a fresh folder: the scene, the file and the files written.
    fn run(format: Format) -> (Scene, PathBuf, Vec<PathBuf>) {
        let dir = folder(format.extension());
        let scene = sample(&dir);
        let path = dir.join(format!("scene.{}", format.extension()));
        let src = dir.join("src");
        let written = export(&scene, format, &path, &|name: &str| Some(src.join(name))).unwrap();
        assert_eq!(written[0], path);
        (scene, path, written)
    }

    fn names(written: &[PathBuf]) -> Vec<String> {
        written.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    fn total_triangles(scene: &Scene) -> usize {
        scene.objects.iter().map(|o| o.mesh.triangles(SMOOTH_ANGLE).triangle_count()).sum()
    }

    #[test]
    fn obj_keeps_polygons_and_writes_its_materials() {
        let (scene, path, written) = run(Format::Obj);
        assert_eq!(names(&written), ["scene.obj", "scene.mtl", "bus.png", "bus_n.png"]);
        let obj = fs::read_to_string(&path).unwrap();
        let count = |prefix: &str| obj.lines().filter(|l| l.starts_with(prefix)).count();
        assert_eq!(count("o "), 2);
        assert_eq!(count("v "), scene.objects.iter().map(|o| o.mesh.positions.len()).sum::<usize>());
        assert_eq!(count("f "), 7);
        assert!(obj.lines().any(|l| l.starts_with("f ") && l.split_whitespace().count() == 6), "the sign stays a pentagon");
        assert!(obj.contains("mtllib scene.mtl") && obj.contains("usemtl Paint") && obj.contains("usemtl Glass"));
        // the box's first corner, moved and turned y-up
        let p = scene.objects[0].mesh.positions[0] + Vec3::new(10.0, 20.0, 0.0);
        assert!(obj.contains(&format!("v {} {} {}", p.x, p.z, -p.y)));
        let mtl = fs::read_to_string(path.with_extension("mtl")).unwrap();
        for line in ["newmtl Paint", "map_Kd bus.png", "map_d bus.png", "map_Bump bus_n.png", "norm bus_n.png", "newmtl Glass", "d 0.5", "Tr 0.5", "Ke 1 0.5 0", "Kd 0.2 0.4 0.6"] {
            assert!(mtl.lines().any(|l| l == line), "{line} missing from\n{mtl}");
        }
        assert_eq!(fs::read(path.with_file_name("bus.png")).unwrap(), PICTURE);
    }

    /// The glTF document's checks shared by `.gltf` and `.glb`.
    fn check_gltf(doc: &gltf::Document, buffers: &[gltf::buffer::Data]) {
        assert_eq!(doc.nodes().count(), 2);
        let mesh = |name: &str| doc.meshes().find(|m| m.name() == Some(name)).unwrap();
        let boxm = mesh("Box");
        assert_eq!(boxm.primitives().count(), 2);
        let tris: usize = boxm.primitives().map(|p| p.reader(|b| Some(&buffers[b.index()].0[..])).read_indices().unwrap().into_u32().count() / 3).sum();
        assert_eq!(tris, 12);
        let first = boxm.primitives().next().unwrap();
        assert!(first.reader(|b| Some(&buffers[b.index()].0[..])).read_positions().unwrap().count() >= 8);
        let pos = first.get(&gltf::Semantic::Positions).unwrap();
        // x 9.5..10.5, north 19.5..20.5 (so -z), up 0..1
        assert_eq!(pos.min().unwrap(), json!([9.5, 0.0, -20.5]));
        assert_eq!(pos.max().unwrap(), json!([10.5, 1.0, -19.5]));
        // the mirrored sign still faces south: +z in glTF's frame
        let sign = mesh("Sign").primitives().next().unwrap();
        let r = sign.reader(|b| Some(&buffers[b.index()].0[..]));
        for n in r.read_normals().unwrap() {
            assert!((Vec3::from(n) - Vec3::Z).length() < 1e-5, "{n:?}");
        }
        assert_eq!(r.read_indices().unwrap().into_u32().count(), 9);
        let paint = doc.materials().find(|m| m.name() == Some("Paint")).unwrap();
        assert_eq!(paint.alpha_mode(), gltf::material::AlphaMode::Mask);
        assert_eq!(paint.alpha_cutoff(), Some(0.5));
        assert!(paint.pbr_metallic_roughness().base_color_texture().is_some());
        assert!(paint.pbr_metallic_roughness().metallic_roughness_texture().is_some());
        assert!(paint.normal_texture().is_some() && paint.occlusion_texture().is_some());
        let glass = doc.materials().find(|m| m.name() == Some("Glass")).unwrap();
        assert_eq!(glass.alpha_mode(), gltf::material::AlphaMode::Blend);
        assert_eq!(glass.emissive_factor(), [1.0, 0.5, 0.0]);
        assert_eq!(glass.pbr_metallic_roughness().base_color_factor(), [0.2, 0.4, 0.6, 0.5]);
        assert_eq!(doc.images().count(), 3);
    }

    #[test]
    fn gltf_reads_back_with_its_bin_and_textures_beside_it() {
        let (_, path, written) = run(Format::Gltf);
        assert_eq!(names(&written), ["scene.gltf", "scene.bin", "bus.png", "bus_n.png", "bus_orm.png"]);
        let g = gltf::Gltf::open(&path).unwrap();
        let buffers = gltf::import_buffers(&g.document, path.parent(), None).unwrap();
        check_gltf(&g.document, &buffers);
        for image in g.document.images() {
            let gltf::image::Source::Uri { uri, .. } = image.source() else { panic!("a .gltf refers to its pictures") };
            assert!(path.with_file_name(uri).is_file());
        }
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"generator\": \"openMapper\""));
    }

    #[test]
    fn glb_holds_its_pictures() {
        let (_, path, written) = run(Format::Glb);
        assert_eq!(names(&written), ["scene.glb"]);
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"glTF");
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize, bytes.len());
        let g = gltf::Gltf::from_slice(&bytes).unwrap();
        let buffers = gltf::import_buffers(&g.document, None, g.blob.clone()).unwrap();
        check_gltf(&g.document, &buffers);
        for image in g.document.images() {
            let gltf::image::Source::View { view, mime_type } = image.source() else { panic!("a .glb holds its pictures") };
            assert_eq!(mime_type, "image/png");
            assert_eq!(&buffers[0].0[view.offset()..view.offset() + view.length()], PICTURE);
        }
    }

    #[test]
    fn fbx_is_binary_and_reads_back() {
        let (scene, path, written) = run(Format::Fbx);
        assert_eq!(names(&written), ["scene.fbx", "bus.png", "bus_n.png"]);
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..23], b"Kaydara FBX Binary  \x00\x1a\x00");
        assert_eq!(u32::from_le_bytes(bytes[23..27].try_into().unwrap()), 7400);
        assert_eq!(&bytes[bytes.len() - 16..], &FBX_FOOTER_MAGIC);
        let fbx = ufbx::load_memory(&bytes, ufbx::LoadOpts { strict: true, ..Default::default() }).unwrap();
        assert!(fbx.metadata.warnings.is_empty(), "{:?}", fbx.metadata.warnings.iter().map(|w| w.description.to_string()).collect::<Vec<_>>());
        assert_eq!(fbx.metadata.version, 7400);
        assert!(!fbx.metadata.ascii);
        assert_eq!(fbx.settings.unit_meters, 1.0);
        assert_eq!(fbx.settings.axes.up, ufbx::CoordinateAxis::PositiveZ);
        assert_eq!(fbx.meshes.len(), 2);
        assert_eq!(fbx.materials.len(), 2);
        let node = |name: &str| fbx.nodes.iter().find(|n| n.element.name.as_ref() == name).unwrap();
        let boxn = node("Box");
        let boxm = boxn.mesh.as_ref().unwrap();
        let cube = &scene.objects[0].mesh;
        assert_eq!(boxm.num_vertices, cube.positions.len());
        assert_eq!(boxm.num_faces, 6);
        assert_eq!(boxm.num_triangles, 12);
        assert_eq!(boxn.materials.len(), 2);
        let v = boxm.vertices[0];
        let p = cube.positions[0] + Vec3::new(10.0, 20.0, 0.0);
        assert_eq!((v.x as f32, v.y as f32, v.z as f32), (p.x, p.y, p.z));
        let sign = node("Sign").mesh.as_ref().unwrap();
        assert_eq!(sign.num_faces, 1);
        assert_eq!(sign.faces[0].num_indices, 5);
        let paint = fbx.materials.iter().find(|m| m.element.name.as_ref() == "Paint").unwrap();
        let tex = paint.fbx.diffuse_color.texture.as_ref().unwrap();
        assert_eq!(tex.relative_filename.as_ref(), "bus.png");
        assert!(paint.fbx.normal_map.texture.is_some());
        let glass = fbx.materials.iter().find(|m| m.element.name.as_ref() == "Glass").unwrap();
        let e = glass.fbx.emission_color.value_vec4;
        assert_eq!((e.x, e.y, e.z), (1.0, 0.5, 0.0));
    }

    #[test]
    fn collada_has_metres_z_up_and_polygons() {
        let (_, path, written) = run(Format::Dae);
        assert_eq!(names(&written), ["scene.dae", "bus.png"]);
        let dae = fs::read_to_string(&path).unwrap();
        for part in ["<unit name=\"meter\" meter=\"1\"/>", "<up_axis>Z_UP</up_axis>", "<init_from>bus.png</init_from>", "<sampler2D>", "<polylist material=\"material1\" count=\"1\">", "<vcount>5</vcount>", "<instance_visual_scene url=\"#Scene\"/>"] {
            assert!(dae.contains(part), "{part} missing");
        }
        assert_eq!(dae.matches("<node ").count(), 2);
        assert_eq!(dae.matches("<polylist").count(), 3);
        // the pentagon's <p> has three numbers per corner
        let sign = &dae[dae.find("object1-mesh").unwrap()..];
        let p = &sign[sign.find("<p>").unwrap() + 3..sign.find("</p>").unwrap()];
        assert_eq!(p.split_whitespace().count(), 15);
    }

    #[test]
    fn usda_has_meshes_materials_and_subsets() {
        let (_, path, written) = run(Format::Usda);
        assert_eq!(names(&written), ["scene.usda", "bus.png", "bus_n.png", "bus_orm.png"]);
        let usd = fs::read_to_string(&path).unwrap();
        assert!(usd.starts_with("#usda 1.0\n"));
        for part in [
            "upAxis = \"Z\"",
            "metersPerUnit = 1",
            "def Xform \"Box\"",
            "def Xform \"Sign\"",
            "int[] faceVertexCounts = [5]",
            "uniform token orientation = \"rightHanded\"",
            "def GeomSubset \"Glass\"",
            "rel material:binding = </World/Looks/Glass>",
            "asset inputs:file = @./bus.png@",
            "uniform token info:id = \"UsdUVTexture\"",
            "float inputs:opacityThreshold = 0.5",
        ] {
            assert!(usd.contains(part), "{part} missing");
        }
        assert_eq!(usd.matches("def Mesh ").count(), 2);
        assert_eq!(usd.matches('{').count(), usd.matches('}').count());
    }

    #[test]
    fn stl_has_fifty_bytes_a_triangle() {
        let (scene, path, written) = run(Format::Stl);
        assert_eq!(written.len(), 1);
        let bytes = fs::read(&path).unwrap();
        let n = total_triangles(&scene);
        assert_eq!(n, 15);
        assert_eq!(bytes.len(), 84 + 50 * n);
        assert_eq!(u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize, n);
        assert!(!bytes.starts_with(b"solid"));
    }

    #[test]
    fn ply_has_its_header_and_records() {
        let (scene, path, _) = run(Format::Ply);
        let bytes = fs::read(&path).unwrap();
        let end = bytes.windows(11).position(|w| w == b"end_header\n").unwrap() + 11;
        let header = std::str::from_utf8(&bytes[..end]).unwrap();
        assert!(header.starts_with("ply\nformat binary_little_endian 1.0\n"));
        assert!(header.contains("property float s\nproperty float t\n") && header.contains("property list uchar int vertex_indices"));
        let nv: usize = scene.objects.iter().map(|o| { let mut m = o.mesh.clone(); m.transform(o.transform); m.triangles(SMOOTH_ANGLE).positions.len() }).sum();
        assert!(header.contains(&format!("element vertex {nv}\n")));
        assert!(header.contains("element face 15\n"));
        assert_eq!(bytes.len(), end + nv * 32 + 15 * 13);
    }

    #[test]
    fn three_mf_is_a_package_with_its_texture() {
        let (_, path, written) = run(Format::ThreeMf);
        assert_eq!(names(&written), ["scene.3mf"]);
        let mut zip = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
        let mut entries: Vec<String> = zip.file_names().map(|name| name.unwrap().into_owned()).collect();
        entries.sort();
        assert_eq!(entries, ["3D/3dmodel.model", "3D/Textures/bus.png", "3D/_rels/3dmodel.model.rels", "[Content_Types].xml", "_rels/.rels"]);
        let mut model = String::new();
        zip.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut model).unwrap();
        assert!(model.contains("unit=\"meter\""));
        assert_eq!(model.matches("<object ").count(), 2);
        assert_eq!(model.matches("<triangle ").count(), 15);
        assert_eq!(model.matches("<item ").count(), 2);
        assert!(model.contains("<base name=\"Glass\" displaycolor=\"#33669980\"/>"));
        assert!(model.contains("<m:texture2d id=") && model.contains("<m:texture2dgroup"));
        let mut types = String::new();
        zip.by_name("[Content_Types].xml").unwrap().read_to_string(&mut types).unwrap();
        assert!(types.contains("Extension=\"png\""));
        let mut pic = Vec::new();
        zip.by_name("3D/Textures/bus.png").unwrap().read_to_end(&mut pic).unwrap();
        assert_eq!(pic, PICTURE);
    }

    #[test]
    fn off_lists_positions_and_polygons() {
        let (scene, path, _) = run(Format::Off);
        let off = fs::read_to_string(&path).unwrap();
        let mut lines = off.lines();
        assert_eq!(lines.next(), Some("OFF"));
        let nv: usize = scene.objects.iter().map(|o| o.mesh.positions.len()).sum();
        assert_eq!(lines.next(), Some(format!("{nv} 7 0").as_str()));
        assert_eq!(off.lines().last().unwrap().split_whitespace().count(), 6);
    }

    #[test]
    fn x_holds_every_object_in_one_mesh() {
        let (scene, path, written) = run(Format::X);
        assert_eq!(names(&written), ["scene.x", "bus.png"]);
        let back = omsi_o3d::load_mesh(&path).unwrap();
        assert_eq!(back.triangles.len(), total_triangles(&scene));
        assert_eq!(back.materials.len(), 2);
        assert!(back.materials.iter().any(|m| m.texture == "bus.png"));
    }

    #[test]
    fn o3d_reads_back_in_openomsi() {
        let (scene, path, written) = run(Format::O3d);
        assert_eq!(names(&written), ["scene.o3d", "bus.png"]);
        let back = omsi_o3d::load_mesh(&path).unwrap();
        assert_eq!(back.version, 3);
        let mut merged = Mesh::default();
        for o in &scene.objects {
            let mut m = o.mesh.clone();
            m.transform(o.transform);
            merged.append(&m, 0);
        }
        let t = merged.triangles(SMOOTH_ANGLE);
        assert_eq!(back.vertices.len(), t.positions.len());
        assert_eq!(back.triangles.len(), t.triangle_count());
        assert_eq!(back.materials.len(), 2);
        let p = t.positions[0];
        assert_eq!(back.vertices[0].position.to_array(), [p.x, p.z, p.y]);
        // corners reversed as the frame is mirrored
        let c = &t.indices[..3];
        assert_eq!(back.triangles[0].indices, [c[0], c[2], c[1]]);
        let paint = &back.materials[0];
        assert_eq!(paint.texture, "bus.png");
        assert_eq!(paint.diffuse, [1.0, 1.0, 1.0, 1.0]);
        let glass = &back.materials[1];
        assert_eq!(glass.emissive, [1.0, 0.5, 0.0]);
        assert_eq!(glass.texture, "");
        assert_eq!(back.transform, Mat4::IDENTITY);
    }

    #[test]
    fn a_big_o3d_takes_32_bit_vertex_numbers() {
        let grid = Primitive::Grid { size: Vec2::new(256.0, 256.0), x_div: 256, y_div: 256 }.build();
        let mat = Material { texture: Some("Tür.png".into()), ..Default::default() };
        let (bytes, used) = to_o3d(&grid, &[mat]);
        assert_eq!(used, vec![0]);
        assert_eq!(&bytes[..4], &[0x84, 0x19, 3, 1]);
        let back = omsi_o3d::parse_o3d(&bytes).unwrap();
        assert!(back.vertices.len() > 65535);
        assert_eq!(back.triangles.len(), 2 * 256 * 256);
        assert!(back.triangles.iter().any(|t| t.indices.iter().any(|&i| i > 65535)));
        assert_eq!(back.materials[0].texture, "Tür.png");
        // a small one keeps 16-bit numbers
        let (small, _) = to_o3d(&Primitive::Cube { size: Vec3::ONE }.build(), &[]);
        assert_eq!(small[3], 0);
        assert_eq!(omsi_o3d::parse_o3d(&small).unwrap().triangles.len(), 12);
    }

    #[test]
    fn a_texture_already_beside_the_file_is_not_copied_onto_itself() {
        let dir = folder("in-place");
        let mut scene = sample(&dir);
        scene.objects.truncate(1);
        fs::write(dir.join("bus.png"), PICTURE).unwrap();
        let path = dir.join("box.obj");
        let d = dir.clone();
        let written = export(&scene, Format::Obj, &path, &|name: &str| Some(d.join(name))).unwrap();
        assert_eq!(names(&written), ["box.obj", "box.mtl"]);
        assert_eq!(fs::read(dir.join("bus.png")).unwrap(), PICTURE);
        assert!(fs::read_to_string(dir.join("box.mtl")).unwrap().contains("map_Kd bus.png"));
    }

    #[test]
    fn missing_textures_and_bad_faces_are_left_out_in_every_format() {
        let dir = folder("rough");
        let mut scene = sample(&dir);
        scene.materials[0].texture = Some("gone.dds".into());
        // a face naming a vertex that is not there, a face of two corners, an empty object
        scene.objects[0].mesh.faces.push(crate::Face::new(vec![0, 1, 99], vec![Vec2::ZERO; 3], 0));
        scene.objects[0].mesh.faces.push(crate::Face::new(vec![0, 1], vec![Vec2::ZERO; 2], 0));
        scene.objects.push(ExportObject { name: String::new(), mesh: Mesh::default(), transform: Mat4::IDENTITY });
        for format in Format::ALL {
            let path = dir.join(format!("rough.{}", format.extension()));
            let written = export(&scene, format, &path, &|_: &str| None).unwrap();
            assert!(!written.iter().any(|p| p.extension().is_some_and(|e| e == "png")), "{format:?}");
            assert!(fs::metadata(&path).unwrap().len() > 0, "{format:?}");
        }
        // and an empty scene still makes files
        for format in Format::ALL {
            let path = dir.join(format!("empty.{}", format.extension()));
            export(&Scene::default(), format, &path, &|_: &str| None).unwrap();
            assert!(path.is_file(), "{format:?}");
        }
        assert!(gltf::Gltf::open(dir.join("empty.gltf")).is_ok());
        assert!(ufbx::load_memory(&fs::read(dir.join("empty.fbx")).unwrap(), ufbx::LoadOpts::default()).is_ok());
    }
}

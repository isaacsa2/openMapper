//! Objects at another size. OMSI 2's map says only where an object stands and how it is
//! turned, so an object scaled in the view becomes a copy of its type made bigger or smaller:
//! `name~s1.5_1.5_2.sco` beside the original (in the content folder, so the original's
//! textures are found as they are), its meshes, collision mesh, lights and paths scaled.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use glam::DVec3;

use crate::i18n::tr;
use crate::install::Roots;

/// The type an object of `rel` is a scaled copy of, and by how much (the type itself, by
/// one, if it is none).
pub fn base_of(rel: &str) -> (String, DVec3) {
    let (dir, file) = match rel.rfind('\\') {
        Some(i) => (&rel[..=i], &rel[i + 1..]),
        None => ("", rel),
    };
    let stem = file.strip_suffix(".sco").or_else(|| file.strip_suffix(".SCO")).unwrap_or(file);
    if let Some((base, s)) = stem.rsplit_once("~s") {
        let f: Vec<f64> = s.split('_').filter_map(|v| v.parse().ok()).collect();
        if f.len() == 3 {
            return (format!("{dir}{base}.sco"), DVec3::new(f[0], f[1], f[2]));
        }
    }
    (rel.to_string(), DVec3::ONE)
}

fn scaled_name(base_rel: &str, s: DVec3) -> String {
    let stem = base_rel.strip_suffix(".sco").unwrap_or(base_rel);
    let n = |v: f64| {
        let t = format!("{v:.3}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    format!("{stem}~s{}_{}_{}.sco", n(s.x), n(s.y), n(s.z))
}

/// The type for an object of `rel` scaled by `factor` along its own axes (on top of any
/// scale it has): made in the content folder if it is not there yet.
pub fn variant(roots: &Roots, rel: &str, factor: DVec3) -> anyhow::Result<String> {
    let (base, had) = base_of(rel);
    let total = had * factor;
    if (total - DVec3::ONE).abs().max_element() < 1e-3 {
        return Ok(base);
    }
    let out_rel = scaled_name(&base, total);
    let out_path = omsi_cfg::resolve_path(&roots.content, &out_rel);
    if out_path.is_file() {
        return Ok(out_rel);
    }
    let src = [omsi_cfg::resolve_path(&roots.content, &base), omsi_cfg::resolve_path(&roots.omsi, &base)].into_iter().find(|p| p.is_file()).ok_or_else(|| anyhow::anyhow!("{base}: {}", tr("file not found", "файл не найден")))?;
    let src_dir = src.parent().map(Path::to_path_buf).unwrap_or_default();
    let out_dir = out_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let text = omsi_cfg::decode_text(&std::fs::read(&src)?);
    let lines = omsi_cfg::split_lines(&text);
    let suffix = out_rel.rsplit('\\').next().and_then(|f| f.strip_suffix(".sco")).and_then(|f| f.rsplit_once("~s")).map(|(_, s)| s.to_string()).unwrap_or_default();
    let mut out = String::new();
    let avg_xy = (total.x * total.y).sqrt();
    let avg = (total.x * total.y * total.z).cbrt();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        let _ = writeln!(out, "{line}");
        i += 1;
        let kw = omsi_cfg::keyword_of(line).map(|k| k.to_lowercase());
        let Some(kw) = kw else { continue };
        // the lines after the keyword: which are scaled and how
        let plan: &[Scale] = match kw.as_str() {
            "model" => anyhow::bail!("{}", tr("this object takes its model from a model file: it cannot be scaled", "модель этого объекта лежит в отдельном файле модели: его нельзя масштабировать")),
            "mesh" | "collision_mesh" | "crossing_heightdeformation" => &[Scale::Mesh],
            "friendlyname" => &[Scale::Name],
            "light_enh" => &[Scale::X, Scale::Y, Scale::Z, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Avg],
            "light_enh_2" => &[Scale::X, Scale::Y, Scale::Z, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Avg],
            "maplight" => &[Scale::X, Scale::Y, Scale::Z, Scale::Keep, Scale::Keep, Scale::Keep, Scale::Avg],
            "spotlight" => &[Scale::X, Scale::Y, Scale::Z],
            "path" | "path_2" => &[Scale::X, Scale::Y, Scale::Z, Scale::Keep, Scale::AvgXy, Scale::AvgXy],
            _ => &[],
        };
        for s in plan {
            let Some(v) = lines.get(i) else { break };
            i += 1;
            let num = |k: f64| v.trim().parse::<f64>().map(|x| fmt_num(x * k)).unwrap_or_else(|_| v.clone());
            let new = match s {
                Scale::Keep => v.clone(),
                Scale::X => num(total.x),
                Scale::Y => num(total.y),
                Scale::Z => num(total.z),
                Scale::Avg => num(avg),
                Scale::AvgXy => num(avg_xy),
                Scale::Name => format!("{} (×{})", v.trim(), suffix.replace('_', "×")),
                Scale::Mesh => {
                    let file = v.trim();
                    match scale_mesh(&src_dir, &out_dir, file, total, &suffix) {
                        Ok(f) => f,
                        Err(e) => {
                            log::warn!("{file}: {e}");
                            file.to_string()
                        }
                    }
                }
            };
            let _ = writeln!(out, "{new}");
        }
    }
    std::fs::create_dir_all(&out_dir)?;
    std::fs::write(&out_path, omsi_cfg::encode_text(&out.replace('\n', "\r\n")))?;
    crate::hot::own_change();
    Ok(out_rel)
}

#[derive(Clone, Copy)]
enum Scale {
    Keep,
    X,
    Y,
    Z,
    Avg,
    AvgXy,
    Name,
    Mesh,
}

fn fmt_num(v: f64) -> String {
    let t = format!("{v:.4}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    if t.is_empty() || t == "-" {
        "0".into()
    } else {
        t.to_string()
    }
}

/// A mesh of the object's (`file`, as its `.sco` names it) scaled into a `.x` beside the
/// copy; the name to write in its place.
fn scale_mesh(src_dir: &Path, out_dir: &Path, file: &str, s: DVec3, suffix: &str) -> anyhow::Result<String> {
    let in_model = omsi_cfg::resolve_path(&omsi_cfg::resolve_path(src_dir, "model"), file);
    let path: PathBuf = if omsi_cfg::vfs::is_file(&in_model) { in_model } else { omsi_cfg::resolve_path(src_dir, file) };
    let m = omsi_o3d::load_mesh(&path)?;
    let stem = Path::new(&file.replace('\\', "/")).file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "mesh".into());
    let name = format!("{stem}~s{suffix}.x");
    let dir = out_dir.join("model");
    std::fs::create_dir_all(&dir)?;
    // (mesh files are in Direct3D's frame: y is up, z forward)
    let k = glam::Vec3::new(s.x as f32, s.z as f32, s.y as f32);
    std::fs::write(dir.join(&name), x_of(&m, k))?;
    Ok(name)
}

/// A mesh as a `.x` text file, every material kept in its slot, positions scaled by `k`.
pub fn x_of(m: &omsi_o3d::Mesh, k: glam::Vec3) -> String {
    let n = m.vertices.len();
    let t = m.triangles.len();
    let flip = k.x * k.y * k.z < 0.0;
    let mut x = String::from("xof 0302txt 0032\n\nMesh {\n");
    let _ = writeln!(x, " {n};");
    for (i, v) in m.vertices.iter().enumerate() {
        let p = v.position * k;
        let _ = writeln!(x, " {:.6};{:.6};{:.6};{}", p.x, p.y, p.z, if i + 1 == n { ";" } else { "," });
    }
    let tri = |tr: &omsi_o3d::Triangle| if flip { [tr.indices[0], tr.indices[2], tr.indices[1]] } else { tr.indices };
    let _ = writeln!(x, " {t};");
    for (i, tr) in m.triangles.iter().enumerate() {
        let f = tri(tr);
        let _ = writeln!(x, " 3;{},{},{};{}", f[0], f[1], f[2], if i + 1 == t { ";" } else { "," });
    }
    // (normals of a stretched mesh: by the inverse scale)
    let _ = writeln!(x, " MeshNormals {{\n  {n};");
    for (i, v) in m.vertices.iter().enumerate() {
        let nn = (v.normal / k).normalize_or_zero();
        let _ = writeln!(x, "  {:.6};{:.6};{:.6};{}", nn.x, nn.y, nn.z, if i + 1 == n { ";" } else { "," });
    }
    let _ = writeln!(x, "  {t};");
    for (i, tr) in m.triangles.iter().enumerate() {
        let f = tri(tr);
        let _ = writeln!(x, "  3;{},{},{};{}", f[0], f[1], f[2], if i + 1 == t { ";" } else { "," });
    }
    x.push_str(" }\n");
    let _ = writeln!(x, " MeshTextureCoords {{\n  {n};");
    for (i, v) in m.vertices.iter().enumerate() {
        let _ = writeln!(x, "  {:.6};{:.6};{}", v.uv.x, v.uv.y, if i + 1 == n { ";" } else { "," });
    }
    x.push_str(" }\n");
    let mats = m.materials.len().max(1);
    let _ = writeln!(x, " MeshMaterialList {{\n  {mats};\n  {t};");
    for (i, tr) in m.triangles.iter().enumerate() {
        let _ = writeln!(x, "  {}{}", (tr.material as usize).min(mats - 1), if i + 1 == t { ";" } else { "," });
    }
    let fallback = omsi_o3d::Material::default();
    for i in 0..mats {
        let mat = m.materials.get(i).unwrap_or(&fallback);
        let d = mat.diffuse;
        let _ = writeln!(
            x,
            "  Material {{\n   {:.4};{:.4};{:.4};{:.4};;\n   {:.4};\n   {:.4};{:.4};{:.4};;\n   {:.4};{:.4};{:.4};;",
            d[0], d[1], d[2], d[3], mat.specular_power, mat.specular[0], mat.specular[1], mat.specular[2], mat.emissive[0], mat.emissive[1], mat.emissive[2]
        );
        if !mat.texture.is_empty() {
            let _ = writeln!(x, "   TextureFilename {{\n    \"{}\";\n   }}", mat.texture.replace('\\', "\\\\"));
        }
        x.push_str("  }\n");
    }
    x.push_str(" }\n}\n");
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scaled_name_reads_back_as_its_base_and_scale() {
        let n = scaled_name("Sceneryobjects\\A\\lamp.sco", DVec3::new(1.5, 2.0, 0.25));
        assert_eq!(n, "Sceneryobjects\\A\\lamp~s1.5_2_0.25.sco");
        let (b, s) = base_of(&n);
        assert_eq!(b, "Sceneryobjects\\A\\lamp.sco");
        assert_eq!(s, DVec3::new(1.5, 2.0, 0.25));
        assert_eq!(base_of("Sceneryobjects\\A\\lamp.sco").1, DVec3::ONE);
    }

    #[test]
    fn a_scaled_copy_scales_its_mesh_and_lights_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("om-scaled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = dir.join("content");
        let omsi = dir.join("omsi");
        let obj = omsi.join("Sceneryobjects").join("T");
        std::fs::create_dir_all(obj.join("model")).unwrap();
        let cube = openmapper_mesh::primitives::Primitive::Cube { size: glam::Vec3::ONE }.build();
        let (x, _) = openmapper_mesh::export::to_x(&cube, &[openmapper_mesh::Material::default()]);
        std::fs::write(obj.join("model").join("box.x"), x).unwrap();
        let sco = "[friendlyname]\nBox\n\n[mesh]\nbox.x\n\n[maplight]\n1\n2\n3\n1\n1\n1\n5\n";
        std::fs::write(obj.join("box.sco"), omsi_cfg::encode_text(&sco.replace('\n', "\r\n"))).unwrap();
        let roots = Roots { omsi: omsi.clone(), content: content.clone() };
        let rel = variant(&roots, "Sceneryobjects\\T\\box.sco", DVec3::new(2.0, 2.0, 2.0)).unwrap();
        assert_eq!(rel, "Sceneryobjects\\T\\box~s2_2_2.sco");
        let path = omsi_cfg::resolve_path(&content, &rel);
        let text = omsi_cfg::decode_text(&std::fs::read(&path).unwrap());
        assert!(text.contains("box~s2_2_2.x"), "{text}");
        assert!(text.contains("\r\n2\r\n4\r\n6\r\n"), "{text}");
        let m = omsi_o3d::load_mesh(&content.join("Sceneryobjects/T/model/box~s2_2_2.x")).unwrap();
        let hi = m.vertices.iter().map(|v| v.position.y).fold(f32::MIN, f32::max);
        assert!((hi - 2.0).abs() < 1e-4);
        // scaled again by a half: the original back
        assert_eq!(variant(&roots, &rel, DVec3::splat(0.5)).unwrap(), "Sceneryobjects\\T\\box.sco");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

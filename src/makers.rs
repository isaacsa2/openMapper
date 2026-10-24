//! More objects made in one click, ready to put on the map and edit further: a picture or a
//! repeating texture on a flat panel (a decal, a pattern, a sign, a billboard), a light
//! source, and a sound source.

use glam::{Mat4, Vec2, Vec3};
use openmapper_mesh::primitives::Primitive;
use openmapper_mesh::{Material, Mesh, Model, Part};

use crate::editable::{Light, LightKind, Source, When};

/// What the makers are set to (kept by the interface).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// A picture's width (m); its height follows the picture's own proportions.
    pub picture_width: f32,
    /// How often the picture repeats along and across the panel (a pattern; 1 and 1: once).
    pub picture_repeat: [f32; 2],
    /// Stands up like a sign (else it lies on the ground like a decal).
    pub picture_standing: bool,
    pub light_color: [u8; 3],
    /// How far the light reaches on the ground (m).
    pub light_range: f32,
    pub light_night_only: bool,
    /// A sound's reach (m) and volume (0..1).
    pub sound_radius: f32,
    pub sound_volume: f32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { picture_width: 4.0, picture_repeat: [1.0, 1.0], picture_standing: false, light_color: [255, 220, 160], light_range: 12.0, light_night_only: true, sound_radius: 40.0, sound_volume: 0.8 }
    }
}

/// A flat panel (one side up, one down, so it shows from both) `w` × `h` m, its picture
/// `repeat` times along and across; lying on the ground a hair above it, or standing.
pub fn picture(texture: &str, aspect: f32, alpha: u8, s: &Settings) -> Source {
    let w = s.picture_width.clamp(0.1, 300.0);
    let h = (w / aspect.max(0.01)).clamp(0.1, 300.0);
    let mut front = Primitive::Plane { size: Vec2::new(w, h) }.build();
    for f in &mut front.faces {
        for uv in &mut f.uvs {
            uv.x *= s.picture_repeat[0].max(0.01) / w;
            uv.y *= s.picture_repeat[1].max(0.01) / h;
        }
    }
    let mut back = front.clone();
    back.flip_normals();
    let mut mesh = Mesh::default();
    mesh.append(&front, 0);
    mesh.append(&back, 0);
    let place = if s.picture_standing { Mat4::from_translation(Vec3::new(0.0, 0.0, h / 2.0)) * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2) } else { Mat4::from_translation(Vec3::new(0.0, 0.0, 0.03)) };
    mesh.transform(place);
    let material = Material { name: "Picture".into(), color: [1.0, 1.0, 1.0, 1.0], texture: Some(texture.to_string()), alpha, specular: [0.0; 3], ..Default::default() };
    Source { name: "Picture".into(), model: Model { parts: vec![Part::new("Panel", mesh)], materials: vec![material] }, ..Default::default() }
}

/// A light: a small lamp that glows and lights the ground round it (at night, or always).
pub fn light_source(s: &Settings) -> Source {
    let bulb = {
        let mut m = Primitive::UvSphere { radius: 0.12, segments: 12, rings: 6 }.build();
        m.transform(Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0)));
        for f in &mut m.faces {
            f.material = 0;
            f.smooth = true;
        }
        m
    };
    let c = s.light_color.map(|v| v as f32 / 255.0);
    let materials = vec![Material { name: "Bulb".into(), color: [c[0], c[1], c[2], 1.0], emissive: c, ..Default::default() }];
    let when = if s.light_night_only { When::Night } else { When::Always };
    let glow = Light { kind: LightKind::Glow, pos: Vec3::new(0.0, 0.0, 1.0), color: s.light_color, size: 0.8, cone: None, when: when.clone(), ..Default::default() };
    let ground = Light { kind: LightKind::Ground, pos: Vec3::new(0.0, 0.0, 1.0), color: s.light_color, range: s.light_range.clamp(1.0, 200.0), when, ..Default::default() };
    Source { name: "Light source".into(), model: Model { parts: vec![Part::new("Bulb", bulb)], materials }, lights: vec![glow, ground], ..Default::default() }
}

/// A sound source: a marker only the editor shows, and OMSI's sound description file (what
/// plays, how loud, how far it carries) written beside the object by [`write_sound`].
pub fn sound_source(stem: &str) -> Source {
    let mut m = Primitive::Cube { size: Vec3::new(0.4, 0.4, 0.4) }.build();
    m.transform(Mat4::from_translation(Vec3::new(0.0, 0.0, 0.5)));
    for f in &mut m.faces {
        f.material = 0;
    }
    let materials = vec![Material::colored("Marker", [0.2, 0.7, 1.0, 1.0])];
    Source {
        name: "Sound source".into(),
        model: Model { parts: vec![Part::new("Marker", m)], materials },
        extra: format!("[onlyeditor]\n\n[sound]\nsound\\{stem}.cfg\n"),
        ..Default::default()
    }
}

/// The sound description (`sound\<stem>.cfg`) and the sound file beside it, in the object's
/// folder.
pub fn write_sound(dir: &std::path::Path, stem: &str, wav: &std::path::Path, s: &Settings) -> anyhow::Result<()> {
    let sound_dir = dir.join("sound");
    std::fs::create_dir_all(&sound_dir)?;
    let file = wav.file_name().map(|f| f.to_string_lossy().to_string()).ok_or_else(|| anyhow::anyhow!("no sound file"))?;
    std::fs::copy(wav, sound_dir.join(&file))?;
    // (the layout of OMSI 2's own ambient sounds: the file and the volume, the position of
    // the source in the object and how far it carries)
    let cfg = format!("\n\n[sound]\n{file}\n{:.2}\n\n[3d]\n0\n0\n0\n{}\n", s.sound_volume.clamp(0.0, 1.0), s.sound_radius.clamp(1.0, 1000.0).round());
    std::fs::write(sound_dir.join(format!("{stem}.cfg")), omsi_cfg::encode_text(&cfg.replace('\n', "\r\n")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_makers_write_objects_openomsi_reads() {
        let dir = std::env::temp_dir().join(format!("om-makers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let roots = crate::install::Roots { omsi: dir.join("omsi"), content: dir.join("content") };
        let s = Settings::default();
        // a light: a glow and a pool of light on the ground
        let rel = crate::editable::new_rel(&roots, "M", "Light");
        crate::editable::save(&roots, &rel, &light_source(&s)).unwrap();
        let sco = omsi_scenery::sco::SceneryObject::load(&omsi_cfg::resolve_path(&roots.content, &rel)).unwrap();
        assert_eq!(sco.map_lights.len(), 1);
        assert_eq!(sco.model.meshes[0].light_enh_2.len(), 1);
        // a sound: named by the object, and its description beside it
        let rel = crate::editable::new_rel(&roots, "M", "Sound");
        let (folder, stem) = crate::editable::folder_of(&roots, &rel);
        crate::editable::save(&roots, &rel, &sound_source(&stem)).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("bird.wav");
        std::fs::write(&wav, b"RIFF").unwrap();
        write_sound(&folder, &stem, &wav, &s).unwrap();
        let sco = omsi_scenery::sco::SceneryObject::load(&omsi_cfg::resolve_path(&roots.content, &rel)).unwrap();
        assert_eq!(sco.sound.as_deref(), Some(format!("sound\\{stem}.cfg").as_str()));
        assert!(folder.join("sound").join("bird.wav").is_file());
        assert!(folder.join("sound").join(format!("{stem}.cfg")).is_file());
        // a picture: one panel with the texture
        let rel = crate::editable::new_rel(&roots, "M", "Picture");
        crate::editable::save(&roots, &rel, &picture("p.png", 2.0, 1, &s)).unwrap();
        let sco = omsi_scenery::sco::SceneryObject::load(&omsi_cfg::resolve_path(&roots.content, &rel)).unwrap();
        assert_eq!(sco.model.meshes.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

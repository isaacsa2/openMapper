//! The asset library: every scenery object and spline type of the installation and the
//! content folder, sorted into a few plain categories a beginner can find things in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::i18n::tr;
use crate::install::Roots;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Object,
    Spline,
}

/// What an asset is, read from its own file (its OMSI tags and traffic paths), never from
/// its name: how it is organised is left to its folder (see `Asset::dir`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    // objects
    /// Anything to look at (buildings, plants, street furniture...: their folders say which).
    Objects,
    /// `[tree]`: a tree card of a texture.
    Trees,
    /// `[busstop]` and `[entrypoint]`.
    Stops,
    /// Has `[splinehelper]`s or traffic lights: a junction or crossing piece.
    Junctions,
    // splines
    /// Carries a street lane.
    Roads,
    /// Carries a rail lane.
    Tracks,
    /// Carries only footway lanes.
    Paths,
    /// Visible, no lanes: fences, walls, kerbs, embankments.
    Decoration,
    /// Not meant to be seen: AI-only splines and helpers (`[onlyeditor]`, no visible
    /// profile, no mesh), help arrows, parking spaces and sound sources.
    Technical,
}

impl Category {
    pub const OBJECTS: [Category; 5] = [Category::Objects, Category::Trees, Category::Stops, Category::Junctions, Category::Technical];
    pub const SPLINES: [Category; 5] = [Category::Roads, Category::Tracks, Category::Paths, Category::Decoration, Category::Technical];

    pub fn label(self) -> &'static str {
        match self {
            Category::Objects => tr("Objects", "Объекты"),
            Category::Trees => tr("Trees", "Деревья"),
            Category::Stops => tr("Stops and start points", "Остановки и точки старта"),
            Category::Junctions => tr("Junctions", "Перекрёстки"),
            Category::Roads => tr("Roads", "Дороги"),
            Category::Tracks => tr("Tracks", "Рельсы"),
            Category::Paths => tr("Footpaths", "Тротуары"),
            Category::Decoration => tr("Fences, walls, kerbs", "Заборы, стены, бордюры"),
            Category::Technical => tr("Technical (AI, helpers)", "Технические (ИИ, хелперы)"),
        }
    }

    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Category::Objects => Icon::Building,
            Category::Trees => Icon::Tree,
            Category::Stops => Icon::Bus,
            Category::Junctions => Icon::Junction,
            Category::Roads => Icon::Road,
            Category::Tracks => Icon::Tram,
            Category::Paths => Icon::Walk,
            Category::Decoration => Icon::Fence,
            Category::Technical => Icon::Settings,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Asset {
    pub kind: Kind,
    /// Path relative to its content root, with OMSI's backslashes (what a map file names).
    pub rel: String,
    pub name: String,
    /// Its folder below `Sceneryobjects` / `Splines` ("Streetobjects_MC\\Lampen"): the
    /// catalogue's tree.
    pub dir: String,
    pub category: Category,
    /// Lower-case text searched in.
    pub haystack: String,
    /// From the content folder (a mod or an import).
    pub from_content: bool,
    // objects
    pub is_bus_stop: bool,
    pub is_entry_point: bool,
    /// `[carpark_p]`: a place for a parked car.
    pub is_car_park: bool,
    pub abs_height: bool,
    /// Lights the ground round it at night (`[maplight]`: a street lamp).
    pub lamp: bool,
    /// A signal head (`[trafficlight]`, no program of its own): it lights only when linked to
    /// a crossing (`[varparent]`).
    pub light_head: bool,
    /// Runs a traffic light program (`[traffic_light]`s): a crossing signal heads link to.
    pub light_program: bool,
    /// Has `[splinehelper]` points: a junction or crossing piece that splines snap to.
    pub helpers: Vec<omsi_scenery::sco::SplineHelper>,
    // splines
    /// Half the width of the drawn profile (m).
    pub half_width: f32,
    pub lanes: (u8, u8, u8),
}

/// Messages of the background scan.
pub enum ScanMsg {
    Found(Vec<Asset>),
    Done,
}

#[derive(Default)]
pub struct Library {
    pub assets: Vec<Asset>,
    pub scanning: bool,
    rx: Option<mpsc::Receiver<ScanMsg>>,
    /// Where each asset is in `assets`, by its lower-case path (`find` is asked many times
    /// a frame).
    index: HashMap<String, usize>,
}

impl Library {
    /// Scan both roots in the background.
    pub fn start_scan(&mut self, roots: &Roots) {
        let (tx, rx) = mpsc::channel();
        let dirs: Vec<(PathBuf, bool)> = vec![(roots.content.clone(), true), (roots.omsi.clone(), false)];
        std::thread::spawn(move || scan(dirs, tx));
        self.assets.clear();
        self.index.clear();
        self.rx = Some(rx);
        self.scanning = true;
    }

    /// Take what the scan found so far.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    ScanMsg::Found(v) => {
                        for a in v {
                            // the content folder's copy hides the installation's
                            let key = a.rel.to_lowercase();
                            if !self.index.contains_key(&key) {
                                self.index.insert(key, self.assets.len());
                                self.assets.push(a);
                            }
                        }
                        changed = true;
                    }
                    ScanMsg::Done => {
                        self.scanning = false;
                        self.assets.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                        self.index = self.assets.iter().enumerate().map(|(i, a)| (a.rel.to_lowercase(), i)).collect();
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    pub fn find(&self, rel: &str) -> Option<&Asset> {
        self.index.get(&rel.to_lowercase()).and_then(|&i| self.assets.get(i))
    }

    /// Assets of a kind and category matching a search (words in any order).
    pub fn filter<'a>(&'a self, kind: Kind, cat: Option<Category>, search: &str) -> impl Iterator<Item = &'a Asset> + 'a + use<'a> {
        let words: Vec<String> = search.to_lowercase().split_whitespace().map(str::to_string).collect();
        self.assets
            .iter()
            .filter(move |a| a.kind == kind && cat.map(|c| c == a.category).unwrap_or(true) && words.iter().all(|w| a.haystack.contains(w)))
    }

    pub fn count(&self, kind: Kind, cat: Category) -> usize {
        self.assets.iter().filter(|a| a.kind == kind && a.category == cat).count()
    }
}

fn scan(dirs: Vec<(PathBuf, bool)>, tx: mpsc::Sender<ScanMsg>) {
    for (root, from_content) in dirs {
        for (sub, ext, kind) in [("Splines", "sli", Kind::Spline), ("Sceneryobjects", "sco", Kind::Object)] {
            let Some(base) = find_child(&root, sub) else { continue };
            let mut files = Vec::new();
            walk(&base, ext, &mut files);
            let mut batch = Vec::new();
            for f in files {
                if let Some(a) = describe(&root, &f, kind, from_content) {
                    batch.push(a);
                }
                if batch.len() >= 64 {
                    let _ = tx.send(ScanMsg::Found(std::mem::take(&mut batch)));
                }
            }
            let _ = tx.send(ScanMsg::Found(batch));
        }
    }
    let _ = tx.send(ScanMsg::Done);
}

/// A folder's child of that name whatever its case.
fn find_child(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.is_dir() && p.file_name().map(|f| f.to_string_lossy().eq_ignore_ascii_case(name)).unwrap_or(false))
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, ext, out);
        } else if p.extension().map(|x| x.to_string_lossy().eq_ignore_ascii_case(ext)).unwrap_or(false) {
            out.push(p);
        }
    }
}

/// "str_2spur_11m_SeeburgerStr1" > "Str 2spur 11m SeeburgerStr1".
fn pretty(stem: &str) -> String {
    let s = stem.replace(['_', '-'], " ");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

fn describe(root: &Path, path: &Path, kind: Kind, from_content: bool) -> Option<Asset> {
    let rel = path.strip_prefix(root).ok()?.to_string_lossy().replace('/', "\\");
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let folder = path.parent().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let parts: Vec<&str> = rel.split('\\').collect();
    let dir = if parts.len() > 2 { parts[1..parts.len() - 1].join("\\") } else { String::new() };
    let mut a = Asset {
        kind,
        rel: rel.clone(),
        name: pretty(&stem),
        dir,
        category: Category::Objects,
        haystack: String::new(),
        from_content,
        is_bus_stop: false,
        is_entry_point: false,
        is_car_park: false,
        abs_height: false,
        lamp: false,
        light_head: false,
        light_program: false,
        helpers: Vec::new(),
        half_width: 0.0,
        lanes: (0, 0, 0),
    };
    let mut groups = String::new();
    match kind {
        Kind::Object => {
            let sco = match omsi_scenery::SceneryObject::load(path) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("catalogue: {} cannot be read: {e}", path.display());
                    return None;
                }
            };
            if !sco.friendly_name.trim().is_empty() {
                a.name = sco.friendly_name.trim().to_string();
            }
            groups = sco.groups.join(" ").to_lowercase();
            a.is_bus_stop = sco.is_bus_stop;
            a.is_entry_point = sco.is_entry_point;
            a.is_car_park = sco.is_car_park;
            a.abs_height = sco.absolute_height();
            a.lamp = !sco.map_lights.is_empty();
            a.light_head = sco.is_traffic_light && sco.traffic_lights.is_empty();
            a.light_program = !sco.traffic_lights.is_empty();
            a.helpers = sco.spline_helpers.clone();
            let has_mesh = !sco.model.meshes.is_empty() || sco.model_file.is_some() || sco.tree.is_some();
            a.category = if sco.is_bus_stop || sco.is_entry_point {
                Category::Stops
            } else if sco.tree.is_some() {
                // (before what only the editor shows: a tree's helper box is `[onlyeditor]`,
                // its tree is seen in the game)
                Category::Trees
            } else if sco.only_editor || sco.is_help_arrow || sco.is_car_park || !has_mesh {
                Category::Technical
            } else if !sco.spline_helpers.is_empty() || sco.is_traffic_light || !sco.traffic_lights.is_empty() {
                Category::Junctions
            } else {
                Category::Objects
            };
        }
        Kind::Spline => {
            let sli = match omsi_scenery::Spline::load(path) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("catalogue: {} cannot be read: {e}", path.display());
                    return None;
                }
            };
            let mut w = 0.0f32;
            for p in &sli.profiles {
                for q in &p.points {
                    w = w.max(q.x.abs());
                }
            }
            a.half_width = w;
            let (mut st, mut sw, mut rl) = (0u8, 0u8, 0u8);
            for p in &sli.paths {
                match p.kind {
                    0 => st += 1,
                    1 => sw += 1,
                    2 => rl += 1,
                    _ => {}
                }
            }
            a.lanes = (st, sw, rl);
            let visible = sli.profiles.iter().any(|p| p.points.len() >= 2) && !sli.only_editor;
            a.category = if !visible {
                Category::Technical
            } else if rl > 0 {
                Category::Tracks
            } else if st > 0 {
                Category::Roads
            } else if sw > 0 {
                Category::Paths
            } else {
                Category::Decoration
            };
        }
    }
    a.haystack = format!("{} {} {} {}", a.name, rel, groups, folder).to_lowercase();
    Some(a)
}

//! The editor: the open map, the tools and what the mouse does with them.

use std::time::Instant;

use glam::{DVec2, DVec3, Vec3};
use omsi_map::{EntryPoint, MapObject, MapSpline, TERRAIN_SAMPLES};

use crate::assets::Assets;
use crate::doc::{tile_of, tile_origin, tile_size, Document, Sel, TileKey};
use crate::history::History;
use crate::i18n::tr;
use crate::library::{Asset, Library};
use crate::render::{object_matrix, Brush, FrameExtras, Ghost, OverlayPoint};
use crate::reshape::{self, Handle};
use crate::roads::{self, Anchor, Seg};
use crate::selection::{self, Origin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Terrain,
    Paint,
    Roads,
    Objects,
    /// What the game uses but does not show: start points, stops, car parks, AI paths,
    /// sound sources and other helpers.
    Logic,
    Tiles,
}

impl Tool {
    pub const ALL: [Tool; 7] = [Tool::Select, Tool::Roads, Tool::Objects, Tool::Logic, Tool::Terrain, Tool::Paint, Tool::Tiles];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => tr("Select", "Выбор"),
            Tool::Terrain => tr("Terrain", "Рельеф"),
            Tool::Paint => tr("Paint", "Покраска"),
            Tool::Roads => tr("Roads", "Дороги"),
            Tool::Objects => tr("Objects", "Объекты"),
            Tool::Logic => tr("Game logic", "Служебное"),
            Tool::Tiles => tr("Tiles", "Тайлы"),
        }
    }

    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Tool::Select => Icon::Select,
            Tool::Terrain => Icon::Terrain,
            Tool::Paint => Icon::Paint,
            Tool::Roads => Icon::Road,
            Tool::Objects => Icon::Objects,
            Tool::Logic => Icon::Sensors,
            Tool::Tiles => Icon::Tiles,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Tool::Select => "1",
            Tool::Roads => "2",
            Tool::Objects => "3",
            Tool::Logic => "4",
            Tool::Terrain => "5",
            Tool::Paint => "6",
            Tool::Tiles => "7",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainMode {
    Raise,
    Lower,
    Smooth,
    Flatten,
    /// Rolls and dips of the natural ground.
    Roughen,
    /// An even slope from where it starts to where it ends, as wide as the brush.
    Ramp,
    /// A lake or a river bed: the ground under the brush dug to `water_depth` under the water's
    /// level (the level of the stroke's start, or the tile's water), water laid on the tiles
    /// it touches.
    Water,
}

impl TerrainMode {
    pub const ALL: [TerrainMode; 7] = [TerrainMode::Raise, TerrainMode::Lower, TerrainMode::Smooth, TerrainMode::Flatten, TerrainMode::Roughen, TerrainMode::Ramp, TerrainMode::Water];
    pub fn label(self) -> &'static str {
        match self {
            TerrainMode::Raise => tr("Raise", "Поднять"),
            TerrainMode::Lower => tr("Lower", "Опустить"),
            TerrainMode::Smooth => tr("Smooth", "Сгладить"),
            TerrainMode::Flatten => tr("Flatten", "Выровнять"),
            TerrainMode::Roughen => tr("Roughen", "Неровности"),
            TerrainMode::Ramp => tr("Ramp", "Пандус"),
            TerrainMode::Water => tr("Lake / river", "Озеро / река"),
        }
    }
    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            TerrainMode::Raise => Icon::Raise,
            TerrainMode::Lower => Icon::Lower,
            TerrainMode::Smooth => Icon::Smooth,
            TerrainMode::Flatten => Icon::Flatten,
            TerrainMode::Roughen => Icon::Terrain,
            TerrainMode::Ramp => Icon::Route,
            TerrainMode::Water => Icon::Lower,
        }
    }
}

/// Where the knife would cut: the road (tile, id), how far along it and how long it is, the
/// point and the road's way there; `fits` while both pieces would be a metre or longer.
#[derive(Debug, Clone, Copy)]
pub struct CutPreview {
    pub key: TileKey,
    pub id: i64,
    pub at: f64,
    pub length: f64,
    pub pos: DVec3,
    pub dir: DVec2,
    pub fits: bool,
}

/// A short message at the bottom of the screen.
pub struct Toast {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

/// What the Logic tool places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicMode {
    /// `[entrypoint]`: where the bus appears.
    Start,
    /// `[busstop]` objects.
    Stops,
    /// `[carpark_p]`: where parked cars stand, one by one or in a row along a road.
    Parking,
    /// Invisible splines with lanes for the AI's cars or people.
    Paths,
    /// Sound sources, help arrows and other `[onlyeditor]` helpers.
    Other,
}

impl LogicMode {
    pub const ALL: [LogicMode; 5] = [LogicMode::Start, LogicMode::Stops, LogicMode::Parking, LogicMode::Paths, LogicMode::Other];

    pub fn label(self) -> &'static str {
        match self {
            LogicMode::Start => tr("Start points", "Точки старта"),
            LogicMode::Stops => tr("Bus stops", "Остановки"),
            LogicMode::Parking => tr("Car parks", "Парковки"),
            LogicMode::Paths => tr("AI paths", "Пути ИИ"),
            LogicMode::Other => tr("Other", "Прочее"),
        }
    }

    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            LogicMode::Start => Icon::Flag,
            LogicMode::Stops => Icon::Bus,
            LogicMode::Parking => Icon::Parking,
            LogicMode::Paths => Icon::Route,
            LogicMode::Other => Icon::Sound,
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            LogicMode::Start => tr("Where the bus appears when a drive starts. Put it on a road or in the depot, facing the way to drive.", "Где появляется автобус в начале поездки. Ставьте на дорогу или в депо, лицом по ходу движения."),
            LogicMode::Stops => tr("Where passengers wait. Put it beside the road at the kerb; name it on the right.", "Где ждут пассажиры. Ставьте у бордюра; имя задаётся справа."),
            LogicMode::Parking => tr("Where parked cars stand. Click for one; with a row on, click the start and end of a row along the kerb.", "Где стоят припаркованные машины. Клик: одно место; с рядом: клик в начале и в конце ряда вдоль бордюра."),
            LogicMode::Paths => tr("Lanes the AI's cars or people follow. Draw like a road: it snaps to lane ends (green: joined, red: dead end) and along lanes. A car path runs the way you draw it.", "Полосы, по которым едут машины ИИ или идут люди. Рисуется как дорога: прилипает к концам полос (зелёные: соединены, красные: тупик) и вдоль полос. Путь машин идёт в ту сторону, куда вы его рисуете."),
            LogicMode::Other => tr("Sound sources, help arrows and other helpers the game does not show.", "Источники звука, стрелки-подсказки и прочие помощники, которых не видно в игре."),
        }
    }
}

/// What the Select tool's drag does to the selection: any of it as it comes (Select), or
/// one thing only, by handles drawn on the selection (Move, Rotate, Scale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Xform {
    Select,
    Move,
    Rotate,
    Scale,
}

impl Xform {
    pub const ALL: [Xform; 4] = [Xform::Select, Xform::Move, Xform::Rotate, Xform::Scale];

    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Xform::Select => Icon::Select,
            Xform::Move => Icon::Move,
            Xform::Rotate => Icon::Rotate,
            Xform::Scale => Icon::Scale,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Xform::Select => tr("Select", "Выбор"),
            Xform::Move => tr("Move (V): drag an arrow along X, Y or Z, a square in its plane, the circle over the ground", "Перемещение (V): тяните стрелку вдоль X, Y или Z, квадрат — в его плоскости, круг — по земле"),
            Xform::Rotate => tr("Rotate (C): drag a ring to turn about X, Y or Z", "Поворот (C): тяните кольцо, чтобы вращать вокруг X, Y или Z"),
            Xform::Scale => tr("Scale (X): drag an arrow to scale along that axis, a square for two, the circle for all three", "Масштаб (X): тяните стрелку для масштаба по оси, квадрат — по двум, круг — по всем трём"),
        }
    }
}

/// What the left mouse button is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    None,
    Brush,
    /// Moving the selection: where it was grabbed; started once the mouse has gone far
    /// enough that it is not a click.
    Move { grab: DVec2, started: bool },
    /// A handle of the Move / Rotate / Scale gizmo held.
    Gizmo { grab: crate::gizmo::Grab, started: bool },
    /// A handle of the gizmo on the vertices, edges or faces of a mesh being edited.
    MeshGizmo { grab: crate::gizmo::Grab, started: bool },
    /// Dragging a grip of the selected spline.
    Reshape(TileKey, i64, Handle),
}

/// How far the mouse must go before a press on the selection moves it (m).
const MOVE_THRESHOLD: f64 = 0.3;

pub struct Editor {
    pub doc: Option<Document>,
    pub history: History,
    pub tool: Tool,
    /// The selection: the one shown in the properties (None: nothing selected, whatever
    /// `also` holds) and the others selected with it.
    pub selected: Option<Sel>,
    pub also: Vec<Sel>,
    pub hover: Option<Sel>,
    /// The grip of the selected spline under the cursor.
    pub hover_handle: Option<Handle>,
    /// The box being dragged out on the screen (window points: where it began, where it is).
    pub select_box: Option<[(f32, f32); 2]>,
    /// What was copied.
    pub clip: Option<crate::clipboard::Clip>,
    /// Where the selection stood when a move began.
    move_from: Vec<Origin>,
    /// A reshape being dragged: its plan, or why it cannot be.
    reshape: Option<Result<reshape::Plan, &'static str>>,
    /// What the editor has learnt of the types it edits (models load in the background):
    /// objects' absolute height and spline helpers, splines' half widths (0: not a road one
    /// sees, the ground is left as it is).
    known_helpers: std::collections::HashMap<String, (bool, Vec<[f32; 4]>)>,
    known_half: std::collections::HashMap<String, f64>,
    /// The road ends the junctions being moved hold, and how far the move has gone.
    move_arms: Vec<(Sel, Vec<selection::Arm>)>,
    move_delta: DVec2,
    /// The Select tool's mode: plain selecting, or the Move / Rotate / Scale handles.
    pub xform: Xform,
    /// The view the handles are seen in this frame (camera, its size, the cursor).
    gizmo_view: Option<crate::gizmo::View>,
    /// The selected things when a handle was taken: height (above their ground or
    /// absolute), heading / pitch / bank and world position.
    move_z_from: Vec<f64>,
    move_rot_from: Vec<[f64; 3]>,
    move_world_from: Vec<DVec3>,
    /// The size each object being scaled has come to (its own axes), drawn so until it is let
    /// go; then the app gives them scaled copies of their files.
    pub scale_preview: Vec<(Sel, DVec3)>,
    pub scale_requests: Vec<(Sel, DVec3)>,
    /// The selected openMapper object's source, and its mesh being edited (Tab).
    pub mesh: Option<crate::meshedit::Session>,
    /// What a drag has come to, for the status bar: metres, degrees or a factor.
    pub xform_value: f64,
    /// How those ends would follow, shown while dragging.
    carry_preview: Vec<reshape::Plan>,
    /// A length and radius typed for a road, put in at the next tick (where the junctions
    /// its neighbour may end at are known).
    pub shape_request: Option<(TileKey, i64, f64, f64)>,
    /// GPS tracks shown to trace over (world metres; Map > GPS tracks to trace).
    pub trace_lines: Vec<Vec<DVec2>>,
    /// The camera asked to go over this point (a window's "go there").
    pub look_request: Option<DVec2>,
    /// The ground under the selected objects is to be levelled at the next tick.
    pub level_request: bool,
    /// The last click on something with Select, and when (a second soon on a road: its chain).
    last_click: Option<(Sel, Instant)>,
    last_mesh_click: Option<(crate::meshedit::Hit, Instant)>,
    /// openMapper's junctions whose arms changed: their plates are made again before the
    /// open history step is closed (the next tick, which has the models).
    pending_rebuild: Vec<i64>,
    /// New options for a junction being made again, and roads to add to it as arms.
    pending_options: std::collections::HashMap<i64, crate::junction::Options>,
    pending_arms: std::collections::HashMap<i64, Vec<crate::junction::Arm>>,
    /// A road being drawn from or into a junction plate (its IDCode, centre, radius).
    road_plate_start: Option<(i64, DVec3, f64)>,
    road_plate_end: Option<(i64, DVec3, f64)>,
    /// Switches the track being drawn lays where it leaves or joins another track.
    switch_plans: Vec<crate::switches::Plan>,
    /// The turnouts the installation and the mods offer (filled when the library is read).
    pub switch_catalogue: Vec<crate::switches::Candidate>,
    /// A road end being dragged onto a plate.
    reshape_plate: Option<(i64, DVec3, f64)>,
    /// The options of junctions shown in the properties (read from their files).
    pub junction_options: std::collections::HashMap<i64, crate::junction::Options>,
    /// The first IDCode of this session: junction files numbered from it on were made now,
    /// and undo may still need them.
    session_first_id: i64,
    /// How objects are put along roads.
    pub row_params: crate::rows::RowParams,
    /// Numbers typed while a road is drawn: length, radius, heading, rise (m) at its end.
    pub typed_text: [String; 4],
    /// Shift held while drawing: a straight.
    pub straight_lock: bool,
    /// The Logic tool's AI paths are being edited (picked, reshaped, split) rather than drawn.
    pub path_edit: bool,
    /// Ctrl / Cmd is held (a click on a selected AI path puts a point into it).
    pub ctrl_held: bool,
    /// The road drawn is planned again this frame.
    pub replan: bool,
    /// The Roads tool builds roundabouts: a click on the centre, then on the ring (or a
    /// drag out to it).
    pub roundabout: bool,
    ring_centre: Option<DVec3>,
    /// Roads keep their height (bridges, embankments): an even gradient from end to end,
    /// the ground left as it is.
    pub keep_height: bool,
    /// The ruler, when on: its first point and, once clicked, its second.
    pub ruler: Option<(Option<DVec3>, Option<DVec3>)>,
    /// The knife (K): a click on a road cuts it there, until right-click or Esc.
    pub cutting: bool,
    // snapping
    pub snap_grid: bool,
    pub grid_step: f64,
    pub snap_angle: bool,
    /// The angle snapping's step, and what R turns by (degrees).
    pub angle_step: f64,
    /// The open history step is the properties panel's.
    panel_step: bool,
    pub cursor: Option<DVec3>,
    /// The cursor is over the 3D view (not over a panel).
    pub cursor_in_view: bool,
    // terrain
    pub terrain_mode: TerrainMode,
    pub brush_radius: f32,
    pub brush_strength: f32,
    flatten_to: Option<f32>,
    /// The water's level of the stroke going on (Lake / river), and how deep the bed is dug under it (m).
    water_level: Option<f32>,
    /// An AI path's pieces picked together for a move: the piece to pick alone again after it.
    /// The spline whose grip the pointer is on (with `hover_handle`), and the point of an AI path picked
    /// by a press on its grip (Del takes that point out, not the whole path).
    hover_handle_of: Option<(TileKey, i64)>,
    /// A press on a road's own lane while AI paths are edited: the road's pieces and the place, for
    /// the app to make the lanes editable (it has the folders) and pick the one clicked.
    pub lane_detach_request: Option<(Vec<(TileKey, i64)>, DVec2)>,
    path_point: Option<((TileKey, i64), Handle)>,
    pub water_depth: f32,
    /// Flatten to this height rather than to where the drag starts.
    pub flatten_fixed: Option<f32>,
    // paint
    pub paint_layer: usize,
    pub paint_erase: bool,
    // roads
    pub road_type: Option<String>,
    road_start: Option<(Anchor, bool)>,
    road_plan: Option<(Vec<Seg>, f64, f64, Option<Anchor>)>,
    pub follow_ground: bool,
    /// What is built follows the ground: new roads, paths and tracks take its height along their
    /// way (off: they run level from where they start). Only what is built from now on; what is
    /// there stays as it is.
    pub auto_terrain: bool,
    /// Where a new road meets or crosses another one, cut both and put a junction there.
    pub auto_junctions: bool,
    /// Where a track branches off another, put a switch there (off: the tracks just meet).
    pub auto_switches: bool,
    /// A road drawn near another's end or middle, or a junction piece's helper, joins it
    /// (off: roads are drawn exactly where clicked).
    pub snap_roads: bool,
    /// After a road is built the next one starts at its end (off: each road is drawn with
    /// two fresh clicks).
    pub keep_drawing: bool,
    /// How junctions treat each road type (corners, lights, crossings by themselves).
    pub road_styles: crate::road_maker::Styles,
    /// openOMSI's content folder (junction objects are written there) and the asphalt
    /// texture of their plates.
    pub content_dir: Option<std::path::PathBuf>,
    pub asphalt: Option<std::path::PathBuf>,
    pub road_snap: Option<Anchor>,
    road_snap_file: Option<String>,
    /// A road started from another one takes its type.
    pub inherit_type: bool,
    // objects
    pub object_type: Option<String>,
    pub place_heading: f64,
    pub align_to_road: bool,
    /// Aligned to a road, the object's front is put at the roadside, this far (m) from the
    /// road's edge (else it is only turned).
    pub align_front: bool,
    pub road_setback: f64,
    /// The road point an aligned object was set against (for the guide line).
    align_guide: Option<(DVec3, DVec3)>,
    /// Shift is held (places an object freely, not aligned).
    pub shift_held: bool,
    /// The Objects tool sows plants with a brush (or clears them).
    pub plant_brush: bool,
    pub plant_erase: bool,
    /// Plants no nearer to one another (m).
    pub plant_spacing: f64,
    /// What the brush sows, picked at random (empty: the catalogue's choice).
    pub plant_mix: Vec<String>,
    /// How the ground is painted along the selected roads (their properties).
    pub paint_along: crate::roadpaint::Along,
    /// Where a ramp of the Terrain tool starts (its first click).
    pub ramp_start: Option<DVec3>,
    /// The plant brush fills (or clears) an outline clicked round instead of painting.
    pub plant_area: bool,
    /// The outline's corners so far (world).
    pub plant_outline: Vec<DVec2>,
    /// The camera's distance at the last frame (how near a click must come to a corner).
    view_distance: std::cell::Cell<f64>,
    plant_rng: crate::plants::Rng,
    ghost_pos: Option<(DVec3, f64)>,
    /// A bus stop placed with "both sides": the one across the road (where, heading).
    stop_twin: Option<(DVec3, f64)>,
    /// A bus stop goes on both sides of the road at once.
    pub stop_both_sides: bool,
    // tiles
    hover_tile: Option<TileKey>,
    /// The tile clicked last with the Tiles tool (its overlay shown in the panel).
    pub picked_tile: Option<TileKey>,
    drag: Drag,
    pub toasts: Vec<Toast>,
    pub show_grid: bool,
    // game logic
    pub logic: LogicMode,
    /// The spline type AI paths are drawn with.
    pub path_type: Option<String>,
    /// The road type while the Logic tool draws AI paths with the road machinery.
    stashed_road: Option<Option<String>>,
    /// Car parks go in a row from one click to the next.
    pub parking_row: bool,
    /// Between car parks in a row (m).
    pub parking_step: f64,
    parking_start: Option<DVec3>,
    /// Things the game does not show are drawn in the view.
    pub show_technical: bool,
    /// Layers hidden in the view, and so not picked either: trees, other objects, splines.
    pub hide: [bool; 3],
    /// The AI's lanes are drawn as arrows (always with the Logic tool).
    pub show_lanes: bool,
    /// The names of bus stops and start points float over them in the view.
    pub show_names: bool,
    /// A window picks things on the map (the Lines window its stops): a click only notes
    /// what is under the cursor and where, nothing is selected or moved.
    pub pick_mode: bool,
    pub picked: Option<(Option<Sel>, Option<DVec3>)>,
    /// An object waiting for the one it takes its state from (`[varparent]`) to be clicked.
    pub link_pick: Option<(TileKey, i64)>,
    /// The paths of what is selected are numbered in the view (its path rules are open).
    pub path_numbers: Option<Sel>,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            doc: None,
            history: History::default(),
            tool: Tool::Select,
            selected: None,
            also: Vec::new(),
            hover: None,
            hover_handle: None,
            select_box: None,
            clip: None,
            move_from: Vec::new(),
            reshape: None,
            known_helpers: Default::default(),
            known_half: Default::default(),
            move_arms: Vec::new(),
            move_delta: DVec2::ZERO,
            xform: Xform::Select,
            gizmo_view: None,
            move_z_from: Vec::new(),
            move_rot_from: Vec::new(),
            move_world_from: Vec::new(),
            scale_preview: Vec::new(),
            scale_requests: Vec::new(),
            mesh: None,
            xform_value: 0.0,
            carry_preview: Vec::new(),
            shape_request: None,
            trace_lines: Vec::new(),
            look_request: None,
            level_request: false,
            last_click: None,
            last_mesh_click: None,
            pending_rebuild: Vec::new(),
            pending_options: Default::default(),
            pending_arms: Default::default(),
            road_plate_start: None,
            road_plate_end: None,
            switch_plans: Vec::new(),
            switch_catalogue: Vec::new(),
            reshape_plate: None,
            junction_options: Default::default(),
            session_first_id: i64::MAX,
            row_params: Default::default(),
            ruler: None,
            cutting: false,
            typed_text: Default::default(),
            straight_lock: false,
            path_edit: false,
            ctrl_held: false,
            replan: false,
            roundabout: false,
            ring_centre: None,
            keep_height: false,
            snap_grid: false,
            grid_step: 1.0,
            snap_angle: false,
            angle_step: 15.0,
            panel_step: false,
            cursor: None,
            cursor_in_view: false,
            terrain_mode: TerrainMode::Raise,
            brush_radius: 25.0,
            brush_strength: 0.5,
            flatten_to: None,
            water_level: None,
            hover_handle_of: None,
            lane_detach_request: None,
            path_point: None,
            water_depth: 1.5,
            flatten_fixed: None,
            paint_layer: 1,
            paint_erase: false,
            road_type: None,
            road_start: None,
            road_plan: None,
            follow_ground: true,
            auto_junctions: true,
            auto_terrain: true,
            auto_switches: true,
            snap_roads: true,
            keep_drawing: false,
            road_styles: Default::default(),
            content_dir: None,
            asphalt: None,
            road_snap: None,
            road_snap_file: None,
            inherit_type: true,
            object_type: None,
            place_heading: 0.0,
            align_to_road: true,
            align_front: true,
            road_setback: 2.0,
            align_guide: None,
            shift_held: false,
            plant_brush: false,
            plant_erase: false,
            plant_spacing: 6.0,
            plant_mix: Vec::new(),
            paint_along: Default::default(),
            ramp_start: None,
            plant_area: false,
            plant_outline: Vec::new(),
            view_distance: std::cell::Cell::new(100.0),
            plant_rng: crate::plants::Rng::new(0x5eed),
            ghost_pos: None,
            stop_twin: None,
            stop_both_sides: true,
            hover_tile: None,
            picked_tile: None,
            drag: Drag::None,
            toasts: Vec::new(),
            show_grid: true,
            logic: LogicMode::Start,
            path_type: None,
            stashed_road: None,
            parking_row: true,
            parking_step: 5.5,
            parking_start: None,
            show_technical: true,
            hide: [false; 3],
            show_lanes: false,
            show_names: true,
            pick_mode: false,
            picked: None,
            link_pick: None,
            path_numbers: None,
        }
    }
}

const ORANGE: [f32; 4] = [0.96, 0.50, 0.19, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 0.9];
const GREEN: [f32; 4] = [0.35, 0.85, 0.45, 1.0];
/// Links between objects (`[varparent]`).
const LINK: [f32; 4] = [0.3, 0.85, 0.95, 0.9];

impl Editor {
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast { text: text.into(), at: Instant::now(), error: false });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast { text: text.into(), at: Instant::now(), error: true });
    }

    pub fn set_tool(&mut self, t: Tool) {
        if self.tool != t {
            self.cancel();
            self.tool = t;
            self.sync_path_type();
            self.leave_selection_for_paths();
        }
    }

    /// The selected roads' own AI lanes made editable: each road gets the copy of its type
    /// without lanes, and each lane becomes an AI path spline of its own (see `lanes_edit`).
    pub fn detach_lanes(&mut self, assets: &mut Assets, roots: &crate::install::Roots) {
        let pieces: Vec<(TileKey, i64)> = self.selection().into_iter().filter_map(|s| if let Sel::Spline(k, id) = s { Some((k, id)) } else { None }).collect();
        let Some(doc) = self.doc.as_ref() else { return };
        let mut free = std::collections::HashMap::new();
        for (k, id) in &pieces {
            let Some(file) = doc.spline(*k, *id).map(|s| s.file.clone()) else { continue };
            if free.contains_key(&file.to_lowercase()) {
                continue;
            }
            let Some(m) = assets.spline(&file) else { continue };
            if m.def.paths.is_empty() || m.is_ai_path() || m.def.paths.iter().any(|p| p.kind == 2) {
                continue;
            }
            match crate::lanes_edit::lane_free_copy(roots, &file) {
                Ok(copy) => {
                    free.insert(file.to_lowercase(), copy);
                }
                Err(e) => {
                    self.error(format!("{e}"));
                    return;
                }
            }
        }
        if free.is_empty() {
            self.toast(tr("These roads have no lanes of their own to take apart (rails stay as they are)", "У этих дорог нет полос, которые можно разобрать (рельсы остаются как есть)"));
            return;
        }
        self.history.begin(tr("Make the lanes editable", "Полосы стали редактируемыми"));
        let doc = self.doc.as_mut().unwrap();
        let keys: Vec<TileKey> = doc.tiles.keys().copied().collect();
        for k in keys {
            self.history.touch_tile(doc, k);
        }
        self.history.touch_global(doc);
        let n = crate::lanes_edit::detach(doc, assets, &pieces, &free, "Splines\\Marcel\\invis_street.sli", "Splines\\invis_sidewalk.sli");
        self.history.commit();
        self.show_lanes = true;
        self.toast(format!("{} {n}", tr("Roads whose lanes are now editable:", "Дорог с редактируемыми полосами:")));
    }

    /// The whole map's relief taken away: every tile's ground levelled to the map's mean height
    /// (the picture of a satellite map stays; a map from OpenStreetMap needs none), the roads
    /// and the AI paths laid on it again and the junction plates brought to it; one undo step.
    /// How many roads were laid.
    pub fn flatten_terrain(&mut self) -> usize {
        let Some(doc) = self.doc.as_ref() else { return 0 };
        let (sum, n) = doc.tiles.values().flat_map(|t| t.terrain.heights.iter()).fold((0.0f64, 0usize), |(s, n), h| (s + *h as f64, n + 1));
        if n == 0 {
            return 0;
        }
        let level = (sum / n as f64) as f32;
        let all: Vec<Sel> = doc.tiles.iter().flat_map(|(k, t)| t.tile.splines.iter().filter(|s| !s.deleted).map(move |s| Sel::Spline(*k, s.id))).collect();
        self.history.begin(tr("Flatten the terrain", "Выровнять рельеф"));
        let keys: Vec<TileKey> = doc.tiles.keys().copied().collect();
        for k in &keys {
            self.history.touch_tile(doc, *k);
        }
        self.history.touch_global(doc);
        let Some(doc) = self.doc.as_mut() else { return 0 };
        for t in doc.tiles.values_mut() {
            t.terrain.heights.iter_mut().for_each(|h| *h = level);
            t.touch_terrain();
        }
        // (the junction plates stand at the ground's height now)
        for t in doc.tiles.values_mut() {
            let mut touched = false;
            for o in t.tile.objects.iter_mut().filter(|o| is_own_junction(&o.file)) {
                o.pos[2] = level as f64;
                touched = true;
            }
            if touched {
                t.touch_content();
            }
        }
        crate::onground::lay(doc, &all);
        self.history.commit();
        all.len()
    }

    /// The AI paths are worked on: what was selected (a building, say) is let go, so the
    /// properties and the dots are the paths'.
    fn leave_selection_for_paths(&mut self) {
        if self.tool == Tool::Logic && self.logic == LogicMode::Paths {
            self.select(None);
        }
    }

    pub fn set_logic(&mut self, m: LogicMode) {
        if self.logic != m {
            self.cancel();
            self.logic = m;
            self.leave_selection_for_paths();
            self.object_type = None;
            self.ghost_pos = None;
            self.sync_path_type();
        }
    }

    /// AI paths are drawn as roads are: while they are, the road machinery's type is the
    /// path's, and the road type waits.
    fn sync_path_type(&mut self) {
        let paths = self.tool == Tool::Logic && self.logic == LogicMode::Paths;
        match (paths, self.stashed_road.is_some()) {
            (true, false) => self.stashed_road = Some(std::mem::replace(&mut self.road_type, self.path_type.clone())),
            (false, true) => self.road_type = self.stashed_road.take().unwrap_or(None),
            _ => {}
        }
    }

    /// Choose the AI path type (while drawing paths, the road machinery takes it at once).
    pub fn set_path_type(&mut self, rel: String) {
        self.path_type = Some(rel.clone());
        if self.stashed_road.is_some() {
            self.road_type = Some(rel);
        }
    }

    /// Drawing AI paths with the Logic tool.
    pub fn drawing_paths(&self) -> bool {
        self.tool == Tool::Logic && self.logic == LogicMode::Paths && !self.path_edit
    }

    /// AI paths are edited: only they can be picked.
    pub fn editing_paths(&self) -> bool {
        self.tool == Tool::Logic && self.logic == LogicMode::Paths && self.path_edit
    }

    /// The tool the mouse works as: the Logic tool draws paths as roads are drawn and
    /// places everything else as objects are placed.
    fn acts_as(&self) -> Tool {
        match self.tool {
            Tool::Logic if self.logic == LogicMode::Paths && self.path_edit => Tool::Select,
            Tool::Logic if self.logic == LogicMode::Paths => Tool::Roads,
            Tool::Logic => Tool::Objects,
            t => t,
        }
    }

    /// Stop what the tool is in the middle of.
    pub fn cancel(&mut self) {
        self.road_start = None;
        self.road_plan = None;
        self.parking_start = None;
        self.drag = Drag::None;
        self.reshape = None;
        self.select_box = None;
        self.ring_centre = None;
        self.plant_outline.clear();
        self.ramp_start = None;
        self.link_pick = None;
        self.cutting = false;
        if let Some(doc) = &mut self.doc {
            self.history.commit();
            let _ = doc;
        }
    }

    /// Undo the last step. What the tool was in the middle of stops first: it may stand on
    /// what the undo takes away (a road drawn on from the end of the one undone).
    pub fn undo(&mut self) {
        if let Some(m) = self.mesh.as_mut().filter(|m| m.editing) {
            m.undo();
            return;
        }
        self.cancel();
        let groups = self.groups().to_vec();
        if let Some(d) = self.doc.as_mut() {
            self.history.undo(d);
        }
        if self.groups() != groups.as_slice() {
            self.save_groups();
        }
        self.select(None);
    }

    pub fn redo(&mut self) {
        if let Some(m) = self.mesh.as_mut().filter(|m| m.editing) {
            m.redo();
            return;
        }
        self.cancel();
        let groups = self.groups().to_vec();
        if let Some(d) = self.doc.as_mut() {
            self.history.redo(d);
        }
        if self.groups() != groups.as_slice() {
            self.save_groups();
        }
        self.select(None);
    }

    pub fn open(&mut self, doc: Document) {
        self.history.clear();
        self.select(None);
        self.hover = None;
        self.cancel();
        // (what was kept by IDCode belongs to the map that was open)
        self.junction_options.clear();
        self.pending_rebuild.clear();
        self.pending_options.clear();
        self.pending_arms.clear();
        self.paint_layer = if doc.global.ground_textures.len() > 1 { 1 } else { 0 };
        self.session_first_id = doc.global.next_id_code;
        let mut doc = doc;
        doc.groups = crate::mapgroups::load(&doc.dir);
        self.doc = Some(doc);
    }

    /// The map's groups (things a click selects and a drag moves as one).
    pub fn groups(&self) -> &[crate::mapgroups::MapGroup] {
        self.doc.as_ref().map(|d| d.groups.as_slice()).unwrap_or(&[])
    }

    /// A selection grown to the whole of the groups it touches.
    fn with_groups(&self, v: Vec<Sel>) -> Vec<Sel> {
        match self.doc.as_ref() {
            Some(doc) if !doc.groups.is_empty() => crate::mapgroups::expand(doc, &doc.groups, &v),
            _ => v,
        }
    }

    /// The groups changed (one undo step), and written beside the map.
    fn change_groups(&mut self, label: &str, f: impl FnOnce(&mut Vec<crate::mapgroups::MapGroup>)) {
        let Some(doc) = self.doc.as_mut() else { return };
        let before = doc.groups.clone();
        f(&mut doc.groups);
        if doc.groups != before {
            self.history.push_snapshot(label, crate::history::Snapshot { groups: Some(before), ..Default::default() });
            self.save_groups();
        }
    }

    /// The group (its place among the map's) the selection is all of, if it is one.
    pub fn selected_group(&self) -> Option<usize> {
        let sels = self.selection();
        let first = sels.first()?;
        let doc = self.doc.as_ref()?;
        let g = crate::mapgroups::group_of(&doc.groups, crate::mapgroups::id_of(*first))?;
        let all = crate::mapgroups::expand(doc, &doc.groups, &[*first]);
        (all.len() == sels.len() && all.iter().all(|s| sels.contains(s))).then_some(g)
    }

    /// Whether anything selected belongs to a group.
    pub fn selection_grouped(&self) -> bool {
        self.selection().iter().any(|s| crate::mapgroups::group_of(self.groups(), crate::mapgroups::id_of(*s)).is_some())
    }

    /// The selection made a group of its own (taken out of any it was in).
    pub fn group_selection(&mut self) {
        let ids: Vec<i64> = self.selection().iter().map(|s| crate::mapgroups::id_of(*s)).collect();
        if ids.len() < 2 {
            self.toast(tr("Select two things or more to group them", "Выделите два объекта или больше, чтобы сгруппировать"));
            return;
        }
        let name = crate::mapgroups::free_name(self.groups(), tr("Group", "Группа"));
        let n2 = name.clone();
        self.change_groups(tr("Group", "Сгруппировать"), move |groups| {
            for g in groups.iter_mut() {
                g.ids.retain(|id| !ids.contains(id));
            }
            groups.retain(|g| g.ids.len() >= 2);
            groups.push(crate::mapgroups::MapGroup { name: n2, ids });
        });
        self.toast(format!("{} «{name}»", tr("Grouped:", "Сгруппировано:")));
    }

    /// The groups the selection touches undone (their things stay where they are).
    pub fn ungroup_selection(&mut self) {
        let ids: Vec<i64> = self.selection().iter().map(|s| crate::mapgroups::id_of(*s)).collect();
        let before = self.groups().len();
        self.change_groups(tr("Ungroup", "Разгруппировать"), |groups| groups.retain(|g| !g.ids.iter().any(|id| ids.contains(id))));
        if self.groups().len() != before {
            self.toast(tr("Ungrouped", "Разгруппировано"));
        }
    }

    pub fn rename_group(&mut self, g: usize, name: &str) {
        let name = name.to_string();
        self.change_groups(tr("Rename group", "Имя группы"), move |groups| {
            if let Some(x) = groups.get_mut(g) {
                x.name = name;
            }
        });
    }

    /// Select every thing of a group.
    pub fn select_group(&mut self, g: usize) {
        let Some(doc) = self.doc.as_ref() else { return };
        let v: Vec<Sel> = doc.groups.get(g).map(|x| x.ids.iter().filter_map(|id| crate::mapgroups::sel_of(doc, *id)).collect()).unwrap_or_default();
        self.set_selection(v);
    }

    /// The groups written beside the map at once (they are not part of OMSI 2's files).
    fn save_groups(&mut self) {
        let Some(doc) = self.doc.as_ref() else { return };
        if let Err(e) = crate::mapgroups::save(&doc.dir, &doc.groups) {
            self.error(format!("groups.json: {e}"));
        }
    }

    // ---------------------------------------------------------------- picking

    /// Where a ray meets the ground (outside the map: the plane at height 0).
    pub fn ground_hit(&self, o: DVec3, d: DVec3) -> Option<DVec3> {
        let doc = self.doc.as_ref()?;
        let mut t = 0.0;
        let max = 20000.0;
        let mut prev = o;
        let above = |p: DVec3| -> Option<bool> { doc.height_at(p.truncate()).map(|h| p.z > h as f64) };
        let mut step = 2.0f64;
        while t < max {
            let p = o + d * t;
            match above(p) {
                Some(false) => {
                    // refine between prev and p
                    let (mut a, mut b) = (prev, p);
                    for _ in 0..24 {
                        let m = (a + b) * 0.5;
                        if above(m).unwrap_or(true) {
                            a = m;
                        } else {
                            b = m;
                        }
                    }
                    let q = (a + b) * 0.5;
                    return Some(DVec3::new(q.x, q.y, doc.ground(q.truncate())));
                }
                _ => {}
            }
            prev = p;
            step = (t * 0.01).clamp(1.0, 25.0);
            t += step;
        }
        let _ = step;
        // the plane at 0
        if d.z < -1e-6 {
            let t = -o.z / d.z;
            return Some(o + d * t);
        }
        None
    }

    /// The object or spline under a ray.
    pub fn pick(&self, assets: &mut Assets, o: DVec3, d: DVec3, ground: Option<DVec3>) -> Option<Sel> {
        let doc = self.doc.as_ref()?;
        let mut best: Option<(f64, Sel)> = None;
        // (relative to the ray's origin: f32 world positions far from the map's origin are
        // half a metre coarse)
        let ov = glam::Vec3::ZERO;
        let dv = d.as_vec3();
        let gdist = ground.map(|g| (g - o).length()).unwrap_or(f64::MAX);
        let paths_only = self.editing_paths();
        // (what is not drawn is not picked)
        let extras = FrameExtras { show_technical: self.show_technical || self.tool == Tool::Logic, hide: self.hide, ..Default::default() };
        let unseen = |m: &crate::assets::ObjectModel| m.technical && !extras.show_technical || crate::render::hidden_layer(m, &extras);
        for (k, t) in doc.tiles.iter().filter(|_| !paths_only) {
            for ob in &t.tile.objects {
                let Some(model) = assets.object(&ob.file) else { continue };
                if unseen(&model) {
                    continue;
                }
                let world = doc.object_world(*k, ob, model.abs_height);
                let m = object_matrix(world - o, ob.rot);
                let c = m.transform_point3(model.centre());
                let r = model.radius().max(1.0);
                if !omsi_geometry::ray_near_sphere(ov, dv, c, r) {
                    continue;
                }
                let hit = if model.parts.is_empty() {
                    // a tree or a marker: its sphere
                    let oc = c.as_dvec3();
                    let tca = oc.dot(d);
                    (tca > 0.0).then_some(tca)
                } else {
                    model.parts.iter().filter_map(|p| omsi_geometry::ray_mesh(ov, dv, &p.mesh, &m)).map(|t| t as f64).reduce(f64::min)
                };
                if let Some(t) = hit {
                    if t < gdist + 2.0 && best.map(|b| t < b.0).unwrap_or(true) {
                        best = Some((t, Sel::Object(*k, ob.id)));
                    }
                }
            }
        }
        // what hangs on objects and rows (signs on posts, lamps on masts), placed as they are drawn
        // (only near where the ray meets the ground: rows are worked out along their roads)
        let near = |k: &TileKey| ground.map(|g| (tile_origin(*k) + DVec2::splat(tile_size() * 0.5) - g.truncate()).length() < tile_size() * 1.5).unwrap_or(false);
        for (k, t) in doc.tiles.iter().filter(|_| !paths_only) {
            if t.tile.attach_objects.is_empty() || !near(k) {
                continue;
            }
            let mut parents: std::collections::HashMap<i64, (crate::place::Pose, std::sync::Arc<crate::assets::ObjectModel>)> = std::collections::HashMap::new();
            for ob in &t.tile.objects {
                let Some(model) = assets.object(&ob.file) else { continue };
                parents.insert(ob.id, (crate::place::Pose::of_object(doc.object_world(*k, ob, model.abs_height), ob.rot), model));
            }
            for a in t.tile.spline_attachments.iter().filter(|a| a.repeater.is_none()) {
                let Some(model) = assets.object(&a.file) else { continue };
                let objs = crate::place::row_objects(a, &t.tile.splines, tile_origin(*k));
                if let Some(first) = objs.iter().min_by_key(|o| o.index) {
                    parents.entry(a.id).or_insert((first.pose, model));
                }
            }
            for ob in &t.tile.attach_objects {
                let Some(model) = assets.object(&ob.file) else { continue };
                let Some((parent, pmodel)) = ob.parent_id.and_then(|p| parents.get(&p)).cloned() else { continue };
                let attach = pmodel.attachments.get(ob.attach_index).copied().unwrap_or(glam::Mat4::IDENTITY);
                let pose = parent.attached(&attach, omsi_geometry::map_rotation(ob.rot));
                parents.insert(ob.id, (pose, model.clone()));
                if unseen(&model) {
                    continue;
                }
                let m = pose.matrix_from(o);
                let c = m.transform_point3(model.centre());
                if !omsi_geometry::ray_near_sphere(ov, dv, c, model.radius().max(0.5)) {
                    continue;
                }
                let hit = if model.parts.is_empty() {
                    let tca = c.as_dvec3().dot(d);
                    (tca > 0.0).then_some(tca)
                } else {
                    model.parts.iter().filter_map(|p| omsi_geometry::ray_mesh(ov, dv, &p.mesh, &m)).map(|t| t as f64).reduce(f64::min)
                };
                if let Some(t) = hit {
                    if t < gdist + 2.0 && best.map(|b| t < b.0).unwrap_or(true) {
                        best = Some((t, Sel::Object(*k, ob.id)));
                    }
                }
            }
        }
        if best.is_some() {
            return best.map(|b| b.1);
        }
        // splines: the nearest centre line within its width from the ground point
        let g = ground?;
        let mut sbest: Option<(f64, Sel)> = None;
        for (k, t) in doc.tiles.iter().filter(|_| !self.hide[2] || paths_only) {
            let org = tile_origin(*k);
            for s in t.tile.splines.iter().filter(|s| !s.deleted) {
                let start = DVec2::new(org.x + s.pos[0], org.y + s.pos[1]);
                if (start - g.truncate()).length() > s.length + 30.0 {
                    continue;
                }
                let model = assets.spline(&s.file);
                let ai = model.as_ref().map(|m| m.is_ai_path()).unwrap_or(false);
                if paths_only && !ai {
                    continue;
                }
                // (an AI path is seen only as lines, with the lanes shown: otherwise not picked)
                if ai && !paths_only && !(self.show_lanes || self.show_technical) {
                    continue;
                }
                // (an AI path has no width of its own: a metre and a half either side)
                let hw = if ai { 1.5 } else { model.map(|m| m.half_width as f64).unwrap_or(3.0).max(1.5) };
                let c = omsi_geometry::SplineCurve::from_map(s, org);
                let n = (s.length / 1.0).ceil().max(2.0) as usize;
                for i in 0..=n {
                    let p = c.point_at(s.length * i as f64 / n as f64);
                    let dd = (p.truncate() - g.truncate()).length();
                    if dd < hw && sbest.map(|b| dd < b.0).unwrap_or(true) {
                        sbest = Some((dd, Sel::Spline(*k, s.id)));
                    }
                }
            }
        }
        sbest.map(|b| b.1)
    }

    // ---------------------------------------------------------------- per frame

    /// Mouse moved or the view changed: update the cursor and what the tool shows.
    pub fn hover_ray(&mut self, assets: &mut Assets, _lib: &Library, o: DVec3, d: DVec3) {
        if self.doc.is_none() {
            return;
        }
        let g = self.ground_hit(o, d);
        self.cursor = g;
        self.hover = None;
        self.hover_tile = None;
        match self.acts_as() {
            Tool::Select => {
                if self.drag == Drag::None {
                    let found = self.handle_at_ray(assets, o, d);
                    self.hover_handle = found.map(|f| f.1);
                    self.hover_handle_of = found.map(|f| f.0);
                    self.hover = if self.hover_handle.is_some() { self.selected } else { self.pick(assets, o, d, g) };
                }
            }
            Tool::Tiles => {
                self.hover_tile = g.map(|p| tile_of(p.truncate()));
            }
            Tool::Roads => self.plan_road(assets),
            // (Game logic: a stop, start point or other helper already there is picked to
            // select and move, not put again on top of it)
            Tool::Objects if self.tool == Tool::Logic && self.drag == Drag::None => {
                self.hover = self.pick(assets, o, d, g).filter(|s| self.is_logic_object(assets, *s));
                if self.hover.is_some() {
                    self.ghost_pos = None;
                    self.stop_twin = None;
                } else {
                    self.plan_object(assets, _lib);
                }
            }
            Tool::Objects => self.plan_object(assets, _lib),
            _ => {}
        }
    }

    /// What the Game logic tool picks rather than places over: a bus stop, a start point or
    /// any helper the game does not show (car parks, sound sources, …).
    fn is_logic_object(&self, assets: &mut Assets, s: Sel) -> bool {
        let Sel::Object(k, id) = s else { return false };
        let Some(o) = self.doc.as_ref().and_then(|d| selection::any_object(d, k, id)) else { return false };
        let file = o.file.clone();
        o.flag == 7 || self.doc.as_ref().map(|d| d.global.entry_points.iter().any(|e| e.object_id == id)).unwrap_or(false) || assets.object(&file).map(|m| m.technical).unwrap_or(false)
    }

    /// The cursor at a ground point (scripted use: the demo and tests).
    pub fn hover_point(&mut self, assets: &mut Assets, _lib: &Library, p: DVec3) {
        self.cursor = Some(p);
        self.cursor_in_view = true;
        match self.acts_as() {
            Tool::Roads => self.plan_road(assets),
            Tool::Objects => self.plan_object(assets, _lib),
            Tool::Tiles => self.hover_tile = Some(tile_of(p.truncate())),
            _ => {}
        }
    }

    /// Continuous work while a button is held (brushes, dragging).
    pub fn tick(&mut self, assets: &mut Assets, dt: f64, held: bool) {
        self.learn(assets);
        if let Some((k, id, length, radius)) = self.shape_request.take() {
            self.set_spline_shape(assets, k, id, length, radius);
        }
        // (once the objects' models are there: their outlines are needed)
        if self.level_request {
            let files: Vec<String> = self.doc.as_ref().map(|d| self.selection().iter().filter_map(|s| if let Sel::Object(k, id) = s { d.object(*k, *id).map(|o| o.file.clone()) } else { None }).collect()).unwrap_or_default();
            if files.iter().all(|f| assets.object(f).is_some() || assets.object_failed(f)) {
                self.level_request = false;
                let n = self.level_under_objects(assets);
                self.toast(format!("{} {n}", tr("Ground levelled under objects:", "Земля выровнена под объектами:")));
            }
        }
        // (an undo in between closed the step: the plates are as they were)
        if !self.pending_rebuild.is_empty() && self.drag == Drag::None {
            if self.history.is_open() {
                self.rebuild_pending(assets);
                self.history.commit();
            }
            self.pending_rebuild.clear();
            self.pending_options.clear();
            self.pending_arms.clear();
        }
        // (numbers typed or Shift changed: the road drawn follows at once)
        if self.replan && self.acts_as() == Tool::Roads {
            self.plan_road(assets);
        }
        self.replan = false;
        if !held {
            match std::mem::replace(&mut self.drag, Drag::None) {
                Drag::None => {}
                Drag::Move { started, .. } => {
                    if started {
                        let sels = self.selection();
                        if let Some(doc) = self.doc.as_mut() {
                            selection::unlink_outside(doc, &mut self.history, &sels);
                        }
                        let arms = std::mem::take(&mut self.move_arms);
                        self.carry(&arms, DVec2::ZERO, 0.0, self.move_delta, 0.0);
                        self.ground_to_selection();
                        self.rebuild_pending(assets);
                    }
                    self.carry_preview.clear();
                    self.history.commit();
                }
                Drag::MeshGizmo { started, .. } => {
                    if let (true, Some(m)) = (started, self.mesh.as_mut()) {
                        m.end_drag();
                    }
                }
                Drag::Gizmo { grab, started } => {
                    if started {
                        self.finish_gizmo(assets, grab);
                    }
                    self.carry_preview.clear();
                    // (a scale is finished by the app, which makes the scaled files: in the same step)
                    if self.scale_requests.is_empty() {
                        self.history.commit();
                    }
                }
                Drag::Reshape(k, id, _) => self.finish_reshape(k, id),
                Drag::Brush => self.history.commit(),
            }
            return;
        }
        let Some(c) = self.cursor else { return };
        match self.drag {
            Drag::Brush => match self.tool {
                Tool::Terrain => self.brush_terrain(assets, c.truncate(), dt),
                Tool::Paint => self.brush_paint(c.truncate(), dt),
                Tool::Objects => self.brush_plants(assets, c.truncate(), dt),
                _ => {}
            },
            Drag::Move { grab, started } => {
                let raw = c.truncate() - grab;
                if !started {
                    if raw.length() < MOVE_THRESHOLD || self.move_from.is_empty() {
                        return;
                    }
                    self.history.begin(tr("Move", "Перемещение"));
                    self.drag = Drag::Move { grab, started: true };
                    self.move_arms = self.arms_for(&self.selection());
                }
                // (the grid holds the first one's place; the rest keep their distances)
                let first = self.move_from[0].pos;
                let delta = self.snap_point(first + raw) - first;
                let Some(doc) = self.doc.as_mut() else { return };
                let now = selection::transform(doc, &mut self.history, &self.move_from, DVec2::ZERO, 0.0, delta);
                // (the roads a junction holds follow when it is let go; meanwhile their new shape
                // is shown)
                let all: Vec<selection::Arm> = self.move_arms.iter().flat_map(|g| g.1.iter().copied()).collect();
                self.carry_preview = selection::carry_plans(doc, &all, DVec2::ZERO, 0.0, delta, 0.0, &|_| false).into_iter().flatten().collect();
                self.move_delta = delta;
                self.set_selection(now);
            }
            Drag::Gizmo { grab, started } => self.drag_gizmo(grab, started),
            Drag::MeshGizmo { grab, started } => {
                let (Some(view), Some(m)) = (self.gizmo_view.clone(), self.mesh.as_mut()) else { return };
                if !started {
                    if !grab.moved(&view) {
                        return;
                    }
                    m.begin_drag();
                    self.drag = Drag::MeshGizmo { grab, started: true };
                }
                let change = grab.change(&view, self.snap_angle.then_some(self.angle_step));
                m.drag(change, grab.gizmo.pivot);
                self.xform_value = match change {
                    crate::gizmo::Change::Move(d) => d.length(),
                    crate::gizmo::Change::Turn(_, deg) => deg,
                    crate::gizmo::Change::Scale(f) => f.max_element().max(f.min_element()),
                };
            }
            Drag::Reshape(k, id, h) => {
                let mut p = self.snap_point(c.truncate());
                // an end dropped on a junction plate goes to its centre and becomes an arm
                let ai = self.editing_paths();
                self.reshape_plate = if h == Handle::Mid || ai { None } else { self.plate_at(assets, c.truncate()) };
                if let Some((_, centre, _)) = self.reshape_plate {
                    p = centre.truncate();
                }
                // (an AI path's end dragged to the end of a lane: it goes there, tangent to it, to join it)
                let kind = self.doc.as_ref().and_then(|d| d.spline(k, id)).and_then(|s| assets.spline(&s.file)).and_then(|m| m.def.paths.first().map(|q| q.kind));
                let lane = match (ai, h, kind) {
                    (true, Handle::Start | Handle::End, Some(kind)) => self.snap_lane_kind(assets, c.truncate(), h == Handle::Start, kind).filter(|(a, _)| a.mid.is_none()).map(|(a, _)| a),
                    _ => None,
                };
                if let Some(a) = &lane {
                    p = a.pos.truncate();
                }
                let joined = self.helper_anchors(assets);
                let locked = |q: DVec2| joined.iter().any(|j| (j.truncate() - q).length() < 0.6);
                let Some(doc) = self.doc.as_ref() else { return };
                let ground = |q: DVec2| doc.ground(q);
                self.reshape = Some(match h {
                    Handle::Start | Handle::End => match &lane {
                        Some(a) => reshape::drag_end_aligned(doc, k, id, h == Handle::End, p, roads::norm_deg(a.heading + 180.0), &ground, &|_| false).or_else(|_| reshape::drag_end(doc, k, id, h == Handle::End, p, &ground, &locked)),
                        None => reshape::drag_end(doc, k, id, h == Handle::End, p, &ground, &locked),
                    },
                    // (an AI path bent in the middle keeps the way it leaves and meets the lanes it is joined to)
                    Handle::Mid if ai => reshape::drag_mid(doc, k, id, p, &|_| true).or_else(|_| reshape::drag_mid(doc, k, id, p, &locked)),
                    Handle::Mid => reshape::drag_mid(doc, k, id, p, &locked),
                });
            }
            Drag::None => {}
        }
    }

    // ---------------------------------------------------------------- clicks

    pub fn left_press(&mut self, assets: &mut Assets, lib: &Library, shift: bool) {
        if self.doc.is_none() {
            return;
        }
        if let (Some((k, id)), Tool::Select) = (self.link_pick, self.tool) {
            self.link_pick = None;
            match self.hover {
                Some(Sel::Object(_, p)) if p != id => self.link_to(k, id, p),
                _ => self.toast(tr("No object clicked: nothing linked", "Объект не выбран: ничего не связано")),
            }
            return;
        }
        if self.pick_mode && self.tool == Tool::Select {
            self.picked = Some((self.hover, self.cursor));
            return;
        }
        // the knife takes the clicks while it is on: each cuts the road under the cursor
        if self.cutting {
            match self.cut_preview() {
                Some(c) if c.fits => self.cut_spline(c.key, c.id, c.at),
                Some(_) => self.error(tr("Too near the road's end to cut it there", "Слишком близко к концу дороги, чтобы разрезать её здесь")),
                None => self.toast(tr("No road under the cursor", "Под курсором нет дороги")),
            }
            return;
        }
        // the ruler takes the clicks while it is on: from one point to the next
        if let (Some(r), Some(c)) = (&mut self.ruler, self.cursor) {
            *r = match *r {
                (Some(a), None) => (Some(a), Some(c)),
                _ => (Some(c), None),
            };
            return;
        }
        // (editing AI paths: a press on a road's own lane makes the road's lanes editable and picks it)
        if self.editing_paths() && self.hover.is_none() && self.hover_handle.is_none() {
            if let Some(req) = self.road_lane_under_cursor(assets) {
                self.lane_detach_request = Some(req);
                return;
            }
        }
        // (Game logic: a press on one already there selects it, and a drag moves it)
        if self.tool == Tool::Logic && self.acts_as() == Tool::Objects && self.hover.is_some() {
            self.select_press(shift);
            return;
        }
        match self.acts_as() {
            Tool::Select => self.select_press(shift),
            Tool::Terrain if self.tool == Tool::Terrain && self.terrain_mode == TerrainMode::Ramp => {
                let Some(c) = self.cursor else { return };
                match self.ramp_start.take() {
                    None => self.ramp_start = Some(c),
                    Some(a) => {
                        let grade = self.ramp(a, c);
                        self.toast(format!("{} {:+.1} %", tr("Ramp:", "Пандус:"), grade));
                    }
                }
            }
            Tool::Terrain | Tool::Paint => {
                self.history.begin(if self.tool == Tool::Terrain { tr("Shape terrain", "Рельеф") } else { tr("Paint ground", "Покраска") });
                self.flatten_to = self.flatten_fixed.or(self.cursor.map(|c| c.z as f32));
                // (a lake's surface: the tile's water if it has some, else a little under where the stroke starts)
                self.water_level = self.cursor.map(|c| {
                    let tile = self.doc.as_ref().and_then(|d| d.tiles.get(&tile_of(c.truncate())).and_then(|t| t.water));
                    tile.map(|w| w.iter().sum::<f32>() / 4.0).unwrap_or(c.z as f32 - 0.3)
                });
                self.drag = Drag::Brush;
            }
            Tool::Roads if self.roundabout && !self.drawing_paths() => self.ring_click(assets),
            Tool::Roads => self.road_click(assets, lib),
            Tool::Objects if self.tool == Tool::Logic && self.logic == LogicMode::Parking && self.parking_row => self.parking_click(assets, lib),
            Tool::Objects if self.tool == Tool::Objects && self.plant_brush && self.plant_area => {
                let Some(c) = self.cursor else { return };
                // (a click on the first corner closes the outline)
                let close = self.plant_outline.len() >= 3 && (self.plant_outline[0] - c.truncate()).length() < self.close_radius();
                if close {
                    self.finish_plant_outline(assets);
                } else {
                    self.plant_outline.push(c.truncate());
                }
            }
            Tool::Objects if self.tool == Tool::Objects && self.plant_brush => {
                self.history.begin(if self.plant_erase { tr("Clear plants", "Убрать растения") } else { tr("Plant", "Посадка") });
                self.drag = Drag::Brush;
            }
            Tool::Objects => self.place_object(lib, shift),
            Tool::Tiles => self.tile_click(),
            Tool::Logic => {}
        }
    }

    /// The left button let go; `dragged`: the mouse moved since it went down (a road is
    /// built by dragging from its start to its end, as well as by two clicks).
    pub fn left_release(&mut self, assets: &mut Assets, lib: &Library, dragged: bool) {
        if self.acts_as() == Tool::Roads && self.roundabout && dragged && self.ring_centre.is_some() {
            self.ring_click(assets);
            return;
        }
        if self.acts_as() == Tool::Roads && dragged && self.road_start.is_some() && self.road_plan.is_some() {
            self.road_click(assets, lib);
        }
    }

    pub fn road_start_active(&self) -> bool {
        self.road_start.is_some()
    }

    /// A right click without a drag: what the tool was doing is called off; true when the
    /// view's menu is to open instead.
    pub fn right_click(&mut self) -> bool {
        if let Some(m) = self.mesh.as_mut().filter(|m| m.modal_active()) {
            m.cancel();
            return false;
        }
        if self.cutting {
            self.cutting = false;
            return false;
        }
        if self.ruler.is_some() {
            self.ruler = None;
            return false;
        }
        match self.acts_as() {
            Tool::Roads if self.ring_centre.is_some() => {
                self.ring_centre = None;
            }
            Tool::Roads if self.road_start.is_some() => {
                self.road_start = None;
                self.road_plan = None;
            }
            Tool::Terrain if self.ramp_start.is_some() => {
                self.ramp_start = None;
            }
            Tool::Objects if !self.plant_outline.is_empty() => {
                self.plant_outline.pop();
            }
            Tool::Objects if self.parking_start.is_some() => {
                self.parking_start = None;
            }
            Tool::Objects if self.object_type.is_some() => {
                self.object_type = None;
                self.ghost_pos = None;
            }
            Tool::Tiles => {
                if let Some(k) = self.hover_tile {
                    self.remove_tile(k);
                }
            }
            // (Select, and Game logic on a thing of its own: what is under the cursor is
            // selected, its group with it, and the view's menu opens)
            Tool::Select => {
                if let Some(h) = self.hover.filter(|h| !self.is_selected(*h)) {
                    let v = if self.ctrl_held { vec![h] } else { self.with_groups(vec![h]) };
                    self.set_selection(v);
                }
                return true;
            }
            Tool::Objects if self.tool == Tool::Logic && self.hover.is_some() => {
                if let Some(h) = self.hover.filter(|h| !self.is_selected(*h)) {
                    let v = self.with_groups(vec![h]);
                    self.set_selection(v);
                }
                return true;
            }
            _ => {
                self.select(None);
            }
        }
        false
    }

    /// Turn what is placed or selected (degrees): a selection turns round its middle.
    pub fn rotate(&mut self, deg: f64) {
        match self.acts_as() {
            Tool::Objects => self.place_heading = self.snap_heading(roads::norm_deg(self.place_heading + deg)),
            _ => {
                let sels = self.selection();
                let arms = self.arms_for(&sels);
                let Some(doc) = self.doc.as_mut() else { return };
                let Some(pivot) = selection::centre(doc, &sels) else { return };
                let from: Vec<Origin> = sels.iter().filter_map(|s| selection::origin_of(doc, *s)).collect();
                // (one thing alone, with angle snapping on, comes to a whole step)
                let deg = match (&from[..], self.snap_angle) {
                    ([one], true) => snap_deg(one.heading + deg, self.angle_step) - one.heading,
                    _ => deg,
                };
                self.history.begin(if sels.len() > 1 { tr("Rotate selection", "Поворот выделенного") } else { tr("Rotate", "Поворот") });
                let now = selection::transform(doc, &mut self.history, &from, pivot, deg, DVec2::ZERO);
                selection::unlink_outside(doc, &mut self.history, &now);
                self.set_selection(now);
                self.carry(&arms, pivot, deg, DVec2::ZERO, 0.0);
                self.ground_to_selection();
                self.finish_step();
            }
        }
    }

    pub fn delete_selected(&mut self) {
        // (an AI path's point picked: that point only)
        if self.editing_paths() && self.path_point.is_some() {
            self.remove_path_point();
            return;
        }
        let sels = self.selection();
        if self.doc.is_none() || sels.is_empty() {
            return;
        }
        self.history.begin(match sels[..] {
            [Sel::Object(..)] => tr("Delete object", "Удаление объекта"),
            [Sel::Spline(..)] => tr("Delete road", "Удаление дороги"),
            _ => tr("Delete selection", "Удаление выделенного"),
        });
        // a junction's signal heads go with it
        let mut sels = sels;
        if let Some(doc) = self.doc.as_ref() {
            let kids: Vec<Sel> = sels.iter().filter_map(|s| if let Sel::Object(_, id) = s { Some(*id) } else { None }).flat_map(|id| crate::junction::children(doc, id)).map(|(k, c)| Sel::Object(k, c)).collect();
            sels.extend(kids.into_iter().filter(|k| !sels.contains(k)).collect::<Vec<_>>());
        }
        // the junctions that lose an arm are made again with the rest (two left: a joint)
        let hit = self.junctions_at_ends(&sels);
        let Some(doc) = self.doc.as_mut() else { return };
        selection::delete(doc, &mut self.history, &sels);
        self.pending_rebuild.extend(hit);
        self.finish_step();
        self.select(None);
    }

    // ---------------------------------------------------------------- objects

    fn plan_object(&mut self, assets: &mut Assets, lib: &Library) {
        self.ghost_pos = None;
        self.stop_twin = None;
        self.align_guide = None;
        if self.tool == Tool::Objects && self.plant_brush {
            return;
        }
        let (Some(c), Some(rel)) = (self.cursor, self.object_type.clone()) else { return };
        let mut heading = self.place_heading;
        let mut pos = c;
        if self.snap_grid {
            let q = self.snap_point(c.truncate());
            pos = DVec3::new(q.x, q.y, self.doc.as_ref().map(|d| d.ground(q)).unwrap_or(c.z));
        }
        // a start point, a car park or a bus stop stands in the traffic's way on its side of
        // the road: a start point on the lane itself, a car park and a stop at the kerb
        let (entry, park, stop) = lib.find(&rel).map(|a| (a.is_entry_point, a.is_car_park, a.is_bus_stop)).unwrap_or((false, false, false));
        if self.align_to_road && !self.shift_held && (entry || park || stop) {
            if let Some(doc) = self.doc.as_ref() {
                let lht = doc.global.left_hand_traffic;
                let (lanes, _) = crate::lanes::near(doc, assets, c.truncate(), 25.0);
                let best = lanes.iter().filter(|l| l.kind == 0).filter_map(|l| l.nearest(c.truncate()).map(|n| (n, l.direction))).filter(|(n, _)| n.2 < 12.0).min_by(|a, b| a.0 .2.total_cmp(&b.0 .2));
                if let Some(((q, h, _, _), dir)) = best {
                    let flow = if dir == 1 { roads::norm_deg(h + 180.0) } else { h };
                    // (the kerb: right of the traffic where it keeps right)
                    let kerb = roads::right(flow) * if lht { -1.0 } else { 1.0 };
                    let at = if entry { q.truncate() } else if park { q.truncate() + kerb * 2.6 } else { q.truncate() + kerb * 3.2 };
                    let z = doc.ground(at);
                    let pos = DVec3::new(at.x, at.y, z);
                    self.align_guide = Some((DVec3::new(q.x, q.y, z), pos));
                    self.ghost_pos = Some((pos, roads::norm_deg(flow + self.place_heading)));
                    // the stop across the road: at the kerb of the nearest lane going the
                    // other way
                    if stop && self.stop_both_sides {
                        let back = |h: f64| ((h - flow + 540.0).rem_euclid(360.0) - 180.0).abs() > 135.0;
                        let twin = lanes
                            .iter()
                            .filter(|l| l.kind == 0 && l.direction != 2)
                            .filter_map(|l| l.nearest(q.truncate()).map(|n| (n, if l.direction == 1 { roads::norm_deg(n.1 + 180.0) } else { n.1 })))
                            .filter(|((_, _, d, _), f)| *d > 0.5 && *d < 15.0 && back(*f))
                            .min_by(|a, b| a.0 .2.total_cmp(&b.0 .2));
                        if let Some(((q2, _, _, _), flow2)) = twin {
                            let kerb2 = roads::right(flow2) * if lht { -1.0 } else { 1.0 };
                            let at2 = q2.truncate() + kerb2 * 3.2;
                            self.stop_twin = Some((DVec3::new(at2.x, at2.y, doc.ground(at2)), roads::norm_deg(flow2 + self.place_heading)));
                        }
                    }
                    return;
                }
            }
        }
        // (Shift held: put it down freely)
        let model = assets.object(&rel);
        // (a model brought in from a file stands where it is put: it was not made for a road)
        let imported = rel.to_lowercase().contains("\\imported\\");
        let free = self.shift_held || imported || model.as_ref().map(|m| m.tree.is_some() || m.technical).unwrap_or(false);
        // a junction or crossing piece (spline helpers) clicks onto the road ends near it:
        // turned and moved so one of its helpers meets an end, facing along it
        if let (true, false, Some(m)) = (self.align_to_road, free, model.as_ref().filter(|m| !m.helpers.is_empty())) {
            let reach = m.radius() as f64 + 12.0;
            let mut best: Option<(f64, DVec3, f64, DVec3)> = None;
            for e in self.free_road_ends(assets, c.truncate(), reach) {
                for (h, _) in &m.helpers {
                    let turn = roads::norm_deg(e.heading + 180.0 - h[3] as f64);
                    let off = object_matrix(DVec3::ZERO, [turn, 0.0, 0.0]).transform_point3(Vec3::new(h[0], h[1], h[2])).as_dvec3();
                    let at = e.pos - DVec3::new(off.x, off.y, 0.0);
                    let d = (at.truncate() - c.truncate()).length();
                    if d < reach && best.map(|b| d < b.0).unwrap_or(true) {
                        best = Some((d, at, turn, e.pos));
                    }
                }
            }
            if let Some((_, at, turn, end)) = best {
                let z = self.doc.as_ref().map(|d| d.ground(at.truncate())).unwrap_or(c.z);
                let pos = DVec3::new(at.x, at.y, z);
                self.align_guide = Some((end, pos));
                self.ghost_pos = Some((pos, roads::norm_deg(turn + self.place_heading)));
                return;
            }
        }
        // on the road itself (the cursor within a road's width): it stands there, at the
        // road's height, and aligned it runs along the road; no setback pushes it off the road
        let on_road = if free && !imported { None } else { self.road_under(assets, c.truncate()) };
        if let (true, Some((h, _))) = (self.align_to_road && !free, on_road) {
            heading = roads::norm_deg(h + self.place_heading);
        }
        if self.align_to_road && !free && on_road.is_none() {
            // its depth decides how far from a road it still belongs to it
            let depth = model.as_ref().map(|m| (m.hi.y - m.lo.y) as f64).unwrap_or(10.0);
            if let Some((q, h, n, hw)) = self.nearest_road(assets, c.truncate(), depth + 25.0) {
                // stand facing the road; the user's turns are added
                heading = roads::norm_deg(h + self.place_heading);
                if self.align_front && self.place_heading.rem_euclid(360.0) < 1e-6 {
                    // the front (+y) at the setback from the road's edge
                    let front = model.as_ref().map(|m| m.hi.y as f64).unwrap_or(0.0);
                    let at = q + n * (hw + self.road_setback + front);
                    // (along the road the cursor decides; across it the setback)
                    pos = DVec3::new(at.x, at.y, self.doc.as_ref().map(|d| d.ground(at)).unwrap_or(c.z));
                    let edge = q + n * hw;
                    let z = self.doc.as_ref().map(|d| d.ground(edge)).unwrap_or(c.z);
                    self.align_guide = Some((DVec3::new(edge.x, edge.y, z), pos));
                }
            }
        }
        if let Some((_, z)) = on_road {
            // (the road's own height: a bridge or an embankment is not the ground under it)
            pos.z = z;
        } else if assets.object(&rel).map(|m| m.abs_height).unwrap_or(false) {
            pos.z = c.z;
        }
        self.ghost_pos = Some((pos, self.snap_heading(heading)));
    }

    /// When `p` lies on a road's surface: the way the road runs there and its height.
    fn road_under(&self, assets: &mut Assets, p: DVec2) -> Option<(f64, f64)> {
        let doc = self.doc.as_ref()?;
        let mut best: Option<(f64, f64, f64)> = None;
        for (k, t) in &doc.tiles {
            let org = tile_origin(*k);
            for s in t.tile.splines.iter().filter(|s| !s.deleted) {
                if (DVec2::new(org.x + s.pos[0], org.y + s.pos[1]) - p).length() > s.length + 20.0 {
                    continue;
                }
                let Some(m) = assets.spline(&s.file) else { continue };
                if !m.joinable() {
                    continue;
                }
                let hw = m.half_width as f64;
                let c = omsi_geometry::SplineCurve::from_map(s, org);
                let n = (s.length / 0.5).ceil().max(2.0) as usize;
                for i in 0..=n {
                    let at = s.length * i as f64 / n as f64;
                    let q = c.point_at(at);
                    let d = (q.truncate() - p).length();
                    if d < hw && best.map(|b| d < b.2).unwrap_or(true) {
                        best = Some((c.heading_at(at), q.z, d));
                    }
                }
            }
        }
        best.map(|b| (b.0, b.1))
    }

    /// The free ends of visible roads within `within` m of `p` (as anchors: heading out of
    /// the road).
    fn free_road_ends(&self, assets: &mut Assets, p: DVec2, within: f64) -> Vec<Anchor> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let mut out = Vec::new();
        // (every spline end near: a helper one of them is at is already used)
        let mut taken: Vec<DVec3> = Vec::new();
        for (k, t) in &doc.tiles {
            let org = tile_origin(*k);
            for s in t.tile.splines.iter().filter(|s| !s.deleted) {
                let c = omsi_geometry::SplineCurve::from_map(s, org);
                taken.push(c.point_at(0.0));
                taken.push(c.end_point());
            }
        }
        for (k, t) in &doc.tiles {
            let org = tile_origin(*k);
            for s in t.tile.splines.iter().filter(|s| !s.deleted && (s.prev_id == 0 || s.next_id == 0)) {
                if (DVec2::new(org.x + s.pos[0], org.y + s.pos[1]) - p).length() > s.length + within {
                    continue;
                }
                if !assets.spline(&s.file).map(|m| m.joinable()).unwrap_or(false) {
                    continue;
                }
                let c = omsi_geometry::SplineCurve::from_map(s, org);
                if s.prev_id == 0 && (c.point_at(0.0).truncate() - p).length() < within {
                    out.push(Anchor { pos: c.point_at(0.0), heading: roads::norm_deg(s.heading + 180.0), spline: Some((*k, s.id, false)), mid: None });
                }
                if s.next_id == 0 && (c.end_point().truncate() - p).length() < within {
                    out.push(Anchor { pos: c.end_point(), heading: roads::norm_deg(c.heading_at(s.length)), spline: Some((*k, s.id, true)), mid: None });
                }
            }
            // the helper points of junction pieces already on the map (not those a road
            // already ends at): a piece can be set against another piece too
            for o in &t.tile.objects {
                let Some(model) = assets.object(&o.file) else { continue };
                if model.helpers.is_empty() {
                    continue;
                }
                let world = doc.object_world(*k, o, model.abs_height);
                let m = object_matrix(DVec3::ZERO, o.rot);
                for (h, _) in &model.helpers {
                    let q = world + m.transform_point3(Vec3::new(h[0], h[1], h[2])).as_dvec3();
                    if (q.truncate() - p).length() < within && !taken.iter().any(|t| (t.truncate() - q.truncate()).length() < 1.0) {
                        out.push(Anchor { pos: q, heading: roads::norm_deg(h[3] as f64 + o.rot[0]), spline: None, mid: None });
                    }
                }
            }
        }
        out
    }

    /// The visible road nearest to `p` within `within` m: the point on its centre line, the
    /// heading that faces it, the unit way from the road towards `p`, and its half width.
    fn nearest_road(&self, assets: &mut Assets, p: DVec2, within: f64) -> Option<(DVec2, f64, DVec2, f64)> {
        let doc = self.doc.as_ref()?;
        let mut best: Option<(DVec2, f64, DVec2, f64, f64)> = None;
        for (k, t) in &doc.tiles {
            let org = tile_origin(*k);
            for s in t.tile.splines.iter().filter(|s| !s.deleted) {
                let start = DVec2::new(org.x + s.pos[0], org.y + s.pos[1]);
                if (start - p).length() > s.length + within {
                    continue;
                }
                let Some(m) = assets.spline(&s.file) else { continue };
                if !m.joinable() {
                    continue;
                }
                let hw = m.half_width as f64;
                let c = omsi_geometry::SplineCurve::from_map(s, org);
                let n = (s.length / 1.0).ceil().max(2.0) as usize;
                for i in 0..=n {
                    let at = s.length * i as f64 / n as f64;
                    let q = c.point_at(at).truncate();
                    let d = (q - p).length();
                    if d < within + hw && best.map(|b| d < b.4).unwrap_or(true) {
                        let h = c.heading_at(at);
                        let right = roads::right(h);
                        let side = if (p - q).dot(right) > 0.0 { 1.0 } else { -1.0 };
                        // face the road: the object's front (+y) towards the centre line
                        let face = roads::norm_deg(h + if side > 0.0 { 270.0 } else { 90.0 });
                        best = Some((q, face, right * side, hw, d));
                    }
                }
            }
        }
        best.map(|b| (b.0, b.1, b.2, b.3))
    }


    /// Place the object chosen at the cursor now (dropped from the asset browser).
    pub fn place_now(&mut self, assets: &mut Assets, lib: &Library) {
        self.plan_object(assets, lib);
        self.place_object(lib, false);
    }

    fn place_object(&mut self, lib: &Library, keep: bool) {
        let _ = keep;
        let (Some((pos, heading)), Some(rel)) = (self.ghost_pos, self.object_type.clone()) else {
            if self.object_type.is_none() {
                self.toast(tr("Pick an object in the catalogue first", "Сначала выберите объект в каталоге"));
            }
            return;
        };
        let asset = lib.find(&rel).cloned();
        let Some(doc) = self.doc.as_mut() else { return };
        let k = tile_of(pos.truncate());
        if !doc.tiles.contains_key(&k) {
            self.error(tr("There is no tile here: add one with the Tiles tool", "Здесь нет тайла: добавьте его инструментом «Тайлы»"));
            return;
        }
        self.history.begin(tr("Place object", "Установка объекта"));
        self.history.touch_tile(doc, k);
        self.history.touch_global(doc);
        let id = doc.new_id();
        let org = tile_origin(k);
        let abs = asset.as_ref().map(|a| a.abs_height).unwrap_or(false);
        let mut o = MapObject {
            file: rel.clone(),
            id,
            // (height above the ground, which an object on a raised road needs)
            pos: [pos.x - org.x, pos.y - org.y, if abs { pos.z } else { let up = pos.z - doc.ground(pos.truncate()); if up.abs() > 0.05 { up } else { 0.0 } }],
            rot: [heading, 0.0, 0.0],
            ..Default::default()
        };
        let is_stop = asset.as_ref().map(|a| a.is_bus_stop).unwrap_or(false);
        if is_stop {
            o.extra = vec![tr("New stop", "Новая остановка").to_string(), "0".into(), "0".into(), "10".into()];
            o.flag = 7;
        }
        let is_entry = asset.as_ref().map(|a| a.is_entry_point).unwrap_or(false);
        // a signal head takes its light from the nearest crossing with a light program
        let mut linked_to: Option<String> = None;
        if asset.as_ref().map(|a| a.light_head).unwrap_or(false) {
            let here = pos.truncate();
            let near = doc.tiles.iter().flat_map(|(tk, t)| t.tile.objects.iter().map(move |x| (tile_origin(*tk) + DVec2::new(x.pos[0], x.pos[1]), x))).filter_map(|(p, x)| lib.find(&x.file).filter(|a| a.light_program).map(|a| ((p - here).length(), x.id, a.name.clone()))).filter(|c| c.0 < 40.0).min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, pid, name)) = near {
                o.var_parent = Some(pid);
                if o.extra.is_empty() {
                    o.extra = vec!["0".into()];
                    o.flag = 1;
                }
                linked_to = Some(name);
            }
        }
        let t = doc.tiles.get_mut(&k).unwrap();
        t.tile.objects.push(o.clone());
        t.touch_content();
        // its twin across the road, of the same name
        if let (true, Some((p2, h2))) = (is_stop, self.stop_twin) {
            let k2 = tile_of(p2.truncate());
            if doc.tiles.contains_key(&k2) {
                self.history.touch_tile(doc, k2);
                let org2 = tile_origin(k2);
                let twin = MapObject { id: doc.new_id(), pos: [p2.x - org2.x, p2.y - org2.y, if abs { p2.z } else { 0.0 }], rot: [h2, 0.0, 0.0], ..o };
                let t2 = doc.tiles.get_mut(&k2).unwrap();
                t2.tile.objects.push(twin);
                t2.touch_content();
            }
        }
        if is_entry {
            let name = format!("{} {}", tr("Start", "Старт"), doc.global.entry_points.len() + 1);
            doc.global.entry_points.push(EntryPoint { object_id: id, name, ..Default::default() });
            sync_entry_points(doc);
        }
        self.history.commit();
        if let Some(n) = linked_to {
            self.toast(format!("{} «{n}»: {}", tr("Linked to", "Связан с"), tr("choose its light in its properties", "сигнал выберите в его свойствах")));
        }
        // (the tool goes on placing: nothing is selected)
    }

    /// Plants of `mix` put at `spots` (OpenStreetMap's trees and woods), each a random one
    /// turned a random way, none on a road nor outside the map; one undo step. How many.
    pub fn plant_osm(&mut self, spots: &[DVec2], mix: &[String]) -> usize {
        self.plant_spots(spots, mix, tr("Trees from OpenStreetMap", "Деревья из OpenStreetMap"))
    }

    /// Plants put at `spots` (drawn from `mix`, turned at random), none on a road nor off
    /// the map; one undo step. How many were put.
    fn plant_spots(&mut self, spots: &[DVec2], mix: &[String], label: &'static str) -> usize {
        let Some(doc) = self.doc.as_ref() else { return 0 };
        if mix.is_empty() {
            return 0;
        }
        // the roads, every 3 m with how far from them is clear, in 20 m cells
        let cell = |p: DVec2| ((p.x / 20.0).floor() as i64, (p.y / 20.0).floor() as i64);
        let mut roads: std::collections::HashMap<(i64, i64), Vec<(DVec2, f64)>> = Default::default();
        for (k, t) in &doc.tiles {
            for sp in t.tile.splines.iter().filter(|s| !s.deleted) {
                let clear = self.known_half.get(&sp.file.to_lowercase()).copied().unwrap_or(4.0) + 2.0;
                let c = omsi_geometry::SplineCurve::from_map(sp, tile_origin(*k));
                let n = (sp.length / 3.0).ceil().max(1.0) as usize;
                for i in 0..=n {
                    let q = c.point_at(sp.length * i as f64 / n as f64).truncate();
                    roads.entry(cell(q)).or_default().push((q, clear));
                }
            }
        }
        let on_road = |p: DVec2| {
            let (cx, cy) = cell(p);
            (cx - 1..=cx + 1).any(|x| (cy - 1..=cy + 1).any(|y| roads.get(&(x, y)).map(|v| v.iter().any(|(q, r)| (*q - p).length() < *r)).unwrap_or(false)))
        };
        self.history.begin(label);
        let mut n = 0;
        for p in spots {
            let k = tile_of(*p);
            let Some(doc) = self.doc.as_mut() else { break };
            if !doc.tiles.contains_key(&k) || on_road(*p) {
                continue;
            }
            self.history.touch_tile(doc, k);
            let id = doc.new_id();
            let org = tile_origin(k);
            let file = mix[(self.plant_rng.next() * mix.len() as f64) as usize % mix.len()].clone();
            let o = MapObject { file, id, pos: [p.x - org.x, p.y - org.y, 0.0], rot: [self.plant_rng.next() * 360.0, 0.0, 0.0], ..Default::default() };
            let t = doc.tiles.get_mut(&k).unwrap();
            t.tile.objects.push(o);
            t.touch_content();
            n += 1;
        }
        if n == 0 {
            self.history.cancel();
        } else {
            self.history.touch_global(self.doc.as_ref().unwrap());
            self.history.commit();
        }
        n
    }

    /// The plant outline clicked so far: closed and filled with the mix (or cleared of
    /// plants with the brush on Clear), one undo step. How many were planted or taken away.
    pub fn finish_plant_outline(&mut self, assets: &mut Assets) -> usize {
        let poly = std::mem::take(&mut self.plant_outline);
        if poly.len() < 3 {
            return 0;
        }
        let mix: Vec<String> = if self.plant_mix.is_empty() { self.object_type.iter().cloned().collect() } else { self.plant_mix.clone() };
        let Some(doc) = self.doc.as_mut() else { return 0 };
        if self.plant_erase {
            self.history.begin(tr("Clear plants", "Убрать растения"));
            let n = crate::plants::clear_inside(doc, &mut self.history, assets, &poly, &mix);
            if n == 0 {
                self.history.cancel();
            } else {
                self.history.commit();
            }
            return n;
        }
        if mix.is_empty() {
            return 0;
        }
        let taken = crate::plants::plants_round(doc, assets, &poly, self.plant_spacing, &mix);
        let spots = crate::plants::fill_spots(&poly, &taken, self.plant_spacing, &mut self.plant_rng);
        self.plant_spots(&spots, &mix, tr("Plant an area", "Посадка на участке"))
    }

    /// How near (m) a click must come to an outline's first corner to close it: some
    /// pixels on the screen.
    fn close_radius(&self) -> f64 {
        (self.view_distance.get() * 0.015).clamp(1.0, 40.0)
    }

    /// How many plants the outline being clicked would take (for the hint).
    pub fn plant_outline_estimate(&self) -> Option<usize> {
        let poly = &self.plant_outline;
        if poly.len() < 3 || self.plant_erase {
            return None;
        }
        let area = (0..poly.len()).map(|i| poly[i].perp_dot(poly[(i + 1) % poly.len()])).sum::<f64>().abs() * 0.5;
        Some((area * 0.6 / (self.plant_spacing * self.plant_spacing)).round() as usize)
    }

    /// Buildings from OpenStreetMap: each outline filled with the model of `pool` whose
    /// ground plan comes nearest (a random one of those about as near), its front to the
    /// nearest road within 40 m; an outline no model fits, or one on a road, is left out.
    /// One undo step; how many were put.
    pub fn place_osm_buildings(&mut self, assets: &mut Assets, outlines: &[(Vec<DVec2>, f64, bool)], pool: &[String]) -> usize {
        // the pool's models (their ground plans)
        let t0 = Instant::now();
        let mut sizes: Vec<(String, f64, f64, DVec2, f64)> = Vec::new();
        let mut waiting: Vec<&String> = pool.iter().collect();
        while !waiting.is_empty() && t0.elapsed().as_secs() < 60 {
            assets.poll();
            waiting.retain(|rel| match assets.object(rel) {
                Some(m) => {
                    let (w, d) = ((m.hi.x - m.lo.x) as f64, (m.hi.y - m.lo.y) as f64);
                    // (trees, technical helpers and flat things are no buildings)
                    if m.tree.is_none() && !m.technical && w > 2.0 && d > 2.0 && m.hi.z - m.lo.z > 2.5 {
                        sizes.push(((*rel).clone(), w, d, DVec2::new(((m.lo.x + m.hi.x) * 0.5) as f64, ((m.lo.y + m.hi.y) * 0.5) as f64), (m.hi.z - m.lo.z) as f64));
                    }
                    false
                }
                None => !assets.object_failed(rel),
            });
            if !waiting.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let Some(doc) = self.doc.as_ref() else { return 0 };
        if sizes.is_empty() {
            return 0;
        }
        // the roads, every 2 m with their heading and half width, in 20 m cells
        let cell = |p: DVec2| ((p.x / 20.0).floor() as i64, (p.y / 20.0).floor() as i64);
        let mut roads_at: std::collections::HashMap<(i64, i64), Vec<(DVec2, f64, f64)>> = Default::default();
        for (k, t) in &doc.tiles {
            for sp in t.tile.splines.iter().filter(|s| !s.deleted) {
                let Some(m) = assets.spline(&sp.file) else { continue };
                if !m.joinable() {
                    continue;
                }
                let c = omsi_geometry::SplineCurve::from_map(sp, tile_origin(*k));
                let n = (sp.length / 2.0).ceil().max(1.0) as usize;
                for i in 0..=n {
                    let at = sp.length * i as f64 / n as f64;
                    let q = c.point_at(at).truncate();
                    roads_at.entry(cell(q)).or_default().push((q, c.heading_at(at), m.half_width as f64));
                }
            }
        }
        let nearest = |p: DVec2, within: f64| -> Option<(DVec2, f64, f64)> {
            let (cx, cy) = cell(p);
            let r = (within / 20.0).ceil() as i64;
            let mut best: Option<((DVec2, f64, f64), f64)> = None;
            for x in cx - r..=cx + r {
                for y in cy - r..=cy + r {
                    for q in roads_at.get(&(x, y)).into_iter().flatten() {
                        let d = (q.0 - p).length();
                        if d < within && best.map(|b| d < b.1).unwrap_or(true) {
                            best = Some((*q, d));
                        }
                    }
                }
            }
            best.map(|b| b.0)
        };
        self.history.begin(tr("Buildings from OpenStreetMap", "Здания из OpenStreetMap"));
        let mut n = 0;
        for (poly, height, known) in outlines {
            if poly.len() < 4 {
                continue;
            }
            let c0 = poly.iter().copied().sum::<DVec2>() / poly.len() as f64;
            // front to the nearest road (none: along its longest side); on a road: none
            let heading = match nearest(c0, 40.0) {
                Some((q, h, hw)) => {
                    if (q - c0).length() < hw + 1.0 {
                        continue;
                    }
                    let side = if (c0 - q).dot(roads::right(h)) > 0.0 { 270.0 } else { 90.0 };
                    roads::norm_deg(h + side)
                }
                None => crate::osm_buildings::own_heading(poly),
            };
            let (c, w, d) = crate::osm_buildings::footprint(poly, heading);
            let fits: Vec<(f64, &(String, f64, f64, DVec2, f64))> = sizes.iter().filter_map(|m| crate::osm_buildings::misfit((m.1, m.2, m.4), (w, d, *height), *known).map(|e| (e, m))).collect();
            let Some(best) = fits.iter().map(|f| f.0).min_by(|a, b| a.total_cmp(b)) else { continue };
            let near: Vec<_> = fits.iter().filter(|f| f.0 <= best + 0.12).collect();
            let pick = near[(self.plant_rng.next() * near.len() as f64) as usize % near.len()].1;
            // (the model's own middle on the outline's)
            let at = c - roads::right(heading) * pick.3.x - roads::dir(heading) * pick.3.y;
            let k = tile_of(at);
            let Some(doc) = self.doc.as_mut() else { break };
            if !doc.tiles.contains_key(&k) {
                continue;
            }
            self.history.touch_tile(doc, k);
            let id = doc.new_id();
            let org = tile_origin(k);
            let o = MapObject { file: pick.0.clone(), id, pos: [at.x - org.x, at.y - org.y, 0.0], rot: [heading, 0.0, 0.0], ..Default::default() };
            let t = doc.tiles.get_mut(&k).unwrap();
            t.tile.objects.push(o);
            t.touch_content();
            n += 1;
        }
        if n == 0 {
            self.history.cancel();
        } else {
            self.history.touch_global(self.doc.as_ref().unwrap());
            self.history.commit();
        }
        n
    }

    /// Stops put down where OpenStreetMap has them (`(where, name)`), each at the kerb of
    /// the lane nearest to it as a stop placed by hand stands; one with no lane within 25 m,
    /// or a stop the map has within 10 m, is left out. One undo step; how many were put.
    pub fn place_osm_stops(&mut self, assets: &mut Assets, stops: &[(DVec2, String)], rel: &str) -> usize {
        let Some(doc) = self.doc.as_ref() else { return 0 };
        let lht = doc.global.left_hand_traffic;
        // where the map's stops stand (the ones put here count too)
        let mut stood: Vec<DVec2> = doc.tiles.iter().flat_map(|(k, t)| t.tile.objects.iter().filter(|o| o.extra.len() >= 4 && o.flag == 7).map(move |o| tile_origin(*k) + DVec2::new(o.pos[0], o.pos[1]))).collect();
        self.history.begin(tr("Stops from OpenStreetMap", "Остановки из OpenStreetMap"));
        let mut n = 0;
        for (p, name) in stops {
            let Some(doc) = self.doc.as_ref() else { break };
            let (lanes, _) = crate::lanes::near(doc, assets, *p, 25.0);
            let best = lanes.iter().filter(|l| l.kind == 0).filter_map(|l| l.nearest(*p).map(|q| (q, l.direction))).filter(|(q, _)| q.2 < 25.0).min_by(|a, b| a.0 .2.total_cmp(&b.0 .2));
            let Some(((q, h, _, _), dir)) = best else {
                log::debug!("osm stop {name} at {p:?}: no lane near ({} lanes)", lanes.len());
                continue;
            };
            let flow = if dir == 1 { roads::norm_deg(h + 180.0) } else { h };
            let kerb = roads::right(flow) * if lht { -1.0 } else { 1.0 };
            let at = q.truncate() + kerb * 3.2;
            if stood.iter().any(|s| (*s - at).length() < 10.0) {
                continue;
            }
            let k = tile_of(at);
            let Some(doc) = self.doc.as_mut() else { break };
            if !doc.tiles.contains_key(&k) {
                continue;
            }
            self.history.touch_tile(doc, k);
            let id = doc.new_id();
            let org = tile_origin(k);
            let label = if name.trim().is_empty() { tr("Stop", "Остановка").to_string() } else { name.trim().to_string() };
            let o = MapObject { file: rel.to_string(), id, pos: [at.x - org.x, at.y - org.y, 0.0], rot: [flow, 0.0, 0.0], extra: vec![label, "0".into(), "0".into(), "10".into()], flag: 7, ..Default::default() };
            let t = doc.tiles.get_mut(&k).unwrap();
            t.tile.objects.push(o);
            t.touch_content();
            stood.push(at);
            n += 1;
        }
        if n == 0 {
            self.history.cancel();
        } else {
            self.history.touch_global(self.doc.as_ref().unwrap());
            self.history.commit();
        }
        n
    }

    /// A car park click with rows on: the first click starts a row, the second fills it to
    /// there with car parks every `parking_step` metres, standing along it.
    fn parking_click(&mut self, assets: &mut Assets, lib: &Library) {
        let Some(c) = self.cursor else { return };
        if self.object_type.is_none() {
            self.toast(tr("Pick a car park in the list first", "Сначала выберите парковку в списке"));
            return;
        }
        let Some(a) = self.parking_start.take() else {
            self.parking_start = Some(c);
            return;
        };
        let d = (c - a).truncate();
        let len = d.length();
        if len < 0.5 {
            self.parking_start = Some(c);
            return;
        }
        // (a car's front along the row; the user's turns are added)
        let heading = roads::norm_deg(roads::heading_of(d) + self.place_heading);
        let n = (len / self.parking_step).floor() as usize + 1;
        let step = if n > 1 { len / (n - 1) as f64 } else { 0.0 };
        let dir = d / len;
        let rel = self.object_type.clone().unwrap();
        let abs = assets.object(&rel).map(|m| m.abs_height).unwrap_or(false);
        self.history.begin(tr("Place car parks", "Установка парковок"));
        let auto_terrain = self.auto_terrain;
        let mut placed = 0;
        for i in 0..n {
            let p = a.truncate() + dir * step * i as f64;
            let Some(doc) = self.doc.as_mut() else { return };
            let k = tile_of(p);
            if !doc.tiles.contains_key(&k) {
                continue;
            }
            self.history.touch_tile(doc, k);
            let org = tile_origin(k);
            let id = doc.new_id();
            let z = if abs && auto_terrain { doc.ground(p) } else { 0.0 };
            let t = doc.tiles.get_mut(&k).unwrap();
            t.tile.objects.push(MapObject { file: rel.clone(), id, pos: [p.x - org.x, p.y - org.y, z], rot: [heading, 0.0, 0.0], ..Default::default() });
            t.touch_content();
            placed += 1;
        }
        self.history.commit();
        let _ = lib;
        self.toast(format!("{} {placed}", tr("Car parks placed:", "Парковок поставлено:")));
    }

    // ---------------------------------------------------------------- roads

    /// Free spline ends and junction connectors near a point.
    /// (and the spline type that is there, which a road started from it goes on with)
    fn snap_anchor(&self, assets: &mut Assets, p: DVec2, within: f64) -> Option<(Anchor, Option<String>)> {
        let doc = self.doc.as_ref()?;
        let mut best: Option<(f64, Anchor, Option<String>)> = None;
        let mut consider = |d: f64, a: Anchor, file: &str| {
            if d < within && best.as_ref().map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, a, (!file.trim().is_empty()).then(|| file.to_string())));
            }
        };
        // (a track joins tracks, a road roads: rails and asphalt only cross)
        let track = self.drawing_track(assets);
        for (k, t) in &doc.tiles {
            let org = tile_origin(*k);
            for s in t.tile.splines.iter().filter(|s| !s.deleted) {
                if assets.spline(&s.file).map(|m| !m.joinable() || m.is_track() != track).unwrap_or(true) {
                    continue;
                }
                let c = omsi_geometry::SplineCurve::from_map(s, org);
                if s.prev_id == 0 {
                    let a = c.point_at(0.0);
                    consider((a.truncate() - p).length(), Anchor { pos: a, heading: roads::norm_deg(s.heading + 180.0), spline: Some((*k, s.id, false)), mid: None }, &s.file);
                }
                if s.next_id == 0 {
                    let e = c.end_point();
                    consider((e.truncate() - p).length(), Anchor { pos: e, heading: roads::norm_deg(c.heading_at(s.length)), spline: Some((*k, s.id, true)), mid: None }, &s.file);
                }
            }
            for o in &t.tile.objects {
                let Some(model) = assets.object(&o.file) else { continue };
                if model.helpers.is_empty() {
                    continue;
                }
                let world = doc.object_world(*k, o, model.abs_height);
                let m = object_matrix(DVec3::ZERO, o.rot);
                for (h, file) in &model.helpers {
                    // (h[3]: the way out of the junction, in the object's frame)
                    let q = world + m.transform_point3(Vec3::new(h[0], h[1], h[2])).as_dvec3();
                    consider((q.truncate() - p).length(), Anchor { pos: q, heading: roads::norm_deg(h[3] as f64 + o.rot[0]), spline: None, mid: None }, file);
                }
            }
        }
        if best.is_some() {
            return best.map(|b| (b.1, b.2));
        }
        // no end near: a point in the middle of a road (the road is cut there). A track's
        // middle is joined only by a switch: with Auto-Switch off, tracks meet at ends only.
        if track && !self.auto_switches {
            return None;
        }
        let mut mid: Option<(f64, Anchor, String)> = None;
        for (k, t) in &doc.tiles {
            let org = tile_origin(*k);
            for s in t.tile.splines.iter().filter(|s| !s.deleted) {
                let start = DVec2::new(org.x + s.pos[0], org.y + s.pos[1]);
                if (start - p).length() > s.length + 40.0 {
                    continue;
                }
                let Some(m) = assets.spline(&s.file) else { continue };
                // (only roads one sees: not a fence, a footpath, or the invisible AI splines
                // under a map's roads; and of the family drawn)
                if !m.joinable() || m.is_track() != track {
                    continue;
                }
                let hw = m.half_width as f64;
                let c = omsi_geometry::SplineCurve::from_map(s, org);
                let n = (s.length / 1.0).ceil().max(2.0) as usize;
                for i in 1..n {
                    let at = s.length * i as f64 / n as f64;
                    // (right at a joint of two splines too: the cut goes just inside one)
                    if at < 0.6 || at > s.length - 0.6 {
                        continue;
                    }
                    let q = c.point_at(at);
                    let d = (q.truncate() - p).length();
                    if d < hw + 2.0 && mid.as_ref().map(|b| d < b.0).unwrap_or(true) {
                        mid = Some((d, Anchor { pos: q, heading: roads::norm_deg(c.heading_at(at) + 90.0), spline: None, mid: Some((*k, s.id, at, hw)) }, s.file.clone()));
                    }
                }
            }
        }
        mid.map(|m| (m.1, Some(m.2)))
    }

    /// The type being drawn is a track (rails meet in switches, not junctions).
    fn drawing_track(&self, assets: &mut Assets) -> bool {
        !self.drawing_paths() && self.road_type.as_deref().and_then(|r| assets.spline(r)).map(|m| m.is_track()).unwrap_or(false)
    }

    /// A switch with its toe at `toe`, opening towards `heading`, its branch to the left or
    /// right: the plan and its diverging heel (place, heading out of the switch). None while
    /// the switch's model loads, or where the installation has none.
    fn switch_plan(&self, assets: &mut Assets, toe: DVec3, heading: f64, left: bool) -> Option<(crate::switches::Plan, (DVec3, f64))> {
        if !self.auto_switches {
            return None;
        }
        let track = self.road_type.as_deref()?;
        let rel = self.best_switch(assets, track, left);
        let model = assets.object(&rel)?;
        let shape = crate::switches::Shape::of(&model)?;
        let (mut pos, out) = shape.world(shape.branch, toe, heading);
        pos.z = self.doc.as_ref().map(|d| d.ground(pos.truncate())).unwrap_or(toe.z);
        let plan = crate::switches::Plan { rel, toe, heading, length: shape.length(), origin: shape.origin(toe, heading) };
        Some((plan, (pos, out)))
    }

    /// The switch that looks most like the track (the one whose model shares the most of
    /// its textures; while the models load, the stock one for its kind).
    fn best_switch(&self, assets: &mut Assets, track: &str, left: bool) -> String {
        let name = |p: &std::path::PathBuf| p.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
        let track_tex: Vec<String> = assets.spline(track).map(|m| m.materials.iter().filter_map(|t| t.texture.as_ref().map(name)).collect()).unwrap_or_default();
        let mut best: Option<(i32, String)> = None;
        for c in self.switch_catalogue.iter().filter(|c| c.left == left) {
            let Some(m) = assets.object(&c.rel) else { continue };
            let tex: Vec<String> = m.parts.iter().flat_map(|p| p.materials.iter()).filter_map(|t| t.texture.as_ref().map(name)).collect();
            let score = crate::switches::fit(track, &track_tex, &c.rel, &tex);
            if best.as_ref().map(|b| score > b.0).unwrap_or(true) {
                best = Some((score, c.rel.clone()));
            }
        }
        best.map(|b| b.1).unwrap_or_else(|| crate::switches::object_for(track, left))
    }

    /// The way a spline runs at a point in its middle (degrees).
    fn heading_in(&self, mid: ((i32, i32), i64, f64, f64)) -> Option<f64> {
        let (k, id, at, _) = mid;
        let s = self.doc.as_ref()?.spline(k, id)?;
        Some(omsi_geometry::SplineCurve::from_map(s, tile_origin(k)).heading_at(at))
    }

    /// The kind of lane the AI path type carries (0 cars, 1 people).
    fn path_kind(&self, assets: &mut Assets) -> i32 {
        self.path_type.as_deref().and_then(|r| assets.spline(r)).and_then(|m| m.def.paths.first().map(|p| p.kind)).unwrap_or(0)
    }

    /// Where an AI path may start (`starting`) or end near a point: a lane end of its kind
    /// that traffic leaves by (to start) or comes in by (to end), else a point along a lane,
    /// heading the way its traffic goes.
    fn snap_lane(&self, assets: &mut Assets, p: DVec2, starting: bool) -> Option<(Anchor, Option<String>)> {
        let kind = self.path_kind(assets);
        self.snap_lane_kind(assets, p, starting, kind)
    }

    /// The lane of `kind` near a point to join: its end, else a place along it.
    fn snap_lane_kind(&self, assets: &mut Assets, p: DVec2, starting: bool, kind: i32) -> Option<(Anchor, Option<String>)> {
        let doc = self.doc.as_ref()?;
        let (lanes, ends) = crate::lanes::near(doc, assets, p, 80.0);
        let file = self.path_type.clone();
        let end = ends
            .iter()
            .filter(|e| e.kind == kind && if starting { e.exit } else { e.entry })
            .map(|e| ((e.pos.truncate() - p).length(), e))
            .filter(|(d, _)| *d < 4.0)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, e)) = end {
            return Some((Anchor { pos: e.pos, heading: e.heading, spline: None, mid: None }, file));
        }
        let along = lanes
            .iter()
            .filter(|l| l.kind == kind)
            .filter_map(|l| l.nearest(p).map(|n| (n, l.direction, l.spline)))
            .filter(|((_, _, d, _), _, _)| *d < 2.0)
            .min_by(|a, b| a.0 .2.total_cmp(&b.0 .2));
        along.map(|((q, h, _, at), dir, sp)| {
            let flow = if dir == 1 { h + 180.0 } else { h };
            // (on a spline's lane: the spline is cut there when the path is built, so the lane
            // has an end there for the path to join - openOMSI joins lanes only end to end)
            let mid = sp.zip(at).filter(|((_, _, len), at)| *at > 0.6 && *at < len - 0.6).map(|((k, id, _), at)| (k, id, at, 0.0));
            (Anchor { pos: q, heading: roads::norm_deg(if starting { flow } else { flow + 180.0 }), spline: None, mid }, file)
        })
    }

    /// A path starts at a lane end of its own kind: near the end of a footway lane the path type
    /// becomes the footpath, near a street lane's the street.
    fn follow_lane_kind(&mut self, assets: &mut Assets, p: DVec2) {
        let Some(doc) = self.doc.as_ref() else { return };
        let (_, ends) = crate::lanes::near(doc, assets, p, 12.0);
        let near = ends.iter().filter(|e| e.kind < 2 && (e.pos.truncate() - p).length() < 3.0).min_by(|a, b| (a.pos.truncate() - p).length().total_cmp(&(b.pos.truncate() - p).length()));
        let Some(kind) = near.map(|e| e.kind) else { return };
        if kind != self.path_kind(assets) {
            self.set_path_type((if kind == 1 { "Splines\\invis_sidewalk.sli" } else { "Splines\\Marcel\\invis_street.sli" }).to_string());
        }
    }

    /// The road lane (not an AI path of its own) under the cursor while AI paths are edited: the
    /// road's pieces to make editable, and where it was clicked.
    fn road_lane_under_cursor(&self, assets: &mut Assets) -> Option<(Vec<(TileKey, i64)>, DVec2)> {
        let doc = self.doc.as_ref()?;
        let p = self.cursor?.truncate();
        let (lanes, _) = crate::lanes::near(doc, assets, p, 12.0);
        let hit = lanes.iter().filter(|l| l.kind < 2 && l.spline.is_some()).filter_map(|l| l.nearest(p).map(|n| (n.2, l.spline.unwrap()))).filter(|(d, _)| *d < 1.2).min_by(|a, b| a.0.total_cmp(&b.0))?;
        let (k, id, _) = hit.1;
        let file = doc.spline(k, id)?.file.clone();
        if assets.spline(&file).map(|m| m.is_ai_path()).unwrap_or(true) {
            return None;
        }
        // (the road's pieces along it, up to 40 each way: a lane goes on through them)
        let mut pieces = vec![(k, id)];
        for dir in [false, true] {
            let mut cur = doc.spline(k, id)?.clone();
            for _ in 0..40 {
                let n = if dir { cur.next_id } else { cur.prev_id };
                let Some((nk, next)) = (n != 0).then(|| doc.find_spline(n)).flatten() else { break };
                if !next.file.eq_ignore_ascii_case(&file) || pieces.contains(&(nk, n)) {
                    break;
                }
                pieces.push((nk, n));
                cur = next.clone();
            }
        }
        Some((pieces, p))
    }

    /// The AI path nearest to a point picked, with the pieces round it.
    pub fn pick_ai_path_near(&mut self, assets: &mut Assets, p: DVec2) {
        let Some(doc) = self.doc.as_ref() else { return };
        let mut best: Option<(f64, TileKey, i64)> = None;
        for (k, t) in &doc.tiles {
            for s in t.tile.splines.iter().filter(|s| !s.deleted) {
                if !assets.spline(&s.file).map(|m| m.is_ai_path()).unwrap_or(false) {
                    continue;
                }
                let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(*k));
                let n = (s.length / 1.0).ceil().max(2.0) as usize;
                for i in 0..=n {
                    let d = (c.point_at(s.length * i as f64 / n as f64).truncate() - p).length();
                    if d < 2.0 && best.map(|b| d < b.0).unwrap_or(true) {
                        best = Some((d, *k, s.id));
                    }
                }
            }
        }
        if let Some((_, k, id)) = best {
            self.select_path_pieces(k, id);
        }
    }

    fn plan_road(&mut self, assets: &mut Assets) {
        self.road_plan = None;
        let Some(c) = self.cursor else { return };
        // (an end pulls the cursor only from close by: aimed at a road a little way from its
        // end, the road is joined there, not bent round to the end)
        if self.drawing_paths() && self.road_start.is_none() {
            self.follow_lane_kind(assets, c.truncate());
        }
        let found = if self.drawing_paths() { self.snap_lane(assets, c.truncate(), self.road_start.is_none()) } else if self.snap_roads { self.snap_anchor(assets, c.truncate(), 5.0) } else { None };
        self.road_snap_file = found.as_ref().and_then(|f| f.1.clone());
        let snap = found.map(|f| f.0);
        // (the start end itself is not a target)
        let snap = snap.filter(|a| self.road_start.as_ref().map(|(s, _)| (s.pos - a.pos).length() > 1.0).unwrap_or(true));
        self.road_snap = snap;
        let Some((mut start, fixed)) = self.road_start else { return };
        let mut target = if snap.is_some() { c.truncate() } else { self.snap_point(c.truncate()) };
        // a straight from a free start: its heading to whole steps
        if !fixed && snap.is_none() && self.snap_angle {
            let d = target - start.pos.truncate();
            target = start.pos.truncate() + roads::dir(snap_deg(roads::heading_of(d), self.angle_step)) * d.length();
        }
        let mut end_anchor = snap;
        // into a junction plate: to its centre (it is cut back to the edge and becomes an arm)
        self.road_plate_end = None;
        if snap.is_none() && !self.drawing_paths() {
            if let Some(p) = self.plate_at(assets, c.truncate()) {
                target = p.1.truncate();
                self.road_plate_end = Some(p);
            }
        }
        // tracks branch off and run into one another as switches: along the track, not
        // across it
        let track = self.drawing_track(assets);
        let mut fixed = fixed;
        self.switch_plans.clear();
        if track {
            if let Some(h) = start.mid.and_then(|m| self.heading_in(m)) {
                // (the way along the track that leans towards where the new one goes, and the
                // side it goes off to)
                let towards = target - start.pos.truncate();
                let travel = if roads::dir(h).dot(towards) >= 0.0 { h } else { roads::norm_deg(h + 180.0) };
                let left = towards.dot(roads::right(travel)) < 0.0;
                match self.switch_plan(assets, start.pos, travel, left) {
                    // a switch: the new track leaves from its diverging heel
                    Some((plan, (pos, heading))) => {
                        start.pos = pos;
                        start.heading = heading;
                        self.switch_plans.push(plan);
                    }
                    None => start.heading = travel,
                }
                fixed = true;
            }
            if let Some(a) = snap.filter(|a| a.mid.is_some()) {
                if let Some(h) = self.heading_in(a.mid.unwrap()) {
                    // a switch whose heels face the new track: it arrives at the diverging one
                    let from = start.pos.truncate() - a.pos.truncate();
                    let open = if roads::dir(h).dot(from) >= 0.0 { h } else { roads::norm_deg(h + 180.0) };
                    let left = from.dot(roads::right(open)) < 0.0;
                    match self.switch_plan(assets, a.pos, open, left) {
                        Some((plan, (pos, heading))) => {
                            target = pos.truncate();
                            end_anchor = Some(Anchor { pos, heading, spline: None, mid: None });
                            self.switch_plans.push(plan);
                        }
                        None => {
                            target = a.pos.truncate();
                            end_anchor = Some(Anchor { pos: a.pos, heading: open, spline: None, mid: None });
                        }
                    }
                }
            }
        }
        // joining a road in its middle: the new road meets its edge, coming straight at it
        if let Some(a) = snap.filter(|a| a.mid.is_some() && !self.drawing_paths() && !track) {
            let (_, _, _, hw) = a.mid.unwrap();
            let from = start.pos.truncate();
            let d = (a.pos.truncate() - from).normalize_or_zero();
            // (a junction is made at the centre line; without one the road stops at the edge)
            target = if self.auto_junctions { a.pos.truncate() } else { a.pos.truncate() - d * hw };
            end_anchor = None;
        }
        // leaving a road from its middle: from its centre line (a junction) or its edge
        if let (Some((_, _, _, hw)), false, false, false) = (start.mid, self.auto_junctions, self.drawing_paths(), track) {
            let d = (target - start.pos.truncate()).normalize_or_zero();
            start.pos += DVec3::new(d.x, d.y, 0.0) * hw;
        }
        let typed = self.typed();
        let z0 = start.pos.z;
        // numbers typed or the straight lock: the road is drawn by them, not to what the
        // cursor is near
        if (typed.any() || self.straight_lock) && !self.drawing_paths() {
            let Some(segs) = crate::draw::plan(start.pos.truncate(), fixed.then_some(start.heading), self.snap_point(c.truncate()), &typed, self.straight_lock) else { return };
            self.road_snap = None;
            let end = segs.last().unwrap().end();
            let z1 = if self.keep_height { z0 + self.typed_rise() } else if !self.auto_terrain { z0 } else { self.doc.as_ref().map(|d| d.ground(end)).unwrap_or(c.z) };
            self.road_plan = Some((segs, z0, z1, None));
            return;
        }
        // (a cursor on the plane far beyond the map, looking at the horizon: no road there)
        let Some(segs) = roads::plan(&start, fixed, target, end_anchor.as_ref()).filter(|s| roads::total_length(s) <= crate::draw::MAX_LENGTH) else { return };
        let z1 = match snap {
            _ if self.road_plate_end.is_some() => self.road_plate_end.unwrap().1.z,
            Some(a) => a.pos.z,
            None if self.keep_height => z0 + self.typed_rise(),
            None if !self.auto_terrain => z0,
            None => c.z,
        };
        self.road_plan = Some((segs, z0, z1, snap));
    }

    fn road_click(&mut self, assets: &mut Assets, lib: &Library) {
        let Some(rel) = self.road_type.clone() else {
            self.toast(tr("Pick a road type in the catalogue first", "Сначала выберите тип дороги в каталоге"));
            return;
        };
        let Some(c) = self.cursor else { return };
        if self.road_start.is_none() {
            // a road started from another one goes on with its type
            if self.inherit_type && !self.drawing_paths() {
                if let (Some(_), Some(f)) = (self.road_snap, self.road_snap_file.clone()) {
                    if lib.find(&f).is_some() && assets.spline(&f).is_some() {
                        self.road_type = Some(f);
                    }
                }
            }
            match self.road_snap {
                // (from the middle of a road the heading is free)
                Some(a) if a.mid.is_some() && !self.drawing_paths() => self.road_start = Some((a, false)),
                Some(a) => self.road_start = Some((a, true)),
                None => {
                    // from inside a junction plate: from its centre, as a new arm of it
                    self.road_plate_start = self.plate_at(assets, c.truncate());
                    let q = self.snap_point(c.truncate());
                    let z = self.doc.as_ref().map(|d| d.ground(q)).unwrap_or(c.z);
                    let pos = self.road_plate_start.map(|p| p.1).unwrap_or(DVec3::new(q.x, q.y, z));
                    self.road_start = Some((Anchor { pos, ..Default::default() }, false));
                }
            }
            return;
        }
        let Some((segs, z0, z1, end)) = self.road_plan.clone() else {
            self.error(tr("That bend is too sharp", "Слишком крутой поворот"));
            return;
        };
        let (start, _) = self.road_start.unwrap();
        let half = assets.spline(&rel).map(|m| m.half_width).unwrap_or(4.0);
        // a road met in its middle is cut in two there: the joint is where a junction goes
        let mids: Vec<_> = [start.mid, end.and_then(|e| e.mid)].into_iter().flatten().collect();
        let end_link = end.filter(|e| e.mid.is_none());
        let start_link = if start.mid.is_some() { Anchor { spline: None, ..start } } else { start };
        let made = self.build_road(&rel, &segs, z0, z1, start_link, end_link, half as f64);
        let mut last = made.as_ref().and_then(|m| m.last().copied());
        if let Some(made) = &made {
            if self.drawing_track(assets) {
                // a switch: the track met is cut where the new one leaves or joins it, so the
                // rails' lanes have ends there for trains to choose between
                let plans = std::mem::take(&mut self.switch_plans);
                let switched = !plans.is_empty();
                if let Some(doc) = self.doc.as_mut() {
                    if plans.is_empty() {
                        for (k, id, at, _) in mids {
                            doc.split_spline(k, id, at);
                        }
                    }
                    // the switch objects, the main track taken out under them; where the main
                    // track bends, it is brought round to the switch's straight heel again
                    for p in &plans {
                        let Some(laid) = crate::switches::lay(doc, p) else { continue };
                        if let (Some(r), Some(shape)) = (laid.rejoin, assets.object(&p.rel).and_then(|m| crate::switches::Shape::of(&m))) {
                            let heel = shape.world(shape.straight, p.toe, p.heading);
                            rejoin_track(doc, r, heel);
                        }
                    }
                }
                if switched {
                    self.toast(tr("Switch made", "Стрелка создана"));
                }
            } else if self.auto_junctions && !self.drawing_paths() {
                let start_mid = start.mid.map(|m| (m, start.pos));
                let end_mid = end.and_then(|e| e.mid.map(|m| (m, e.pos)));
                let n = self.make_junctions(assets, made, half as f64, start_mid, end_mid);
                if n > 0 {
                    self.toast(format!("{} {n}", tr("Junctions made:", "Перекрёстков создано:")));
                }
                // drawing goes on from the road's end, if a junction has not taken it
                last = last.filter(|l| self.doc.as_ref().map(|d| d.spline(l.0, l.1).is_some()).unwrap_or(false));
            } else if let Some(doc) = self.doc.as_mut() {
                for (k, id, at, _) in mids {
                    doc.split_spline(k, id, at);
                }
            }
            // a road from or into a junction plate: cut back to its edge, an arm of it
            let plates = [(self.road_plate_start.take(), made.first().copied(), false), (self.road_plate_end.take(), made.last().copied(), true)];
            for (plate, end, at_end) in plates {
                let (Some((jid, _, r)), Some((k, sid))) = (plate, end) else { continue };
                let Some(doc) = self.doc.as_mut() else { break };
                if let Some(arm) = doc.trim(k, sid, at_end, r) {
                    self.pending_arms.entry(jid).or_default().push(crate::junction::Arm { key: arm.0, id: arm.1, at_end: arm.2 });
                    self.pending_rebuild.push(jid);
                    if at_end {
                        last = None;
                    }
                }
            }
            self.finish_step();
        }
        let made = if end.is_none() { last } else { made.and_then(|m| m.last().copied()) };
        let _ = lib;
        match (made, end) {
            (Some(last), None) if self.keep_drawing => {
                // go on from the end just made
                let (k, id) = last;
                let doc = self.doc.as_ref().unwrap();
                if let Some(s) = doc.spline(k, id) {
                    let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(k));
                    self.road_start = Some((Anchor { pos: c.end_point(), heading: roads::norm_deg(c.heading_at(s.length)), spline: Some((k, id, true)), mid: None }, true));
                }
            }
            _ => {
                self.road_start = None;
            }
        }
        self.road_plan = None;
    }

    /// Make the splines of a planned road. Returns the last one.
    fn build_road(&mut self, rel: &str, segs: &[Seg], z0: f64, z1: f64, start: Anchor, end: Option<Anchor>, half: f64) -> Option<Vec<(TileKey, i64)>> {
        let doc = self.doc.as_mut()?;
        let pieces = roads::split(segs, 50.0);
        let total = roads::total_length(&pieces).max(0.1);
        for p in &pieces {
            if !doc.tiles.contains_key(&tile_of(p.start)) {
                self.error(tr("The road leaves the map: add tiles there first", "Дорога выходит за карту: сначала добавьте тайлы"));
                return None;
            }
        }
        self.history.begin(tr("Build road", "Строительство дороги"));
        let keys: Vec<TileKey> = doc.tiles.keys().copied().collect();
        for k in keys {
            self.history.touch_tile(doc, k);
        }
        self.history.touch_global(doc);
        let _ = total;
        self.lay_road(rel, &pieces, z0, z1, start, end, half)
    }

    /// The splines of a road, cut into pieces (`roads::split`), put into the map, linked to
    /// what its ends meet; inside an open step that keeps every tile.
    fn lay_road(&mut self, rel: &str, pieces: &[Seg], z0: f64, z1: f64, start: Anchor, end: Option<Anchor>, half: f64) -> Option<Vec<(TileKey, i64)>> {
        let doc = self.doc.as_mut()?;
        let pieces = pieces.to_vec();
        let profile = if self.keep_height || !self.auto_terrain { even_profile(&pieces, z0, z1) } else { road_profile(doc, &pieces, z0, z1) };
        let mut made: Vec<(TileKey, i64)> = Vec::new();
        let ids: Vec<i64> = pieces.iter().map(|_| doc.new_id()).collect();
        let mut curves = Vec::new();
        let mut chain = 0.0;
        for (i, p) in pieces.iter().enumerate() {
            let k = tile_of(p.start);
            let org = tile_origin(k);
            let sp = piece_spline(rel, p, &profile, i, org);
            let sp = MapSpline {
                id: ids[i],
                prev_id: if i > 0 { ids[i - 1] } else { 0 },
                next_id: if i + 1 < ids.len() { ids[i + 1] } else { 0 },
                // (textures run on across the joints)
                tex_offset: chain,
                map_chain_offset: Some(chain),
                ..sp
            };
            chain += p.length;
            curves.push(omsi_geometry::SplineCurve::from_map(&sp, org));
            let t = doc.tiles.get_mut(&k).unwrap();
            t.tile.splines.push(sp);
            t.touch_content();
            made.push((k, ids[i]));
        }
        // link the ends to what they meet (whether it is still there with that end free: a
        // one-sided link would point at nothing)
        let link = |doc: &mut Document, at: Option<(TileKey, i64, bool)>, mine: i64| -> bool {
            let mut linked = false;
            if let Some((k, id, at_end)) = at {
                if let Some(t) = doc.tiles.get_mut(&k) {
                    if let Some(s) = t.tile.splines.iter_mut().find(|s| s.id == id && !s.deleted) {
                        if at_end && s.next_id == 0 {
                            s.next_id = mine;
                            linked = true;
                        } else if !at_end && s.prev_id == 0 {
                            s.prev_id = mine;
                            linked = true;
                        }
                    }
                    t.touch_content();
                }
            }
            linked
        };
        if let Some((k, id, _)) = start.spline {
            let first = made[0];
            if link(doc, start.spline, first.1) {
                if let Some(t) = doc.tiles.get_mut(&first.0) {
                    if let Some(s) = t.tile.splines.iter_mut().find(|s| s.id == first.1) {
                        s.prev_id = id;
                    }
                }
            }
            let _ = k;
        }
        if let Some(e) = end {
            if let Some((_, id, _)) = e.spline {
                let last = *made.last().unwrap();
                if link(doc, e.spline, last.1) {
                    if let Some(t) = doc.tiles.get_mut(&last.0) {
                        if let Some(s) = t.tile.splines.iter_mut().find(|s| s.id == last.1) {
                            s.next_id = id;
                        }
                    }
                }
            }
        }
        if self.follow_ground && self.auto_terrain && !self.keep_height && !(self.tool == Tool::Logic) {
            conform_ground(doc, &curves, half);
        }
        // (the step stays open: the caller may cut the road it joined, in the same step)
        Some(made)
    }

    /// Roads from OpenStreetMap built as if drawn one after the other (in `osm::roads`'
    /// order; a one-way road with `oneway`'s type when there is one): each joins the roads it meets with junctions, or goes on from a road end in
    /// line with it; one undo step. A road lying mostly on a road the map had is left out
    /// (built before). Gives back how many roads and junctions were made.
    pub fn build_osm(&mut self, assets: &mut Assets, list: &[crate::osm::Road], types: &[(crate::osm::Class, String)], oneway: Option<&str>) -> (usize, usize) {
        // (the types' widths and kinds first, and the map's own roads': junctions are made
        // only with roads whose kind is known)
        let t0 = Instant::now();
        let mut files: Vec<String> = types.iter().map(|t| t.1.clone()).chain(oneway.map(str::to_string)).collect();
        if let Some(doc) = self.doc.as_ref() {
            let mut seen = std::collections::HashSet::new();
            files.extend(doc.tiles.values().flat_map(|t| t.tile.splines.iter().filter(|s| !s.deleted).map(|s| s.file.clone())).filter(|f| seen.insert(f.to_lowercase())));
        }
        for t in &files {
            while assets.spline(t).is_none() && !assets.spline_failed(t) && t0.elapsed().as_secs() < 20 {
                assets.poll();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        // (and the switches' models, where tracks branch: their shape places the new track)
        if self.auto_switches && list.iter().any(|r| r.class.is_track()) {
            let rels: Vec<String> = self.switch_catalogue.iter().map(|c| c.rel.clone()).collect();
            for r in &rels {
                while assets.object(r).is_none() && !assets.object_failed(r) && t0.elapsed().as_secs() < 25 {
                    assets.poll();
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
        let Some(doc) = self.doc.as_ref() else { return (0, 0) };
        // where the map's roads run, every 3 m in 10 m cells
        let cell = |p: DVec2| ((p.x / 10.0).floor() as i64, (p.y / 10.0).floor() as i64);
        let mut had: std::collections::HashMap<(i64, i64), Vec<DVec2>> = Default::default();
        for (k, t) in &doc.tiles {
            for sp in t.tile.splines.iter().filter(|s| !s.deleted) {
                let c = omsi_geometry::SplineCurve::from_map(sp, tile_origin(*k));
                let n = (sp.length / 3.0).ceil().max(1.0) as usize;
                for i in 0..=n {
                    let q = c.point_at(sp.length * i as f64 / n as f64).truncate();
                    had.entry(cell(q)).or_default().push(q);
                }
            }
        }
        let near_old = |p: DVec2| {
            let (cx, cy) = cell(p);
            (cx - 1..=cx + 1).any(|x| (cy - 1..=cy + 1).any(|y| had.get(&(x, y)).map(|v| v.iter().any(|q| (*q - p).length() < 4.0)).unwrap_or(false)))
        };
        self.history.begin(tr("Roads from OpenStreetMap", "Дороги из OpenStreetMap"));
        let keys: Vec<TileKey> = doc.tiles.keys().copied().collect();
        for k in keys {
            self.history.touch_tile(doc, k);
        }
        self.history.touch_global(doc);
        let was = self.road_type.clone();
        let (mut built, mut junctions, mut switches, mut joined_plates) = (0, 0, 0, 0);
        let mut ends_made: Vec<Option<(TileKey, i64, bool)>> = Vec::new();
        for r in list {
            let Some(rel) = types.iter().find(|t| t.0 == r.class).map(|t| t.1.clone()) else { continue };
            let track = r.class.is_track();
            let rel = match oneway {
                Some(o) if r.oneway && !track => o.to_string(),
                _ => rel,
            };
            let Some(model) = assets.spline(&rel) else { continue };
            let half = model.half_width as f64;
            let mut segs = crate::osm::segments(&r.points, r.class);
            if segs.is_empty() {
                continue;
            }
            let samples: Vec<DVec2> = segs.iter().flat_map(|s| {
                let c = s.curve();
                let n = (s.length / 5.0).ceil().max(1.0) as usize;
                (0..=n).map(move |i| c.point_at(s.length * i as f64 / n as f64).truncate())
            }).collect();
            // (a tram's track lies on its street: only roads are checked so)
            if !track && samples.iter().filter(|p| near_old(**p)).count() * 2 > samples.len() {
                continue;
            }
            self.road_type = Some(rel.clone());
            let (p0, p1) = (segs[0].start, segs.last().unwrap().end());
            // (a track may meet another's middle only where a switch is made)
            let mid_ok = |a: &Anchor| !track || a.mid.is_none() || self.auto_switches;
            // an end in a junction plate already there: run to its centre, an arm of it
            let plate0 = if track || !self.auto_junctions { None } else { self.plate_at(assets, p0) };
            let plate1 = if track || !self.auto_junctions { None } else { self.plate_at(assets, p1) };
            let a0 = if plate0.is_some() { None } else { self.snap_anchor(assets, p0, 3.0).map(|a| a.0).filter(mid_ok) };
            let a1 = if plate1.is_some() { None } else { self.snap_anchor(assets, p1, 3.0).map(|a| a.0).filter(|a| a.spline.map(|s| Some(s) != a0.and_then(|b| b.spline)).unwrap_or(true)).filter(mid_ok) };
            if let Some(p) = plate0 {
                if let Some(sg) = roads::straight(p.1.truncate(), p0) {
                    segs.insert(0, sg);
                }
            }
            if let Some(p) = plate1 {
                segs.extend(roads::straight(p1, p.1.truncate()));
            }
            // a track branching off another one or joining it: a switch there (Auto-Switch),
            // the new track starting from (or running into) its diverging heel
            let mut plans: Vec<crate::switches::Plan> = Vec::new();
            let mut track_mids: Vec<((i32, i32), i64, f64, f64)> = Vec::new();
            let mut start_heel: Option<Anchor> = None;
            let mut end_heel: Option<Anchor> = None;
            if track {
                if let Some(a) = a0.filter(|a| a.mid.is_some()) {
                    let m = a.mid.unwrap();
                    if let Some(h) = self.heading_in(m) {
                        let towards = segs[0].curve().point_at(segs[0].length.min(10.0)).truncate() - a.pos.truncate();
                        let travel = if roads::dir(h).dot(towards) >= 0.0 { h } else { roads::norm_deg(h + 180.0) };
                        let left = towards.dot(roads::right(travel)) < 0.0;
                        match self.switch_plan(assets, a.pos, travel, left) {
                            Some((plan, (pos, heading))) => {
                                start_heel = Some(Anchor { pos, heading, spline: None, mid: None });
                                plans.push(plan);
                            }
                            None => track_mids.push(m),
                        }
                    }
                }
                if let Some(a) = a1.filter(|a| a.mid.is_some()) {
                    let m = a.mid.unwrap();
                    if let Some(h) = self.heading_in(m) {
                        let last = segs.last().unwrap();
                        let from = last.curve().point_at((last.length - 10.0).max(0.0)).truncate() - a.pos.truncate();
                        let open = if roads::dir(h).dot(from) >= 0.0 { h } else { roads::norm_deg(h + 180.0) };
                        let left = from.dot(roads::right(open)) < 0.0;
                        match self.switch_plan(assets, a.pos, open, left) {
                            Some((plan, (pos, heading))) => {
                                end_heel = Some(Anchor { pos, heading, spline: None, mid: None });
                                plans.push(plan);
                            }
                            None => track_mids.push(m),
                        }
                    }
                }
                // the OSM line between the switch and where it parts from the old track gives
                // way to a curve from the heel
                if start_heel.is_some() || end_heel.is_some() {
                    let mut pts: Vec<DVec2> = r.points.clone();
                    if let (Some(h), Some(a)) = (start_heel, a0) {
                        let reach = (h.pos.truncate() - a.pos.truncate()).length() + 4.0;
                        while pts.len() > 2 && (pts[0] - a.pos.truncate()).length() < reach {
                            pts.remove(0);
                        }
                    }
                    if let (Some(h), Some(a)) = (end_heel, a1) {
                        let reach = (h.pos.truncate() - a.pos.truncate()).length() + 4.0;
                        while pts.len() > 2 && (pts[pts.len() - 1] - a.pos.truncate()).length() < reach {
                            pts.pop();
                        }
                    }
                    let mut mid_segs = crate::osm::segments(&pts, r.class);
                    if mid_segs.is_empty() {
                        mid_segs = roads::straight(pts[0], pts[pts.len() - 1]).into_iter().collect();
                    }
                    let mut all = Vec::new();
                    if let Some(h) = start_heel {
                        all.extend(roads::plan(&h, true, mid_segs[0].start, None).unwrap_or_default());
                    }
                    all.extend(mid_segs);
                    if let Some(h) = end_heel {
                        let last = *all.last().unwrap();
                        let from = Anchor { pos: DVec3::new(last.end().x, last.end().y, 0.0), heading: last.curve().heading_at(last.length), spline: None, mid: None };
                        all.extend(roads::plan(&from, true, h.pos.truncate(), Some(&h)).unwrap_or_default());
                    }
                    if !all.is_empty() {
                        segs = all;
                    }
                }
            }
            // (into a road's middle: from its centre line exactly)
            if let Some(a) = a0.filter(|a| a.mid.is_some() && !(track && start_heel.is_some())) {
                if let Some(s) = roads::straight(a.pos.truncate(), segs[0].start) {
                    segs.insert(0, s);
                }
            }
            if let Some(a) = a1.filter(|a| a.mid.is_some() && !(track && end_heel.is_some())) {
                let e = segs.last().unwrap().end();
                segs.extend(roads::straight(e, a.pos.truncate()));
            }
            let h0 = segs[0].heading;
            let h1 = segs.last().map(|s| s.curve().heading_at(s.length)).unwrap_or(0.0);
            // a road end met in line with it: going on from it (else beside it, unjoined)
            let in_line = |a: f64, b: f64| (roads::norm_deg(a - b + 180.0) - 180.0).abs() < 25.0;
            let start_link = match a0 {
                Some(a) if a.mid.is_none() && in_line(a.heading, h0) => a,
                _ => Anchor { pos: DVec3::new(segs[0].start.x, segs[0].start.y, 0.0), heading: h0, ..Default::default() },
            };
            let end_link = a1.filter(|a| a.mid.is_none() && in_line(a.heading + 180.0, h1));
            let auto_terrain = self.auto_terrain;
            let Some(doc) = self.doc.as_ref() else { break };
            let q0 = segs[0].start;
            let q1 = segs.last().unwrap().end();
            let z0 = start_heel.map(|h| h.pos.z).or(plate0.map(|p| p.1.z)).or(a0.map(|a| a.pos.z)).unwrap_or_else(|| doc.ground(q0));
            let z1 = end_heel.map(|h| h.pos.z).or(plate1.map(|p| p.1.z)).or(a1.map(|a| a.pos.z)).unwrap_or_else(|| if auto_terrain { doc.ground(q1) } else { z0 });
            if segs.iter().any(|s| !doc.tiles.contains_key(&tile_of(s.start)) || !doc.tiles.contains_key(&tile_of(s.end()))) {
                continue;
            }
            let pieces = roads::split(&segs, 50.0);
            let Some(made) = self.lay_road(&rel, &pieces, z0, z1, start_link, end_link, half) else { continue };
            built += 1;
            ends_made.push(made.first().map(|m| (m.0, m.1, false)));
            ends_made.push(made.last().map(|m| (m.0, m.1, true)));
            if track {
                // the switches laid, the old track taken out under them; or the old track cut
                // where the new one meets it
                if let Some(doc) = self.doc.as_mut() {
                    for (k, id, at, _) in track_mids {
                        doc.split_spline(k, id, at);
                    }
                    for p in &plans {
                        let Some(laid) = crate::switches::lay(doc, p) else { continue };
                        if let (Some(r), Some(shape)) = (laid.rejoin, assets.object(&p.rel).and_then(|m| crate::switches::Shape::of(&m))) {
                            let heel = shape.world(shape.straight, p.toe, p.heading);
                            rejoin_track(doc, r, heel);
                        }
                        switches += 1;
                    }
                }
            } else if self.auto_junctions {
                let start_mid = a0.and_then(|a| a.mid.map(|m| (m, a.pos)));
                let end_mid = a1.and_then(|a| a.mid.map(|m| (m, a.pos)));
                junctions += self.make_junctions(assets, &made, half, start_mid, end_mid);
                // into a plate there before: cut back to its edge and joined to it
                for (plate, end, at_end) in [(plate0, made.first().copied(), false), (plate1, made.last().copied(), true)] {
                    let (Some((jid, _, r)), Some((k, sid))) = (plate, end) else { continue };
                    let Some(doc) = self.doc.as_mut() else { break };
                    if let Some(arm) = doc.trim(k, sid, at_end, r) {
                        self.pending_arms.entry(jid).or_default().push(crate::junction::Arm { key: arm.0, id: arm.1, at_end: arm.2 });
                        self.rebuild_junction(assets, jid);
                        joined_plates += 1;
                    }
                }
            }
        }
        // where streets end at one node (OSM splits its ways at crossings): a junction there
        if self.auto_junctions {
            junctions += self.junctions_at_nodes(assets, &ends_made.into_iter().flatten().collect::<Vec<_>>());
        }
        if switches > 0 || joined_plates > 0 {
            log::info!("osm: {switches} switches, {joined_plates} roads joined to junctions there before");
        }
        self.road_type = was;
        self.finish_step();
        log::info!("osm: {built} of {} roads built, {junctions} junctions", list.len());
        (built, junctions)
    }

    /// Junctions where three or more road ends meet (or two at an angle), as OpenStreetMap's
    /// ways end at the crossings it splits them at: each end cut back and the plate made in
    /// the middle. `ends` are the ends of the roads just built; the free ends of others near
    /// them count too. How many junctions were made.
    fn junctions_at_nodes(&mut self, assets: &mut Assets, ends: &[(TileKey, i64, bool)]) -> usize {
        let (Some(content), asphalt) = (self.content_dir.clone(), self.asphalt.clone()) else { return 0 };
        let styles = self.road_styles.clone();
        let follow = self.follow_ground;
        let mut made = 0;
        // every free end of a road one sees and joins (tracks meet in switches)
        let free = |doc: &Document, assets: &mut Assets| -> Vec<(TileKey, i64, bool, DVec3, f64, f64)> {
            let mut v = Vec::new();
            for (k, t) in &doc.tiles {
                for sp in t.tile.splines.iter().filter(|s| !s.deleted) {
                    let Some(m) = assets.spline(&sp.file) else { continue };
                    if !m.joinable() || m.is_track() {
                        continue;
                    }
                    let c = omsi_geometry::SplineCurve::from_map(sp, tile_origin(*k));
                    if sp.prev_id == 0 {
                        v.push((*k, sp.id, false, c.point_at(0.0), c.heading_at(0.0), m.half_width as f64));
                    }
                    if sp.next_id == 0 {
                        v.push((*k, sp.id, true, c.end_point(), roads::norm_deg(c.heading_at(sp.length) + 180.0), m.half_width as f64));
                    }
                }
            }
            v
        };
        let Some(doc) = self.doc.as_ref() else { return 0 };
        let all = free(doc, assets);
        let seeds: Vec<DVec2> = ends.iter().filter_map(|(k, id, at_end)| all.iter().find(|e| e.0 == *k && e.1 == *id && e.2 == *at_end).map(|e| e.3.truncate())).collect();
        let mut used = vec![false; all.len()];
        let mut clusters: Vec<Vec<usize>> = Vec::new();
        for s in seeds {
            let near: Vec<usize> = (0..all.len()).filter(|&i| !used[i] && (all[i].3.truncate() - s).length() < 3.0).collect();
            if near.len() < 2 {
                continue;
            }
            for &i in &near {
                used[i] = true;
            }
            clusters.push(near);
        }
        for c in clusters {
            // (two ends in line are one road going on: they are joined, not made a junction)
            let headings: Vec<f64> = c.iter().map(|&i| all[i].4).collect();
            let sharpest = headings.iter().enumerate().flat_map(|(i, a)| headings.iter().skip(i + 1).map(move |b| (roads::norm_deg(a - b + 180.0) - 180.0).abs())).fold(180.0f64, f64::min);
            let centre = c.iter().map(|&i| all[i].3).sum::<DVec3>() / c.len() as f64;
            if self.plate_at(assets, centre.truncate()).is_some() {
                continue;
            }
            let halves: Vec<f64> = c.iter().map(|&i| all[i].5).collect();
            let corner = c.iter().filter_map(|&i| self.doc.as_ref().and_then(|d| d.spline(all[i].0, all[i].1)).map(|s| styles.get(&s.file).corner)).fold(1.0f64, f64::max).clamp(0.5, 3.0);
            let r = crate::junction::radius_at(&halves, sharpest.max(20.0)) * corner;
            let Some(doc) = self.doc.as_mut() else { break };
            if c.len() == 2 {
                // two streets at a corner: both cut back and joined by a bend (in line they go
                // on as they are; folded back on each other they stay apart)
                if (30.0..165.0).contains(&sharpest) {
                    let back = halves.iter().cloned().fold(3.0, f64::max) + 2.0;
                    let arms: Vec<crate::junction::Arm> = c.iter().filter_map(|&i| doc.trim(all[i].0, all[i].1, all[i].2, back)).map(|x| crate::junction::Arm { key: x.0, id: x.1, at_end: x.2 }).collect();
                    if arms.len() == 2 {
                        join_arms(doc, &arms[0], &arms[1]);
                    }
                }
                continue;
            }
            let arms: Vec<crate::junction::Arm> = c.iter().filter_map(|&i| doc.trim(all[i].0, all[i].1, all[i].2, r)).map(|x| crate::junction::Arm { key: x.0, id: x.1, at_end: x.2 }).collect();
            if arms.len() < 3 {
                continue;
            }
            let opts = style_options(doc, &styles, &arms, centre);
            match crate::junction::make_with(doc, assets, &content, asphalt.as_deref(), centre, &arms, &opts) {
                Ok(_) => {
                    made += 1;
                    if follow {
                        let e = arm_ends_world(doc, &arms);
                        conform_disc(doc, centre, r * 1.1, &e);
                    }
                }
                Err(e) => log::warn!("junction at a node: {e}"),
            }
        }
        log::info!("osm: {made} junctions where streets end at one node");
        made
    }

    /// Cut the roads where a newly built road `made` crosses one or meets one's middle, and
    /// fill each meeting point with a junction object.
    fn make_junctions(&mut self, assets: &mut Assets, made: &[(TileKey, i64)], half: f64, start_mid: Option<(((i32, i32), i64, f64, f64), DVec3)>, end_mid: Option<(((i32, i32), i64, f64, f64), DVec3)>) -> usize {
        let (Some(content), asphalt) = (self.content_dir.clone(), self.asphalt.clone()) else { return 0 };
        let follow = self.follow_ground;
        let styles = self.road_styles.clone();
        let mine_style = self.road_type.as_deref().map(|r| styles.get(r)).unwrap_or_default();
        let Some(doc) = self.doc.as_mut() else { return 0 };
        let mut count = 0;
        // the new road's splines (their IDCodes change as they are cut)
        let mut mine: Vec<i64> = made.iter().map(|m| m.1).collect();
        let first = made.first().copied();
        let last = made.last().copied();
        // the ends that meet a road's middle (T junctions), first: their roads are cut there
        for (mid, end, pos) in [(end_mid, last, true), (start_mid, first, false)].into_iter().filter_map(|(m, e, at_end)| Some((m?, e?, at_end))) {
            let (((k, id, at, hw), centre), (nk, nid), at_end) = (mid, end, pos);
            // (a road run in at a slant needs more room for its sharp corner)
            let angle = {
                let h1 = doc.spline(k, id).map(|s| omsi_geometry::SplineCurve::from_map(s, tile_origin(k)).heading_at(at)).unwrap_or(0.0);
                let h2 = doc.spline(nk, nid).map(|s| { let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(nk)); c.heading_at(if at_end { s.length } else { 0.0 }) }).unwrap_or(h1 + 90.0);
                h1 - h2
            };
            let other = doc.spline(k, id).map(|s| styles.get(&s.file)).unwrap_or_default();
            let r = crate::junction::radius_at(&[hw, half], angle) * mine_style.corner.max(other.corner).clamp(0.5, 3.0);
            let (p, q) = doc.cut_at(k, id, at);
            let arms = [p.and_then(|p| doc.trim(p.0, p.1, p.2, r)), q.and_then(|q| doc.trim(q.0, q.1, q.2, r)), doc.trim(nk, nid, at_end, r)];
            log::debug!("T junction at {at:.1} on {id} (r {r:.1}): arms {:?}", arms);
            for x in arms.iter().flatten() {
                if let Some(sp) = doc.spline(x.0, x.1) {
                    let c = omsi_geometry::SplineCurve::from_map(sp, tile_origin(x.0));
                    let e = if x.2 { c.end_point() } else { c.point_at(0.0) };
                    log::debug!("  arm spline {} ({}): junction end at ({:.1}, {:.1}) = {:.1} m from the centre", x.1, sp.file, e.x, e.y, (e.truncate() - centre.truncate()).length());
                }
            }
            if let Some(c) = arms[2] {
                mine.push(c.1);
            }
            let arms: Vec<crate::junction::Arm> = arms.iter().flatten().map(|x| crate::junction::Arm { key: x.0, id: x.1, at_end: x.2 }).collect();
            if arms.len() >= 3 {
                let opts = style_options(doc, &styles, &arms, centre);
                match crate::junction::make_with(doc, assets, &content, asphalt.as_deref(), centre, &arms, &opts) {
                    Ok(_) => {
                        count += 1;
                        if follow {
                            let ends = arm_ends_world(doc, &arms);
                            conform_disc(doc, centre, r * 1.1, &ends);
                        }
                    }
                    Err(e) => log::warn!("junction: {e}"),
                }
            } else if arms.len() == 2 {
                join_arms(doc, &arms[0], &arms[1]);
            }
        }
        // where the new road crosses another one (X junctions), one after the other
        let mut skip: Vec<(i64, i64)> = Vec::new();
        for _ in 0..24 {
            let ours: Vec<(TileKey, MapSpline)> = doc.tiles.iter().flat_map(|(k, t)| t.tile.splines.iter().filter(|s| !s.deleted && mine.contains(&s.id)).map(move |s| (*k, s.clone()))).collect();
            let mut found = None;
            'search: for (k1, s1) in &ours {
                let c1 = omsi_geometry::SplineCurve::from_map(s1, tile_origin(*k1));
                let p1 = crate::junction::polyline(&c1);
                for (k2, t) in &doc.tiles {
                    for s2 in t.tile.splines.iter().filter(|s| !s.deleted && !ours.iter().any(|o| o.1.id == s.id)) {
                        if skip.contains(&(s1.id, s2.id)) {
                            continue;
                        }
                        let org = tile_origin(*k2);
                        let start = DVec2::new(org.x + s2.pos[0], org.y + s2.pos[1]);
                        if (start - c1.start.truncate()).length() > s1.length + s2.length + 10.0 {
                            continue;
                        }
                        let Some(m) = assets.spline(&s2.file) else { continue };
                        // (a road over a track is a level crossing: both go on as they are; so is a
                        // road whose style makes no junctions)
                        if !m.joinable() || m.is_track() || !mine_style.junctions || !styles.get(&s2.file).junctions {
                            continue;
                        }
                        let c2 = omsi_geometry::SplineCurve::from_map(s2, org);
                        let linked = s1.prev_id == s2.id || s1.next_id == s2.id || s2.prev_id == s1.id || s2.next_id == s1.id;
                        let joints: Vec<DVec2> = [c1.point_at(0.0), c1.end_point()].into_iter().map(|p| p.truncate()).filter(|p| [c2.point_at(0.0), c2.end_point()].iter().any(|q| (q.truncate() - *p).length() < 1.0)).collect();
                        log::trace!("candidate {} ({:.0},{:.0}) x {} ({:.0},{:.0})", s1.id, c1.start.x, c1.start.y, s2.id, c2.start.x, c2.start.y);
                        for (pt, a1, a2) in crate::junction::crossings(&p1, &crate::junction::polyline(&c2)) {
                            let (z1, z2) = (c1.point_at(a1).z, c2.point_at(a2).z);
                            log::debug!("crossing {} x {} at ({:.1}, {:.1}): z {z1:.2} / {z2:.2}, {a2:.1} of {:.1}", s1.id, s2.id, pt.x, pt.y, s2.length);
                            // (well above or below: a bridge, not a junction; and not at a
                            // spline's very end, where roads are only linked)
                            if (z1 - z2).abs() > 2.5 {
                                skip.push((s1.id, s2.id));
                                continue;
                            }
                            if linked && joints.iter().any(|j| (*j - pt).length() < 1.5) {
                                continue;
                            }
                            found = Some((*k1, s1.id, a1, *k2, s2.id, a2, DVec3::new(pt.x, pt.y, z2), m.half_width as f64));
                            break 'search;
                        }
                    }
                }
            }
            log::debug!("junctions: {} splines of the new road, found {:?}", ours.len(), found.map(|f| (f.1, f.4)));
            let Some((k1, id1, a1, k2, id2, a2, centre, hw2)) = found else { break };
            // (a sharp crossing needs more room for its corners)
            let angle = {
                let h1 = doc.spline(k1, id1).map(|s| omsi_geometry::SplineCurve::from_map(s, tile_origin(k1)).heading_at(a1)).unwrap_or(0.0);
                let h2 = doc.spline(k2, id2).map(|s| omsi_geometry::SplineCurve::from_map(s, tile_origin(k2)).heading_at(a2)).unwrap_or(90.0);
                h1 - h2
            };
            let other = doc.spline(k2, id2).map(|s| styles.get(&s.file)).unwrap_or_default();
            let r = crate::junction::radius_at(&[half, hw2], angle) * mine_style.corner.max(other.corner).clamp(0.5, 3.0);
            // both roads cut there (at a joint, the splines meeting there are the sides)
            let (p1, q1) = doc.cut_at(k1, id1, a1);
            for x in [p1, q1].iter().flatten() {
                mine.push(x.1);
            }
            let (p2, q2) = doc.cut_at(k2, id2, a2);
            let arms = [p1.and_then(|x| doc.trim(x.0, x.1, x.2, r)), q1.and_then(|x| doc.trim(x.0, x.1, x.2, r)), p2.and_then(|x| doc.trim(x.0, x.1, x.2, r)), q2.and_then(|x| doc.trim(x.0, x.1, x.2, r))];
            for a in arms.iter().take(2).flatten() {
                mine.push(a.1);
            }
            skip.push((id1, id2));
            let arms: Vec<crate::junction::Arm> = arms.iter().flatten().map(|x| crate::junction::Arm { key: x.0, id: x.1, at_end: x.2 }).collect();
            if arms.len() >= 3 {
                let opts = style_options(doc, &styles, &arms, centre);
                match crate::junction::make_with(doc, assets, &content, asphalt.as_deref(), centre, &arms, &opts) {
                    Ok(_) => {
                        count += 1;
                        if follow {
                            let ends = arm_ends_world(doc, &arms);
                            conform_disc(doc, centre, r * 1.1, &ends);
                        }
                    }
                    Err(e) => log::warn!("junction: {e}"),
                }
            } else if arms.len() == 2 {
                // (the rest were stubs too short to keep: a bend, not a junction)
                join_arms(doc, &arms[0], &arms[1]);
            }
        }
        // a junction made before whose arm road this one trimmed away (a short road between
        // the two): made again for the roads it still holds, or a joint of two
        for id in self.orphaned_junctions(assets) {
            log::debug!("junction {id} lost an arm: made again");
            self.rebuild_junction(assets, id);
        }
        count
    }

    /// openMapper's junctions with an arm no road ends at any more.
    fn orphaned_junctions(&self, assets: &mut Assets) -> Vec<i64> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let mut out = Vec::new();
        for (k, t) in &doc.tiles {
            for o in t.tile.objects.iter().filter(|o| is_own_junction(&o.file)) {
                let Some((abs, helpers)) = self.content_dir.as_deref().and_then(|c| plate_helpers(c, assets, &o.file)) else { continue };
                let pts = selection::helper_points(doc, *k, o.id, abs, &helpers);
                // (a helper for each way along an arm: one place per arm)
                let mut places: Vec<DVec2> = Vec::new();
                for (p, _) in &pts {
                    if !places.iter().any(|q| (*q - p.truncate()).length() < 1.5) {
                        places.push(p.truncate());
                    }
                }
                let held = places.iter().filter(|q| pts.iter().filter(|(p, _)| (p.truncate() - **q).length() < 1.5).any(|(p, _)| selection::free_end_at(doc, p.truncate()).is_some())).count();
                if held < places.len() {
                    out.push(o.id);
                }
            }
        }
        out
    }

    // ---------------------------------------------------------------- terrain and paint

    fn brush_terrain(&mut self, assets: &mut Assets, c: DVec2, dt: f64) {
        let Some(doc) = self.doc.as_mut() else { return };
        let r = self.brush_radius as f64;
        // the roads near the brush: the ground is not raised up through them
        let roads = road_points_near(doc, assets, c, r + 15.0);
        let strength = self.brush_strength as f64;
        let mode = self.terrain_mode;
        let flat = self.flatten_to.unwrap_or(0.0) as f64;
        let cell = tile_size() / (TERRAIN_SAMPLES - 1) as f64;
        let n = (TERRAIN_SAMPLES - 1) as i64;
        let (gx0, gx1) = (((c.x - r) / cell).floor() as i64, ((c.x + r) / cell).ceil() as i64);
        let (gy0, gy1) = (((c.y - r) / cell).floor() as i64, ((c.y + r) / cell).ceil() as i64);
        // read first (smoothing needs the neighbours as they were)
        let mut changes: Vec<(i64, i64, f32)> = Vec::new();
        for gy in gy0..=gy1 {
            for gx in gx0..=gx1 {
                let p = DVec2::new(gx as f64 * cell, gy as f64 * cell);
                let d = (p - c).length();
                if d > r {
                    continue;
                }
                let w = {
                    let x = 1.0 - (d / r) * (d / r);
                    x * x
                };
                let Some(h) = world_sample(doc, gx, gy, n) else { continue };
                let h = h as f64;
                let nh = match mode {
                    TerrainMode::Raise => h + strength * 12.0 * dt * w,
                    TerrainMode::Lower => h - strength * 12.0 * dt * w,
                    TerrainMode::Smooth => {
                        let mut s = 0.0;
                        let mut k = 0.0;
                        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                            if let Some(v) = world_sample(doc, gx + dx, gy + dy, n) {
                                s += v as f64;
                                k += 1.0;
                            }
                        }
                        if k > 0.0 { h + (s / k - h) * (strength * 6.0 * dt * w).min(1.0) } else { h }
                    }
                    TerrainMode::Flatten => h + (flat - h) * (strength * 8.0 * dt * w).min(1.0),
                    // (towards a fixed pattern of rolls, the same wherever the brush goes, so
                    // holding it longer shapes them more and does not make noise of noise)
                    TerrainMode::Roughen => h + rolls(p) * strength * 3.0 * dt * w,
                    // (clicked from end to end, not brushed)
                    TerrainMode::Ramp => h,
                    // (dug towards the bed, never filled; the banks ease out with the brush's rim)
                    TerrainMode::Water => {
                        let bed = self.water_level.unwrap_or(0.0) as f64 - self.water_depth as f64;
                        if h > bed { h + (bed - h) * (strength * 8.0 * dt * w).min(1.0) } else { h }
                    }
                };
                changes.push((gx, gy, nh as f32));
            }
        }
        for (gx, gy, h) in changes {
            let p = DVec2::new(gx as f64 * cell, gy as f64 * cell);
            let h = match ground_cap(&roads, p, cell) {
                // (never above the road; ground that already was stays as it is)
                Some(cap) => (h as f64).min(cap.max(world_sample(doc, gx, gy, n).unwrap_or(h) as f64)) as f32,
                None => h,
            };
            set_world_sample(doc, &mut self.history, gx, gy, n, h);
        }
        if mode == TerrainMode::Water {
            if let Some(level) = self.water_level {
                // (the tiles under the brush that have no water get it, at the lake's level)
                let mut keys = std::collections::BTreeSet::new();
                for p in [c, c + DVec2::new(r, 0.0), c - DVec2::new(r, 0.0), c + DVec2::new(0.0, r), c - DVec2::new(0.0, r)] {
                    keys.insert(tile_of(p));
                }
                for k in keys {
                    if doc.tiles.get(&k).map(|t| t.water.is_none()).unwrap_or(false) {
                        self.history.touch_tile(doc, k);
                        if let Some(t) = doc.tiles.get_mut(&k) {
                            t.water = Some([level; 4]);
                            t.touch_content();
                        }
                    }
                }
            }
        }
    }

    /// The ground from `a` to `b` made one even slope from the height at `a` to that at
    /// `b`, as wide as the brush (fully within 60 % of its radius, easing out to its rim);
    /// one undo step. Its gradient (%).
    pub fn ramp(&mut self, a: DVec3, b: DVec3) -> f64 {
        let Some(doc) = self.doc.as_mut() else { return 0.0 };
        let r = self.brush_radius as f64;
        let core = r * 0.6;
        let (a2, b2) = (a.truncate(), b.truncate());
        let len = (b2 - a2).length();
        let cell = tile_size() / (TERRAIN_SAMPLES - 1) as f64;
        let n = (TERRAIN_SAMPLES - 1) as i64;
        let (lo, hi) = (a2.min(b2) - DVec2::splat(r), a2.max(b2) + DVec2::splat(r));
        self.history.begin(tr("Ramp", "Пандус"));
        let mut changes: Vec<(i64, i64, f32)> = Vec::new();
        for gy in (lo.y / cell).floor() as i64..=(hi.y / cell).ceil() as i64 {
            for gx in (lo.x / cell).floor() as i64..=(hi.x / cell).ceil() as i64 {
                let p = DVec2::new(gx as f64 * cell, gy as f64 * cell);
                let t = if len > 1e-6 { ((p - a2).dot(b2 - a2) / (len * len)).clamp(0.0, 1.0) } else { 0.0 };
                let d = (p - (a2 + (b2 - a2) * t)).length();
                if d > r {
                    continue;
                }
                let Some(h) = world_sample(doc, gx, gy, n) else { continue };
                let target = a.z + (b.z - a.z) * t;
                let w = if d <= core {
                    1.0
                } else {
                    let x = 1.0 - (d - core) / (r - core);
                    x * x * (3.0 - 2.0 * x)
                };
                changes.push((gx, gy, (h as f64 + (target - h as f64) * w) as f32));
            }
        }
        for (gx, gy, h) in changes {
            set_world_sample(doc, &mut self.history, gx, gy, n, h);
        }
        self.history.commit();
        if len > 1e-6 { (b.z - a.z) / len * 100.0 } else { 0.0 }
    }

    /// The plant brush held over `c` for `dt` seconds.
    fn brush_plants(&mut self, assets: &mut Assets, c: DVec2, dt: f64) {
        let mix: Vec<String> = if self.plant_mix.is_empty() { self.object_type.iter().cloned().collect() } else { self.plant_mix.clone() };
        let r = self.brush_radius as f64;
        let Some(doc) = self.doc.as_mut() else { return };
        if self.plant_erase {
            crate::plants::clear(doc, &mut self.history, assets, c, r, &mix);
            return;
        }
        if mix.is_empty() {
            self.toast(tr("Pick a tree in the catalogue (or add some to the mix) first", "Сначала выберите дерево в каталоге (или добавьте в смесь)"));
            return;
        }
        // (about as many tries a second as the circle holds plants: it fills in a moment)
        let room = std::f64::consts::PI * r * r / (self.plant_spacing * self.plant_spacing);
        let tries = (room * dt * 3.0).ceil().clamp(1.0, 200.0) as usize;
        crate::plants::sow(doc, &mut self.history, assets, c, r, self.plant_spacing, &mix, &mut self.plant_rng, tries);
    }

    fn brush_paint(&mut self, c: DVec2, dt: f64) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.sync_mask_slots();
        let layers = doc.global.ground_textures.len();
        if layers < 2 && self.paint_layer > 0 {
            return;
        }
        let r = self.brush_radius as f64;
        let k = (self.brush_strength as f64 * 6.0 * dt).min(1.0);
        let layer = self.paint_layer.min(layers.saturating_sub(1));
        let erase = self.paint_erase;
        let ts = tile_size();
        let t0 = tile_of(c - DVec2::splat(r));
        let t1 = tile_of(c + DVec2::splat(r));
        for ty in t0.1..=t1.1 {
            for tx in t0.0..=t1.0 {
                let key = (tx, ty);
                if !doc.tiles.contains_key(&key) {
                    continue;
                }
                self.history.touch_tile(doc, key);
                let org = tile_origin(key);
                let sizes: Vec<usize> = doc.global.ground_textures.iter().map(crate::doc::mask_size).collect();
                let t = doc.tiles.get_mut(&key).unwrap();
                let mut touched = false;
                for li in 1..layers {
                    // the layer painted goes up; the ones above it go down so it shows
                    let up = li == layer && !erase;
                    let down = (li == layer && erase) || (li > layer && !erase);
                    if !up && !down {
                        continue;
                    }
                    let size = sizes[li];
                    let texel = ts / size as f64;
                    let (x0, x1) = ((((c.x - r - org.x) / texel).floor()).max(0.0) as usize, (((c.x + r - org.x) / texel).ceil()).min(size as f64) as usize);
                    let (y0w, y1w) = ((c.y - r - org.y) / texel, (c.y + r - org.y) / texel);
                    if x1 <= x0 {
                        continue;
                    }
                    let mask = t.masks[li].get_or_insert_with(|| vec![0u8; size * size]);
                    if mask.len() != size * size {
                        *mask = vec![0u8; size * size];
                    }
                    for yi in (y0w.floor().max(0.0) as usize)..(y1w.ceil().min(size as f64) as usize) {
                        for xi in x0..x1 {
                            let p = DVec2::new(org.x + (xi as f64 + 0.5) * texel, org.y + (yi as f64 + 0.5) * texel);
                            let d = (p - c).length();
                            if d > r {
                                continue;
                            }
                            let w = (1.0 - d / r).clamp(0.0, 1.0).powf(0.7) * k;
                            // (row 0 is the north edge)
                            let row = size - 1 - yi;
                            let a = &mut mask[row * size + xi];
                            let v = *a as f64;
                            let nv = if up { v + (255.0 - v) * w } else { v - v * w };
                            *a = nv.round().clamp(0.0, 255.0) as u8;
                            touched = true;
                        }
                    }
                }
                // painting the base ground: everything above it goes down (done above when
                // layer == 0: every layer counts as "above")
                if touched {
                    t.touch_masks();
                }
            }
        }
    }

    // ---------------------------------------------------------------- tiles

    fn tile_click(&mut self) {
        let Some(k) = self.hover_tile else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        if doc.tiles.contains_key(&k) {
            self.picked_tile = Some(k);
            self.toast(tr("Right-click a tile to remove it", "Правый клик по тайлу: удалить его"));
            return;
        }
        let near = doc.tiles.is_empty() || [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|(dx, dy)| doc.tiles.contains_key(&(k.0 + dx, k.1 + dy)));
        if !near {
            self.error(tr("New tiles go next to the map", "Новые тайлы добавляются рядом с картой"));
            return;
        }
        self.history.begin(tr("Add tile", "Добавление тайла"));
        self.history.touch_tile(doc, k);
        self.history.touch_global(doc);
        doc.add_tile(k);
        self.history.commit();
        self.picked_tile = Some(k);
    }

    /// A tile's water: on at a height (all four corners), or off. Kept open as one step
    /// while the number is dragged.
    pub fn set_water(&mut self, k: TileKey, level: Option<f32>) {
        self.begin_panel_step(tr("Water", "Вода"));
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.touch_tile(doc, k);
        if let Some(t) = doc.tiles.get_mut(&k) {
            t.water = level.map(|h| [h; 4]);
            t.touch_content();
        }
    }

    /// A ground layer's paint detail (its masks `2^p` texels a side), every tile's mask of it
    /// made again at that size; one undo step.
    pub fn set_paint_detail(&mut self, layer: usize, p: f32) {
        let Some(doc) = self.doc.as_mut() else { return };
        if layer == 0 || layer >= doc.global.ground_textures.len() {
            return;
        }
        let from = crate::doc::mask_size(&doc.global.ground_textures[layer]);
        self.history.begin(tr("Paint detail", "Детальность покраски"));
        self.history.touch_global(doc);
        doc.global.ground_textures[layer].params[0] = p;
        let to = crate::doc::mask_size(&doc.global.ground_textures[layer]);
        let keys: Vec<TileKey> = doc.tiles.keys().copied().collect();
        for k in keys {
            if doc.tiles[&k].masks.get(layer).map(|m| m.is_some()).unwrap_or(false) {
                self.history.touch_tile(doc, k);
                let t = doc.tiles.get_mut(&k).unwrap();
                if let Some(m) = t.masks[layer].as_mut() {
                    *m = crate::doc::resample_mask(m, from, to);
                }
                t.touch_masks();
            }
        }
        doc.global_dirty = true;
        doc.global_rev = crate::doc::next_rev();
        self.history.commit();
    }

    /// A painted ground layer moved one place on (`over`: after the next, drawn over it) or
    /// back, its painting on every tile going with it; the base ground stays first. One undo
    /// step.
    pub fn move_ground_layer(&mut self, layer: usize, over: bool) {
        let Some(doc) = self.doc.as_mut() else { return };
        let other = if over { layer + 1 } else { layer.wrapping_sub(1) };
        let n = doc.global.ground_textures.len();
        if layer == 0 || other == 0 || layer >= n || other >= n {
            return;
        }
        self.history.begin(tr("Ground layer order", "Порядок слоёв земли"));
        self.history.touch_global(doc);
        doc.global.ground_textures.swap(layer, other);
        let keys: Vec<TileKey> = doc.tiles.keys().copied().collect();
        for k in keys {
            // (every tile is written again: its mask files are named by the layers' places)
            self.history.touch_tile(doc, k);
            let t = doc.tiles.get_mut(&k).unwrap();
            t.masks.resize(n, None);
            t.masks.swap(layer, other);
            t.touch_masks();
        }
        doc.global_dirty = true;
        doc.global_rev = crate::doc::next_rev();
        self.history.commit();
        self.paint_layer = other;
    }

    /// A ground layer's texture, or its detail texture (the fine grain over it, seen close
    /// up), changed; its painting stays. One undo step.
    pub fn set_ground_texture(&mut self, layer: usize, detail: bool, rel: &str) {
        let Some(doc) = self.doc.as_mut() else { return };
        if layer >= doc.global.ground_textures.len() {
            return;
        }
        self.history.begin(if detail { tr("Detail texture", "Детальная текстура") } else { tr("Ground texture", "Текстура земли") });
        self.history.touch_global(doc);
        let g = &mut doc.global.ground_textures[layer];
        if detail {
            g.detail_texture = rel.to_string();
        } else {
            g.texture = rel.to_string();
        }
        doc.global_dirty = true;
        doc.global_rev = crate::doc::next_rev();
        self.history.commit();
    }

    /// Whether the game lights a tile's ground from the lamps on and round it at night
    /// (`[variable_terrainlightmap]`: it bakes the tile's light map when it loads it).
    pub fn set_tile_lamplight(&mut self, k: TileKey, on: bool) {
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.begin(tr("Lamp light on the ground", "Свет фонарей на земле"));
        self.history.touch_tile(doc, k);
        if let Some(t) = doc.tiles.get_mut(&k) {
            t.tile.variable_terrain_lightmap = on;
            t.touch_content();
        }
        self.history.commit();
    }

    /// A ring of new tiles round the whole map (corners too), in one step. How many.
    pub fn grow_map(&mut self) -> usize {
        let Some(doc) = self.doc.as_mut() else { return 0 };
        let mut new: Vec<TileKey> = Vec::new();
        for k in doc.tiles.keys() {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let n = (k.0 + dx, k.1 + dy);
                    if !doc.tiles.contains_key(&n) && !new.contains(&n) {
                        new.push(n);
                    }
                }
            }
        }
        new.sort();
        self.history.begin(tr("Add tiles", "Добавление тайлов"));
        self.history.touch_global(doc);
        // (side neighbours first: a corner tile takes its edges' heights from them)
        new.sort_by_key(|k| std::cmp::Reverse([(-1, 0), (1, 0), (0, -1), (0, 1)].iter().filter(|(dx, dy)| doc.tiles.contains_key(&(k.0 + dx, k.1 + dy))).count()));
        for k in &new {
            self.history.touch_tile(doc, *k);
            doc.add_tile(*k);
        }
        self.history.commit();
        new.len()
    }

    fn remove_tile(&mut self, k: TileKey) {
        let Some(doc) = self.doc.as_mut() else { return };
        if !doc.tiles.contains_key(&k) {
            return;
        }
        if doc.tiles.len() == 1 {
            self.error(tr("A map needs at least one tile", "На карте должен остаться хотя бы один тайл"));
            return;
        }
        self.history.begin(tr("Remove tile", "Удаление тайла"));
        let keys: Vec<TileKey> = doc.tiles.keys().copied().collect();
        for t in keys {
            self.history.touch_tile(doc, t);
        }
        self.history.touch_global(doc);
        doc.remove_tile(k);
        self.history.commit();
        if self.selection().iter().any(|s| matches!(s, Sel::Object(t, _) | Sel::Spline(t, _) if *t == k)) {
            self.select(None);
        }
    }

    // ---------------------------------------------------------------- drawing

    /// The brush, ghosts and lines this frame shows.
    /// The points junction objects offer roads to join them (their `[splinehelper]`s).
    fn helper_anchors(&self, assets: &mut Assets) -> Vec<DVec3> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let mut out = Vec::new();
        for (k, t) in &doc.tiles {
            for o in &t.tile.objects {
                let Some(model) = assets.object(&o.file) else { continue };
                if model.helpers.is_empty() {
                    continue;
                }
                let world = doc.object_world(*k, o, model.abs_height);
                let m = object_matrix(DVec3::ZERO, o.rot);
                for (h, _) in &model.helpers {
                    out.push(world + m.transform_point3(Vec3::new(h[0], h[1], h[2])).as_dvec3());
                }
            }
        }
        out
    }

    pub fn extras(&self, cam: &crate::camera::Camera, assets: &mut Assets) -> FrameExtras {
        self.view_distance.set(cam.distance);
        let mut x = FrameExtras { grid: self.show_grid, selected: self.selected, selected_more: self.selection().into_iter().skip(1).collect(), hover: self.hover, ..Default::default() };
        let Some(doc) = self.doc.as_ref() else { return x };
        let w = (cam.distance * 0.004).clamp(0.08, 6.0) as f32;
        // (the dots that mark places on the ground are small: they are there to be seen, not to cover)
        let dw = w * 0.4;
        x.show_technical = self.show_technical || self.tool == Tool::Logic;
        x.hide = self.hide;
        // the AI's lanes: arrows the way traffic goes, ends green where joined, red where not
        // (thin and small: a lane is a line on the road, not a road of its own)
        if self.tool == Tool::Logic || self.show_lanes {
            let r = (cam.distance * 1.3).clamp(40.0, 500.0);
            let (lanes, ends) = crate::lanes::near(doc, assets, cam.target.truncate(), r);
            let lw = (cam.distance * 0.0012).clamp(0.03, 1.2) as f32;
            for l in lanes.iter().filter(|l| l.kind != 2) {
                let color = if l.kind == 0 { [1.0, 0.78, 0.2, 0.9] } else { [0.35, 0.8, 1.0, 0.9] };
                let lift = DVec3::Z * 0.15;
                for s in l.pts.windows(2) {
                    line(&mut x.overlay, cam, s[0] + lift, s[1] + lift, lw, color);
                }
                // a chevron every 8 m (both ways for a two-way lane)
                let mut acc = 0.0;
                for s in l.pts.windows(2) {
                    let seg = (s[1] - s[0]).length();
                    acc += seg;
                    // (further apart when seen from far, so they do not merge into a band)
                    if acc < (cam.distance * 0.06).clamp(6.0, 40.0) || seg < 1e-3 {
                        continue;
                    }
                    acc = 0.0;
                    let mid = (s[0] + s[1]) * 0.5 + lift;
                    let d = (s[1] - s[0]).truncate() / seg;
                    for back in [false, true] {
                        if (l.direction == 1) != back && l.direction != 2 {
                            continue;
                        }
                        let f = if back { -d } else { d };
                        let sz = (lw as f64 * 6.0).clamp(0.3, 4.0);
                        let tip = mid + DVec3::new(f.x, f.y, 0.0) * sz;
                        let side = DVec3::new(f.y, -f.x, 0.0) * sz * 0.6;
                        line(&mut x.overlay, cam, tip, mid - side, lw, color);
                        line(&mut x.overlay, cam, tip, mid + side, lw, color);
                    }
                }
            }
            for e in ends.iter().filter(|e| e.kind != 2) {
                disc(&mut x.overlay, cam, e.pos + DVec3::Z * 0.2, lw * 1.2, if e.linked { [0.35, 0.85, 0.45, 0.95] } else { [0.95, 0.25, 0.2, 0.95] });
            }
        }
        // an AI path has nothing to see: the picked and the hovered one drawn as lines
        {
            let editing = self.editing_paths();
            for (sel, color) in self.selection().into_iter().map(|s| (s, ORANGE)).chain(self.hover.map(|h| (h, WHITE))) {
                let Sel::Spline(k, id) = sel else { continue };
                let Some(s) = doc.spline(k, id) else { continue };
                if !editing && !assets.spline(&s.file).map(|m| m.is_ai_path()).unwrap_or(false) {
                    continue;
                }
                let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(k));
                let n = (s.length / 1.0).ceil().max(2.0) as usize;
                let pts: Vec<DVec3> = (0..=n).map(|i| c.point_at(s.length * i as f64 / n as f64) + DVec3::Z * 0.25).collect();
                for p in pts.windows(2) {
                    line(&mut x.overlay, cam, p[0], p[1], w * 0.6, color);
                }
            }
        }
        // a row of car parks being laid out
        if let (Some(a), Some(c), Tool::Logic) = (self.parking_start, self.cursor, self.tool) {
            line(&mut x.overlay, cam, a + DVec3::Z * 0.3, c + DVec3::Z * 0.3, w, ORANGE);
            let d = (c - a).truncate();
            let len = d.length();
            if len > 0.5 {
                let n = (len / self.parking_step).floor() as usize + 1;
                for i in 0..n {
                    let t = if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 };
                    disc(&mut x.overlay, cam, a + (c - a) * t + DVec3::Z * 0.3, dw * 2.5, ORANGE);
                }
            }
        }
        // a ramp being laid out: its middle and its edges, from its start to the cursor
        if let (Some(a), Some(c), true) = (self.ramp_start, self.cursor, self.tool == Tool::Terrain && self.terrain_mode == TerrainMode::Ramp) {
            let d = (c - a).truncate();
            let side = DVec2::new(-d.y, d.x).normalize_or_zero() * self.brush_radius as f64 * 0.6;
            let lift = DVec3::Z * 0.4;
            line(&mut x.overlay, cam, a + lift, c + lift, w, ORANGE);
            for s in [side, -side] {
                let o = DVec3::new(s.x, s.y, 0.0);
                line(&mut x.overlay, cam, a + o + lift, c + o + lift, w * 0.5, [1.0, 1.0, 1.0, 0.6]);
            }
            disc(&mut x.overlay, cam, a + lift, dw * 3.0, ORANGE);
        }
        // GPS tracks to trace, in magenta on the ground
        for t in &self.trace_lines {
            let on = |p: DVec2| DVec3::new(p.x, p.y, doc.ground(p) + 0.4);
            for seg in t.windows(2) {
                let n = ((seg[1] - seg[0]).length() / 6.0).ceil().clamp(1.0, 50.0) as usize;
                for i in 0..n {
                    line(&mut x.overlay, cam, on(seg[0].lerp(seg[1], i as f64 / n as f64)), on(seg[0].lerp(seg[1], (i + 1) as f64 / n as f64)), w * 0.8, [0.95, 0.3, 0.85, 0.9]);
                }
            }
        }
        // a plant outline being clicked round, closed to the cursor
        if self.tool == Tool::Objects && self.plant_brush && self.plant_area {
            let color = if self.plant_erase { [1.0, 0.4, 0.4, 1.0] } else { [0.45, 0.9, 0.45, 1.0] };
            let on = |p: DVec2| DVec3::new(p.x, p.y, doc.ground(p) + 0.3);
            let mut pts: Vec<DVec2> = self.plant_outline.clone();
            if let (Some(c), true) = (self.cursor, self.cursor_in_view) {
                pts.push(c.truncate());
            }
            // (every 4 m on the ground, so the line lies over hills)
            let edge = |x: &mut FrameExtras, a: DVec2, b: DVec2, width: f32, color: [f32; 4]| {
                let n = ((b - a).length() / 4.0).ceil().max(1.0) as usize;
                for i in 0..n {
                    line(&mut x.overlay, cam, on(a.lerp(b, i as f64 / n as f64)), on(a.lerp(b, (i + 1) as f64 / n as f64)), width, color);
                }
            };
            for seg in pts.windows(2) {
                edge(&mut x, seg[0], seg[1], w, color);
            }
            if pts.len() >= 3 {
                edge(&mut x, pts[pts.len() - 1], pts[0], w * 0.6, [color[0], color[1], color[2], 0.5]);
            }
            for (i, p) in self.plant_outline.iter().enumerate() {
                disc(&mut x.overlay, cam, on(*p), dw * if i == 0 { 4.0 } else { 2.5 }, color);
            }
        }
        match self.acts_as() {
            Tool::Objects if self.tool == Tool::Objects && self.plant_brush && self.plant_area => {}
            Tool::Objects if self.tool == Tool::Objects && self.plant_brush && self.cursor_in_view => {
                if let Some(c) = self.cursor {
                    let color = if self.plant_erase { [1.0, 0.4, 0.4, 1.0] } else { [0.45, 0.9, 0.45, 1.0] };
                    x.brush = Some(Brush { centre: c.truncate(), radius: self.brush_radius, color });
                }
            }
            Tool::Terrain | Tool::Paint if self.cursor_in_view => {
                if let Some(c) = self.cursor {
                    let color = match (self.tool, self.terrain_mode, self.paint_erase) {
                        (Tool::Terrain, TerrainMode::Lower | TerrainMode::Water, _) => [0.35, 0.65, 1.0, 1.0],
                        (Tool::Paint, _, true) => [1.0, 0.4, 0.4, 1.0],
                        _ => [1.0, 1.0, 1.0, 1.0],
                    };
                    x.brush = Some(Brush { centre: c.truncate(), radius: self.brush_radius, color });
                }
            }
            Tool::Roads => {
                // the switches the track being drawn lays
                for p in &self.switch_plans {
                    let o = DVec3::new(p.origin.x, p.origin.y, doc.ground(p.origin.truncate()));
                    x.ghosts.push(Ghost { rel: p.rel.clone(), world: o, rot: [p.heading, 0.0, 0.0], tint: [0.35, 0.85, 0.45, 0.4] });
                }
                // a roundabout being laid out: its ring
                if let (Some((c, r)), Some(rel)) = (self.ring_radius(), &self.road_type) {
                    for sg in crate::roundabout::ring(c.truncate(), r, doc.global.left_hand_traffic) {
                        let mut s = piece_spline(rel, &sg, &[(c.z, 0.0), (c.z, 0.0)], 0, DVec2::ZERO);
                        s.pos[2] += 0.05;
                        x.ghost_splines.push((rel.clone(), s, [0.35, 0.85, 0.45, 0.3]));
                    }
                    disc(&mut x.overlay, cam, c, dw * 3.0, GREEN);
                }
                if let Some(rel) = &self.road_type {
                    if let Some((segs, z0, z1, _)) = &self.road_plan {
                        let pieces = roads::split(segs, 50.0);
                        let total = roads::total_length(&pieces).max(0.1);
                        let _ = total;
                        let profile = if self.keep_height || !self.auto_terrain { even_profile(&pieces, *z0, *z1) } else { road_profile(doc, &pieces, *z0, *z1) };
                        for (i, p) in pieces.iter().enumerate() {
                            let mut s = piece_spline(rel, p, &profile, i, DVec2::ZERO);
                            s.pos[2] += 0.02;
                            x.ghost_splines.push((rel.clone(), s, [0.35, 0.85, 0.45, 0.25]));
                        }
                        for p in &pieces {
                            let c = p.curve();
                            let n = (p.length / 2.0).ceil().max(1.0) as usize;
                            for i in 0..n {
                                let a = c.point_at(p.length * i as f64 / n as f64);
                                let b = c.point_at(p.length * (i + 1) as f64 / n as f64);
                                let za = doc.ground(a.truncate()) + 0.3;
                                let zb = doc.ground(b.truncate()) + 0.3;
                                line(&mut x.overlay, cam, DVec3::new(a.x, a.y, za), DVec3::new(b.x, b.y, zb), w * 0.6, [1.0, 1.0, 1.0, 0.5]);
                            }
                        }
                        if total > 1.0 {
                            let _ = total;
                        }
                    }
                }
                if let Some((s, _)) = &self.road_start {
                    disc(&mut x.overlay, cam, s.pos, dw * 3.0, GREEN);
                }
                // every free end, so the user sees where roads connect (not those that
                // already join a junction object)
                let joined = if self.drawing_paths() { Vec::new() } else { self.helper_anchors(assets) };
                let free = |p: DVec3| !joined.iter().any(|j| (j.truncate() - p.truncate()).length() < 0.6);
                for (k, t) in doc.tiles.iter().filter(|_| !self.drawing_paths()) {
                    let org = tile_origin(*k);
                    for s in t.tile.splines.iter().filter(|s| !s.deleted) {
                        let c = omsi_geometry::SplineCurve::from_map(s, org);
                        if s.prev_id == 0 && free(c.point_at(0.0)) {
                            disc(&mut x.overlay, cam, c.point_at(0.0), dw * 2.0, [1.0, 1.0, 1.0, 0.6]);
                        }
                        if s.next_id == 0 && free(c.end_point()) {
                            disc(&mut x.overlay, cam, c.end_point(), dw * 2.0, [1.0, 1.0, 1.0, 0.6]);
                        }
                    }
                }
                if let Some(a) = &self.road_snap {
                    disc(&mut x.overlay, cam, a.pos, dw * 2.5, ORANGE);
                    let d = roads::dir(a.heading);
                    line(&mut x.overlay, cam, a.pos, a.pos + DVec3::new(d.x, d.y, 0.0) * (w as f64 * 25.0), w, ORANGE);
                } else if let (Some(c), true) = (self.cursor, self.cursor_in_view) {
                    disc(&mut x.overlay, cam, c, dw * 2.0, WHITE);
                }
            }
            Tool::Objects => {
                // the points a junction piece or a road can be joined at, near the cursor
                if let (Some(c), true) = (self.cursor, self.cursor_in_view) {
                    for a in self.free_road_ends(&mut *assets, c.truncate(), (cam.distance * 0.6).clamp(30.0, 200.0)) {
                        disc(&mut x.overlay, cam, a.pos + DVec3::Z * 0.2, dw * 2.0, [1.0, 1.0, 1.0, 0.7]);
                    }
                }
                if let (Some((p, h)), Some(rel)) = (self.ghost_pos, &self.object_type) {
                    if self.cursor_in_view {
                        let ok = doc.tiles.contains_key(&tile_of(p.truncate()));
                        x.ghosts.push(Ghost { rel: rel.clone(), world: p, rot: [h, 0.0, 0.0], tint: if ok { [0.35, 0.85, 0.45, 0.35] } else { [1.0, 0.3, 0.3, 0.5] } });
                        if let Some((p2, h2)) = self.stop_twin {
                            x.ghosts.push(Ghost { rel: rel.clone(), world: p2, rot: [h2, 0.0, 0.0], tint: [0.35, 0.85, 0.45, 0.25] });
                        }
                        // an arrow for the front
                        let d = roads::dir(h);
                        line(&mut x.overlay, cam, p + DVec3::Z * 0.3, p + DVec3::new(d.x * 6.0, d.y * 6.0, 0.3), w, GREEN);
                        // set against a road: the road's edge it keeps its distance from
                        if let Some((edge, at)) = self.align_guide {
                            line(&mut x.overlay, cam, edge + DVec3::Z * 0.3, at + DVec3::Z * 0.3, w * 0.5, WHITE);
                            disc(&mut x.overlay, cam, edge + DVec3::Z * 0.3, w * 1.5, ORANGE);
                        }
                    }
                }
            }
            Tool::Tiles => {
                x.highlight_tile = self.hover_tile;
                if let Some(k) = self.hover_tile {
                    if !doc.tiles.contains_key(&k) {
                        let o = tile_origin(k);
                        let ts = tile_size();
                        let pts = [DVec3::new(o.x, o.y, 0.0), DVec3::new(o.x + ts, o.y, 0.0), DVec3::new(o.x + ts, o.y + ts, 0.0), DVec3::new(o.x, o.y + ts, 0.0)];
                        for i in 0..4 {
                            line(&mut x.overlay, cam, pts[i], pts[(i + 1) % 4], w * 2.0, GREEN);
                        }
                    }
                }
                // the outline of every tile that may be added
                for k in doc.tiles.keys() {
                    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                        let n = (k.0 + dx, k.1 + dy);
                        if doc.tiles.contains_key(&n) {
                            continue;
                        }
                        let o = tile_origin(n);
                        let ts = tile_size();
                        let h = doc.ground(tile_origin(*k) + DVec2::splat(ts * 0.5));
                        let c = DVec3::new(o.x + ts * 0.5, o.y + ts * 0.5, h);
                        disc(&mut x.overlay, cam, c, w * 6.0, [1.0, 1.0, 1.0, 0.35]);
                    }
                }
            }
            _ => {}
        }
        self.selection_extras(&mut x, cam, w, assets);
        x.resized = self.scale_preview.clone();
        if let (Some(m), Some(view)) = (self.mesh.as_ref().filter(|m| m.editing), self.gizmo_view.as_ref()) {
            m.extras(view, &mut x);
            x.resized.push((m.inst, DVec3::ZERO));
            x.hover = None;
        }
        self.gizmo_extras(&mut x);
        // the grid things snap to, round the cursor (fading out), while snapping is on
        if let (true, Some(c)) = (self.snap_grid, self.cursor) {
            let step = self.grid_step.max(0.1);
            // (no more lines than a few dozen on the screen: a fine step seen from afar shows
            // every so many)
            let every = ((cam.distance * 0.02) / step).ceil().max(1.0);
            let cell = step * every;
            let n = 12i32;
            let base = DVec2::new((c.x / cell).round() * cell, (c.y / cell).round() * cell);
            let reach = cell * n as f64;
            for i in -n..=n {
                for (along_x, off) in [(true, i as f64 * cell), (false, i as f64 * cell)] {
                    // (each line in pieces, so it lies on the ground)
                    for j in -n..n {
                        let (s0, s1) = (j as f64 * cell, (j + 1) as f64 * cell);
                        let (p0, p1) = if along_x { (base + DVec2::new(s0, off), base + DVec2::new(s1, off)) } else { (base + DVec2::new(off, s0), base + DVec2::new(off, s1)) };
                        let fade = 1.0 - ((p0 + p1) * 0.5 - c.truncate()).length() / reach;
                        if fade <= 0.0 {
                            continue;
                        }
                        let z0 = doc.ground(p0) + 0.15;
                        let z1 = doc.ground(p1) + 0.15;
                        line(&mut x.overlay, cam, DVec3::new(p0.x, p0.y, z0), DVec3::new(p1.x, p1.y, z1), w * 0.35, [1.0, 1.0, 1.0, 0.35 * fade as f32]);
                    }
                }
            }
        }
        // the knife: a line across the road where a click cuts it
        if let Some(c) = self.cut_preview() {
            let half = doc.spline(c.key, c.id).and_then(|s| assets.spline(&s.file)).map(|m| m.half_width as f64).unwrap_or(4.0).max(1.5) + 0.5;
            let side = DVec3::new(c.dir.y, -c.dir.x, 0.0) * half;
            let lift = DVec3::Z * 0.4;
            let col = if c.fits { [1.0, 0.25, 0.2, 1.0] } else { [0.6, 0.6, 0.6, 1.0] };
            line(&mut x.overlay, cam, c.pos - side + lift, c.pos + side + lift, w * 1.5, col);
            disc(&mut x.overlay, cam, c.pos + lift, w * 2.5, col);
        }
        if let Some((a, b)) = self.ruler_line() {
            let lift = DVec3::Z * 0.5;
            line(&mut x.overlay, cam, a + lift, b + lift, w, [1.0, 0.85, 0.2, 1.0]);
            for p in [a, b] {
                disc(&mut x.overlay, cam, p + lift, w * 2.5, [1.0, 0.85, 0.2, 1.0]);
            }
        }
        x
    }
}

/// The free road ends within `reach` of `centre` that face it (the arms a junction plate
/// there had), nearest first.
fn ends_facing(doc: &Document, centre: DVec2, reach: f64) -> Vec<crate::junction::Arm> {
    let mut found: Vec<(f64, crate::junction::Arm)> = Vec::new();
    for (k, t) in &doc.tiles {
        let org = tile_origin(*k);
        for s in t.tile.splines.iter().filter(|s| !s.deleted) {
            let c = omsi_geometry::SplineCurve::from_map(s, org);
            for at_end in [false, true] {
                if (if at_end { s.next_id } else { s.prev_id }) != 0 {
                    continue;
                }
                let (p, h) = if at_end { (c.end_point(), c.heading_at(s.length)) } else { (c.point_at(0.0), c.heading_at(0.0) + 180.0) };
                let to = centre - p.truncate();
                let d = to.length();
                let off = ((roads::heading_of(to) - h + 540.0).rem_euclid(360.0) - 180.0).abs();
                if d < reach && (d < 1.0 || off < 35.0) {
                    found.push((d, crate::junction::Arm { key: *k, id: s.id, at_end }));
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    found.into_iter().map(|f| f.1).collect()
}

/// A junction openMapper made (and can make again): `Sceneryobjects\openMapper\<map>\junction_<id>.sco`.
/// A junction plate's helper points (x, forward, height, heading out) and whether it stands
/// at a height of its own: from its model, or, for a plate made in this very build and not
/// loaded yet, read from its file.
fn plate_helpers(content: &std::path::Path, assets: &mut Assets, file: &str) -> Option<(bool, Vec<[f32; 4]>)> {
    if let Some(m) = assets.object(file) {
        return Some((m.abs_height, m.helpers.iter().map(|h| h.0).collect()));
    }
    let sco = omsi_scenery::SceneryObject::load(&omsi_cfg::resolve_path(content, file)).ok()?;
    // (OMSI's order: x, height, forward)
    Some((sco.absolute_height(), sco.spline_helpers.iter().map(|h| [h.x, h.z, h.y, h.heading + 180.0]).collect()))
}

pub fn is_own_junction(file: &str) -> bool {
    let f = file.to_lowercase();
    f.contains("\\openmapper\\") && f.contains("junction_")
}

/// A heading to the nearest whole 15 degrees.
pub fn snap_deg(h: f64, step: f64) -> f64 {
    let step = if step > 0.0 { step } else { 15.0 };
    roads::norm_deg((h / step).round() * step)
}

// -------------------------------------------------------------------- selection and editing

impl Editor {
    /// Everything selected, the one shown in the properties first.
    pub fn selection(&self) -> Vec<Sel> {
        let Some(p) = self.selected else { return Vec::new() };
        let mut v = vec![p];
        for s in &self.also {
            if !v.contains(s) {
                v.push(*s);
            }
        }
        v
    }

    pub fn is_selected(&self, s: Sel) -> bool {
        self.selected == Some(s) || (self.selected.is_some() && self.also.contains(&s))
    }

    /// Select one thing (or nothing).
    pub fn select(&mut self, s: Option<Sel>) {
        self.selected = s;
        self.path_point = None;
        self.also.clear();
    }

    pub fn set_selection(&mut self, v: Vec<Sel>) {
        let mut v = v.into_iter();
        self.selected = v.next();
        self.path_point = None;
        self.also = v.collect();
    }

    /// Shift-click: in the selection if it was not, out if it was.
    pub fn toggle_selected(&mut self, s: Sel) {
        let mut v = self.selection();
        // (a group comes and goes as one)
        let whole = if self.ctrl_held { vec![s] } else { self.with_groups(vec![s]) };
        if v.contains(&s) {
            v.retain(|x| !whole.contains(x));
        } else {
            for (i, w) in whole.into_iter().enumerate() {
                if !v.contains(&w) {
                    v.insert(i, w);
                }
            }
        }
        self.set_selection(v);
    }

    /// The spline selected alone (its grips are shown).
    pub fn single_spline(&self) -> Option<(TileKey, i64)> {
        match (self.selected, self.also.iter().any(|s| Some(*s) != self.selected)) {
            (Some(Sel::Spline(k, id)), false) => Some((k, id)),
            _ => None,
        }
    }

    /// Where the grips are: a road's three (its two ends and its middle); on an AI path, a grip
    /// at each of its points (the ends and the joints of its pieces) and one in the middle of
    /// each piece, to bend it.
    fn handles(&self) -> Vec<((TileKey, i64), Handle, DVec3)> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let pieces: Vec<(TileKey, i64)> = if self.editing_paths() {
            self.selection().into_iter().filter_map(|s| if let Sel::Spline(k, id) = s { Some((k, id)) } else { None }).collect()
        } else {
            self.single_spline().into_iter().collect()
        };
        let mut out = Vec::new();
        for (k, id) in pieces.iter().copied() {
            let Some(s) = doc.spline(k, id) else { continue };
            let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(k));
            // (a joint is the grip of the piece that ends there)
            let joined_before = self.editing_paths() && s.prev_id != 0 && pieces.iter().any(|p| p.1 == s.prev_id);
            if !joined_before {
                out.push(((k, id), Handle::Start, c.point_at(0.0)));
            }
            out.push(((k, id), Handle::End, c.end_point()));
            out.push(((k, id), Handle::Mid, c.point_at(s.length * 0.5)));
        }
        out
    }

    /// A grip as a ball: its centre and radius. The radius follows the size of the spline
    /// (a kerb's grips are small, a street's larger), but never drops below a few pixels
    /// on the screen, however far the camera is.
    fn grip(&self, assets: &mut Assets, sp: (TileKey, i64), h: Handle, at: DVec3, dist: f64) -> (DVec3, f64) {
        let half = self.doc.as_ref().and_then(|d| d.spline(sp.0, sp.1)).and_then(|s| assets.spline(&s.file)).map(|m| m.half_width as f64).unwrap_or(1.5);
        let r = (half * 0.05).max(dist * 0.0025).clamp(0.05, 1.0);
        let r = if h == Handle::Mid { r * 0.75 } else { r };
        (at + DVec3::Z * (r * 0.5), r)
    }

    /// The grip the ray from `o` along `d` passes through (or just by: a grip takes clicks
    /// well beyond its edge). A point beats the middle of a piece when they lie close.
    fn handle_at_ray(&self, assets: &mut Assets, o: DVec3, d: DVec3) -> Option<((TileKey, i64), Handle)> {
        let dir = d.normalize_or_zero();
        if dir == DVec3::ZERO {
            return None;
        }
        self.handles()
            .into_iter()
            .filter_map(|(sp, h, q)| {
                let t = (q - o).dot(dir);
                if t <= 0.0 {
                    return None;
                }
                let (c, r) = self.grip(assets, sp, h, q, t);
                let off = (c - (o + dir * (c - o).dot(dir))).length();
                (off < r * 1.8).then_some(((sp, h), off / r + if h == Handle::Mid { 0.3 } else { 0.0 }))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(x, _)| x)
    }

    /// Take the picked point out of an AI path: the two pieces that meet there become one (its
    /// shape fitted again between their far ends), or, at an end, the last piece goes.
    fn remove_path_point(&mut self) {
        let Some(((k, id), h)) = self.path_point.take() else { return };
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(s) = doc.spline(k, id).cloned() else { return };
        // (the piece before the point and the one after it)
        let (a, b) = match h {
            Handle::End if s.next_id != 0 => (Some((k, id)), doc.find_spline(s.next_id).map(|(nk, n)| (nk, n.id))),
            Handle::Start if s.prev_id != 0 => (doc.find_spline(s.prev_id).map(|(pk, p)| (pk, p.id)), Some((k, id))),
            Handle::Mid => {
                self.toast(tr("Pick a point (a dot at an end or where pieces meet) to take it out", "Выберите точку (кружок на конце или на стыке кусков), чтобы убрать её"));
                return;
            }
            // an end of the path: its last piece goes, if there is another
            _ => (None, None),
        };
        match (a, b) {
            (Some(a), Some(b)) => {
                let (Some(sa), Some(sb)) = (doc.spline(a.0, a.1).cloned(), doc.spline(b.0, b.1).cloned()) else { return };
                let (ca, cb) = (omsi_geometry::SplineCurve::from_map(&sa, tile_origin(a.0)), omsi_geometry::SplineCurve::from_map(&sb, tile_origin(b.0)));
                let (p0, p1) = (ca.point_at(0.0), cb.end_point());
                let keep0 = sa.prev_id != 0;
                let keep1 = sb.next_id != 0;
                let segs = reshape::fit(
                    reshape::End { pos: p0.truncate(), heading: keep0.then(|| roads::norm_deg(sa.heading)) },
                    reshape::End { pos: p1.truncate(), heading: keep1.then(|| roads::norm_deg(cb.heading_at(sb.length))) },
                )
                .or_else(|| reshape::fit(reshape::End { pos: p0.truncate(), heading: None }, reshape::End { pos: p1.truncate(), heading: None }));
                let Some(segs) = segs else {
                    self.error(tr("That bend is too sharp", "Слишком крутой поворот"));
                    return;
                };
                self.history.begin(tr("Remove a point of the path", "Убрать точку пути"));
                let plan = reshape::Plan { pieces: vec![reshape::Piece { key: a.0, id: a.1, segs, z0: p0.z, z1: p1.z, g0: None, g1: None }] };
                self.apply_plan(&plan);
                if let Some(doc) = self.doc.as_mut() {
                    // (the second piece goes; what followed it follows the first)
                    for t in [a.0, b.0] {
                        self.history.touch_tile(doc, t);
                    }
                    let after = sb.next_id;
                    doc.delete_spline(b.0, b.1);
                    if let Some((nk, _)) = doc.find_spline(after) {
                        self.history.touch_tile(doc, nk);
                    }
                    if let Some(t) = doc.tiles.get_mut(&a.0) {
                        if let Some(x) = t.tile.splines.iter_mut().find(|x| x.id == a.1) {
                            x.next_id = after;
                        }
                        t.touch_content();
                    }
                    if after != 0 {
                        if let Some((nk, _)) = doc.find_spline(after) {
                            if let Some(t) = doc.tiles.get_mut(&nk) {
                                for x in t.tile.splines.iter_mut().filter(|x| x.id == after) {
                                    x.prev_id = a.1;
                                }
                                t.touch_content();
                            }
                        }
                    }
                }
                self.history.commit();
                self.select_path_pieces(a.0, a.1);
                self.toast(tr("Point taken out", "Точка убрана"));
            }
            _ => {
                // an end: the piece that ends the path goes, if the path has more than that one
                let linked = if h == Handle::Start { s.next_id != 0 } else { s.prev_id != 0 };
                if !linked {
                    self.toast(tr("A path of one piece: Del with nothing picked deletes it", "Путь из одного куска: Del без выбранной точки удалит его целиком"));
                    return;
                }
                self.history.begin(tr("Remove a point of the path", "Убрать точку пути"));
                if let Some(doc) = self.doc.as_mut() {
                    let (other, other_end) = if h == Handle::Start { (s.next_id, false) } else { (s.prev_id, true) };
                    self.history.touch_tile(doc, k);
                    if let Some((ok, _)) = doc.find_spline(other) {
                        self.history.touch_tile(doc, ok);
                        if let Some(t) = doc.tiles.get_mut(&ok) {
                            for x in t.tile.splines.iter_mut().filter(|x| x.id == other) {
                                if other_end { x.next_id = 0 } else { x.prev_id = 0 }
                            }
                            t.touch_content();
                        }
                        doc.delete_spline(k, id);
                        self.history.commit();
                        self.select_path_pieces(ok, other);
                        return;
                    }
                }
                self.history.commit();
            }
        }
    }

    /// The Move / Rotate / Scale handles of the selection (None: the Select tool's plain
    /// mode, nothing selected, or a single road, which has grips of its own).
    pub fn gizmo(&self) -> Option<crate::gizmo::Gizmo> {
        use crate::gizmo::{Gizmo, Mode};
        if let Some(m) = self.mesh.as_ref().filter(|m| m.editing) {
            // (in the edit mode the handles move what is selected, even in plain Select)
            return m.gizmo(match self.xform {
                Xform::Rotate => Mode::Rotate,
                Xform::Scale => Mode::Scale,
                _ => Mode::Move,
            });
        }
        if self.tool != Tool::Select || self.xform == Xform::Select || self.cutting || self.ruler.is_some() {
            return None;
        }
        let sels = self.selection();
        if sels.is_empty() || self.single_spline().is_some() {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let pts: Vec<DVec3> = sels.iter().filter_map(|s| self.world_of(doc, *s)).collect();
        if pts.is_empty() {
            return None;
        }
        let pivot = pts.iter().sum::<DVec3>() / pts.len() as f64;
        let mode = match self.xform {
            Xform::Rotate => Mode::Rotate,
            Xform::Scale => Mode::Scale,
            _ => Mode::Move,
        };
        // (one object scales along its own axes; the rest use the world's)
        let axes = match (mode, sels.as_slice()) {
            (Mode::Scale, [Sel::Object(k, id)]) => doc.object(*k, *id).map(|o| glam::DMat3::from_quat(crate::gizmo::object_quat(o.rot))).unwrap_or(glam::DMat3::IDENTITY),
            _ => glam::DMat3::IDENTITY,
        };
        // (a road turns only about the vertical: tilting it is its climb and banking)
        let roads = sels.iter().any(|s| matches!(s, Sel::Spline(..)));
        let allowed = if mode == Mode::Rotate && roads { [false, false, true] } else { [true; 3] };
        Some(Gizmo { mode, pivot, axes, allowed })
    }

    /// Where a selected thing stands in the world: an object's origin, a road's middle.
    fn world_of(&self, doc: &Document, s: Sel) -> Option<DVec3> {
        match s {
            Sel::Object(k, id) => {
                let o = doc.object(k, id)?;
                let abs = self.known_helpers.get(&o.file.to_lowercase()).map(|h| h.0).unwrap_or(false);
                Some(doc.object_world(k, o, abs))
            }
            Sel::Spline(k, id) => doc.spline(k, id).map(|sp| omsi_geometry::SplineCurve::from_map(sp, tile_origin(k)).point_at(sp.length * 0.5)),
        }
    }

    /// The view the handles are picked and dragged in, each frame.
    pub fn set_gizmo_view(&mut self, cam: &crate::camera::Camera, w: f32, h: f32, x: f32, y: f32) {
        self.gizmo_view = Some(crate::gizmo::View { cam: cam.clone(), w: w.max(1.0), h: h.max(1.0), pointer: DVec2::new(x as f64, y as f64) });
    }

    /// A drag let go of without its end (cancelled with Esc).
    pub fn stop_drag(&mut self) {
        self.drag = Drag::None;
    }

    /// What a drag of the handles or a sized operation has come to, for the status bar.
    pub fn drag_readout(&self) -> Option<String> {
        use crate::gizmo::Mode;
        if let Some((kind, v)) = self.mesh.as_ref().and_then(|m| m.modal_value()) {
            let what = match kind {
                crate::meshedit::Sizing::Extrude => tr("Extrude", "Выдавливание"),
                crate::meshedit::Sizing::Inset => tr("Inset", "Вставка"),
                crate::meshedit::Sizing::Bevel => tr("Bevel", "Фаска"),
                crate::meshedit::Sizing::LoopCut(_) => tr("Loop cut", "Разрез петлёй"),
            };
            return Some(format!("{what} {v:.3} · {}", tr("click: done, right click / Esc: cancel, wheel: segments", "клик: готово, правый клик / Esc: отмена, колесо: сегменты")));
        }
        let mode = match self.drag {
            Drag::Gizmo { grab, started: true } | Drag::MeshGizmo { grab, started: true } => grab.gizmo.mode,
            _ => return None,
        };
        Some(match mode {
            Mode::Move => format!("{} {:.3} m", tr("Move", "Перемещение"), self.xform_value),
            Mode::Rotate => format!("{} {:.1}°", tr("Rotate", "Поворот"), self.xform_value),
            Mode::Scale => format!("{} ×{:.3}", tr("Scale", "Масштаб"), self.xform_value),
        })
    }

    pub fn gizmo_view(&self) -> Option<crate::gizmo::View> {
        self.gizmo_view.clone()
    }

    /// Whether the cursor is on a handle (a press there takes it instead of the map).
    pub fn gizmo_under_cursor(&self) -> bool {
        matches!(self.drag, Drag::Gizmo { .. } | Drag::MeshGizmo { .. }) || matches!((self.gizmo(), &self.gizmo_view), (Some(g), Some(v)) if g.hit(v).is_some())
    }

    fn start_gizmo(&mut self, gz: crate::gizmo::Gizmo, part: crate::gizmo::Part, view: &crate::gizmo::View) {
        let Some(doc) = self.doc.as_ref() else { return };
        let sels = self.selection();
        let mut from: Vec<Origin> = sels.iter().filter_map(|s| selection::origin_of(doc, *s)).collect();
        let mut partial = false;
        if gz.mode == crate::gizmo::Mode::Scale {
            let had = from.len();
            from.retain(|o| matches!(o.sel, Sel::Object(..)));
            if from.is_empty() {
                self.toast(tr("Roads are not scaled: change their length and width in their properties", "Дороги не масштабируются: длина и ширина меняются в их свойствах"));
                return;
            }
            partial = from.len() < had;
        }
        self.move_z_from = from.iter().map(|o| match o.sel {
            Sel::Object(k, id) => doc.object(k, id).map(|o| o.pos[2]).unwrap_or(0.0),
            Sel::Spline(k, id) => doc.spline(k, id).map(|s| s.pos[2]).unwrap_or(0.0),
        })
        .collect();
        self.move_rot_from = from.iter().map(|o| if let Sel::Object(k, id) = o.sel { doc.object(k, id).map(|o| o.rot).unwrap_or([0.0; 3]) } else { [0.0; 3] }).collect();
        self.move_world_from = from.iter().map(|o| self.world_of(doc, o.sel).unwrap_or(DVec3::new(o.pos.x, o.pos.y, 0.0))).collect();
        self.move_from = from;
        self.xform_value = 0.0;
        self.drag = Drag::Gizmo { grab: crate::gizmo::Grab::new(gz, part, view), started: false };
        if partial {
            self.toast(tr("Only the objects are scaled, the roads stay", "Масштабируются только объекты, дороги остаются"));
        }
    }

    /// A handle dragged: the selection follows from where it stood when it was taken.
    fn drag_gizmo(&mut self, grab: crate::gizmo::Grab, started: bool) {
        use crate::gizmo::Change;
        let Some(view) = self.gizmo_view.clone() else { return };
        let change = grab.change(&view, self.snap_angle.then_some(self.angle_step));
        if !started {
            if !grab.moved(&view) || self.move_from.is_empty() {
                return;
            }
            self.history.begin(match change {
                Change::Move(_) => tr("Move", "Перемещение"),
                Change::Turn(..) => tr("Rotate", "Поворот"),
                Change::Scale(_) => tr("Scale", "Масштаб"),
            });
            self.drag = Drag::Gizmo { grab, started: true };
            self.move_arms = self.arms_for(&self.selection());
        }
        match change {
            Change::Move(d) => self.gizmo_move(grab.part, d),
            Change::Turn(axis, deg) => self.gizmo_turn(grab.gizmo.pivot, axis, deg),
            Change::Scale(s) => self.gizmo_scale(&grab.gizmo, s),
        }
    }

    fn gizmo_move(&mut self, part: crate::gizmo::Part, d: DVec3) {
        let first = self.move_from[0].pos;
        let mut delta = d.truncate();
        if delta.length() > 1e-9 {
            delta = self.snap_point(first + delta) - first;
            // (the grid may not pull it off the arrow it is held to)
            match part {
                crate::gizmo::Part::Axis(0) => delta.y = 0.0,
                crate::gizmo::Part::Axis(1) => delta.x = 0.0,
                _ => {}
            }
        }
        let dz = if self.snap_grid { (d.z / 0.05).round() * 0.05 } else { d.z };
        let Some(doc) = self.doc.as_mut() else { return };
        let now = selection::transform(doc, &mut self.history, &self.move_from, DVec2::ZERO, 0.0, delta);
        for (i, o) in self.move_from.iter().enumerate() {
            set_height(doc, &mut self.history, o.sel, self.move_z_from[i] + dz);
        }
        let all: Vec<selection::Arm> = self.move_arms.iter().flat_map(|g| g.1.iter().copied()).collect();
        self.carry_preview = selection::carry_plans(doc, &all, DVec2::ZERO, 0.0, delta, 0.0, &|_| false).into_iter().flatten().collect();
        self.move_delta = delta;
        self.xform_value = DVec3::new(delta.x, delta.y, dz).length();
        self.set_selection(now);
    }

    fn gizmo_turn(&mut self, pivot: DVec3, axis: DVec3, deg: f64) {
        let Some(doc) = self.doc.as_mut() else { return };
        // about the vertical: as the Select tool always turned things (roads too)
        if axis.z.abs() > 0.999 {
            let cw = -deg * axis.z.signum();
            let now = selection::transform(doc, &mut self.history, &self.move_from, pivot.truncate(), cw, DVec2::ZERO);
            let all: Vec<selection::Arm> = self.move_arms.iter().flat_map(|g| g.1.iter().copied()).collect();
            self.carry_preview = selection::carry_plans(doc, &all, pivot.truncate(), cw, DVec2::ZERO, 0.0, &|_| false).into_iter().flatten().collect();
            self.xform_value = cw;
            self.set_selection(now);
            return;
        }
        let q = glam::DQuat::from_axis_angle(axis, deg.to_radians());
        let mut now = Vec::new();
        for (i, o) in self.move_from.iter().enumerate() {
            let Sel::Object(_, id) = o.sel else { continue };
            let p = pivot + q * (self.move_world_from[i] - pivot);
            let rot = crate::gizmo::object_rot(q * crate::gizmo::object_quat(self.move_rot_from[i]));
            if let Some(s) = put_object_3d(doc, &mut self.history, id, p, rot, &self.known_helpers) {
                now.push(s);
            }
        }
        self.xform_value = deg;
        self.set_selection(now);
    }

    fn gizmo_scale(&mut self, gz: &crate::gizmo::Gizmo, s: DVec3) {
        let Some(doc) = self.doc.as_mut() else { return };
        let mut now = Vec::new();
        let mut preview = Vec::new();
        for (i, o) in self.move_from.iter().enumerate() {
            let Sel::Object(_, id) = o.sel else { continue };
            // where it stands spreads from the middle along the handles' axes
            let local = gz.axes.transpose() * (self.move_world_from[i] - gz.pivot);
            let p = gz.pivot + gz.axes * (local * s);
            if let Some(sel) = put_object_3d(doc, &mut self.history, id, p, self.move_rot_from[i], &self.known_helpers) {
                now.push(sel);
                // and its own size, along its own axes
                let own = glam::DMat3::from_quat(crate::gizmo::object_quat(self.move_rot_from[i]));
                let world_scale = gz.axes * glam::DMat3::from_diagonal(s) * gz.axes.transpose();
                let f = DVec3::new((world_scale * own.col(0)).length(), (world_scale * own.col(1)).length(), (world_scale * own.col(2)).length());
                preview.push((sel, f));
            }
        }
        self.xform_value = if (s.x - s.y).abs() < 1e-9 && (s.y - s.z).abs() < 1e-9 { s.x } else { s.max_element().max(s.min_element()) };
        self.scale_preview = preview;
        self.set_selection(now);
    }

    /// A handle let go: the roads the selection holds follow it, the ground under roads is
    /// made again, and scaled objects are handed to the app for their files.
    fn finish_gizmo(&mut self, assets: &mut Assets, grab: crate::gizmo::Grab) {
        let sels = self.selection();
        if let Some(doc) = self.doc.as_mut() {
            selection::unlink_outside(doc, &mut self.history, &sels);
        }
        let arms = std::mem::take(&mut self.move_arms);
        match grab.gizmo.mode {
            crate::gizmo::Mode::Move => self.carry(&arms, DVec2::ZERO, 0.0, self.move_delta, 0.0),
            crate::gizmo::Mode::Rotate if matches!(grab.part, crate::gizmo::Part::Ring(2)) => self.carry(&arms, grab.gizmo.pivot.truncate(), self.xform_value, DVec2::ZERO, 0.0),
            _ => {}
        }
        self.ground_to_selection();
        self.rebuild_pending(assets);
        self.scale_requests = std::mem::take(&mut self.scale_preview).into_iter().filter(|(_, f)| (*f - DVec3::ONE).abs().max_element() > 1e-4).collect();
    }

    /// Give the objects a scale dragged out to scaled copies of their types (the step the drag
    /// opened is closed here).
    pub fn apply_scales(&mut self, roots: &crate::install::Roots) {
        if self.scale_requests.is_empty() {
            return;
        }
        let reqs = std::mem::take(&mut self.scale_requests);
        let mut failed = None;
        for (sel, f) in reqs {
            let Sel::Object(_, id) = sel else { continue };
            let Some(doc) = self.doc.as_mut() else { break };
            let Some((k, o)) = selection::object_by_id(doc, id) else { continue };
            // (openMapper's own objects take the size into their meshes, a copy of their own)
            let made = if crate::editable::is_editable(roots, &o.file) { crate::editable::scaled_copy(roots, &o.file, f) } else { crate::scaled::variant(roots, &o.file, f) };
            match made {
                Ok(rel) => {
                    self.history.touch_tile(doc, k);
                    // (the copy has the same height and helpers as what it was made from)
                    if let Some(h) = self.known_helpers.get(&o.file.to_lowercase()).cloned() {
                        self.known_helpers.insert(rel.to_lowercase(), h);
                    }
                    if let Some(ob) = doc.object_mut(k, id) {
                        ob.file = rel;
                    }
                    if let Some(t) = doc.tiles.get_mut(&k) {
                        t.touch_content();
                    }
                }
                Err(e) => failed = Some(e.to_string()),
            }
        }
        self.history.commit();
        if let Some(e) = failed {
            self.error(format!("{}: {e}", tr("Could not scale", "Не удалось масштабировать")));
        }
    }

    /// The Move / Rotate / Scale handles drawn on the selection.
    fn gizmo_extras(&self, x: &mut FrameExtras) {
        let (Some(gz), Some(view)) = (self.gizmo(), self.gizmo_view.as_ref()) else { return };
        let (gz, active) = match self.drag {
            // (while dragging, the handles stay where they were taken)
            Drag::Gizmo { grab, started: true } => (crate::gizmo::Gizmo { pivot: grab.gizmo.pivot + self.drag_offset(&grab), ..grab.gizmo }, Some(grab.part)),
            Drag::Gizmo { grab, .. } => (gz, Some(grab.part)),
            Drag::MeshGizmo { grab, started: true } => (crate::gizmo::Gizmo { pivot: grab.gizmo.pivot, ..gz }, Some(grab.part)),
            Drag::MeshGizmo { grab, .. } => (gz, Some(grab.part)),
            _ => (gz, None),
        };
        let hot = if active.is_none() { gz.hit(view) } else { None };
        gz.draw(view, hot, active, &mut x.overlay);
    }

    /// How far the handles have gone with a move being dragged.
    fn drag_offset(&self, grab: &crate::gizmo::Grab) -> DVec3 {
        match (grab.gizmo.mode, &self.gizmo_view) {
            (crate::gizmo::Mode::Move, Some(view)) => match grab.change(view, None) {
                crate::gizmo::Change::Move(d) => d,
                _ => DVec3::ZERO,
            },
            _ => DVec3::ZERO,
        }
    }

    /// The left button down with the Select tool.
    fn select_press(&mut self, shift: bool) {
        // (whatever the properties had open is done: the view's edit is a step of its own)
        if self.panel_step {
            self.panel_step = false;
            self.history.commit();
        }
        if let (Some(gz), Some(view)) = (self.gizmo(), self.gizmo_view.clone()) {
            if let Some(part) = gz.hit(&view) {
                if self.mesh.as_ref().map(|m| m.editing).unwrap_or(false) {
                    self.drag = Drag::MeshGizmo { grab: crate::gizmo::Grab::new(gz, part, &view), started: false };
                } else {
                    self.start_gizmo(gz, part, &view);
                }
                return;
            }
        }
        if let (Some(m), Some(view)) = (self.mesh.as_mut().filter(|m| m.editing), self.gizmo_view.clone()) {
            // (a press in the edit mode: an operation going on is done, or vertices, edges or
            // faces are picked; a second click soon picks the loop)
            if m.modal_active() {
                m.confirm();
                return;
            }
            let hit = m.pick(&view);
            let double = hit.is_some() && self.last_mesh_click.map(|(h, t)| Some(h) == hit && t.elapsed().as_millis() < 450).unwrap_or(false);
            m.click(hit, shift, double);
            self.last_mesh_click = if double { None } else { hit.map(|h| (h, Instant::now())) };
            return;
        }
        if let (Some(h), Some((k, id))) = (self.hover_handle, self.hover_handle_of) {
            // (on an AI path the grip pressed is the point picked: Del takes it out)
            self.path_point = self.editing_paths().then_some(((k, id), h));
            self.drag = Drag::Reshape(k, id, h);
            self.reshape = None;
            return;
        }
        let Some(hit) = self.hover else {
            if !shift {
                self.select(None);
            }
            return;
        };
        if shift {
            self.toggle_selected(hit);
            return;
        }
        // Ctrl + click on the AI path being edited: a new point there, to drag
        if let (true, true, Sel::Spline(k, id), Some(c)) = (self.editing_paths(), self.ctrl_held, hit, self.cursor) {
            self.split_path_at(k, id, c.truncate());
            return;
        }
        // a double click on a road: the whole road, all its pieces up to where it branches or ends
        let double = self.last_click.map(|(s, t)| s == hit && t.elapsed().as_millis() < 450).unwrap_or(false);
        self.last_click = Some((hit, Instant::now()));
        if let (true, Sel::Spline(k, id)) = (double, hit) {
            self.last_click = None;
            // (an AI path: a double click puts a point in it, to drag; a road: the whole road)
            if let (true, Some(c)) = (self.editing_paths(), self.cursor) {
                self.split_path_at(k, id, c.truncate());
            } else {
                self.select_chain(k, id);
            }
            return;
        }
        let was_selected = self.is_selected(hit);
        if !self.is_selected(hit) {
            // (a group's thing brings the whole group; Ctrl + click takes it alone)
            let v = if self.ctrl_held { vec![hit] } else { self.with_groups(vec![hit]) };
            self.set_selection(v);
        }
        // an AI path is picked as a whole, each point of it a grip of its own
        if let (true, false, Sel::Spline(k, id)) = (self.editing_paths(), was_selected, hit) {
            self.select_path_pieces(k, id);
        }
        // a press on the selection may become a move of all of it
        if let (Some(c), Some(doc)) = (self.cursor, self.doc.as_ref()) {
            let mut sels = self.selection();
            // (the one grabbed leads: the grid holds its place)
            if let Some(i) = sels.iter().position(|s| *s == hit) {
                sels.swap(0, i);
            }
            self.move_from = sels.iter().filter_map(|s| selection::origin_of(doc, *s)).collect();
            self.drag = Drag::Move { grab: c.truncate(), started: false };
        }
    }

    /// The spline and every one linked on from it either way: the road it is a piece of.
    /// An AI path's pieces round `id`: up to 8 each way along it (a long route through a whole
    /// town is not all picked at once, each piece with grips of its own).
    pub fn select_path_pieces(&mut self, k: TileKey, id: i64) {
        let Some(doc) = self.doc.as_ref() else { return };
        let mut v = vec![Sel::Spline(k, id)];
        let Some((_, first)) = doc.find_spline(id) else { return };
        for dir in [false, true] {
            let mut cur = first.clone();
            for _ in 0..8 {
                let n = if dir { cur.next_id } else { cur.prev_id };
                let Some((nk, next)) = (n != 0).then(|| doc.find_spline(n)).flatten() else { break };
                if v.contains(&Sel::Spline(nk, n)) {
                    break;
                }
                v.push(Sel::Spline(nk, n));
                cur = next.clone();
            }
        }
        self.set_selection(v);
    }

    pub fn select_chain(&mut self, k: TileKey, id: i64) {
        let Some(doc) = self.doc.as_ref() else { return };
        let mut v = vec![Sel::Spline(k, id)];
        let mut seen = std::collections::HashSet::from([id]);
        let mut todo = vec![id];
        while let Some(cur) = todo.pop() {
            let Some((_, s)) = doc.find_spline(cur) else { continue };
            for n in [s.prev_id, s.next_id] {
                if n != 0 && seen.len() < 5000 && seen.insert(n) {
                    if let Some((nk, _)) = doc.find_spline(n) {
                        v.push(Sel::Spline(nk, n));
                        todo.push(n);
                    }
                }
            }
        }
        self.set_selection(v);
    }

    /// A point put into an AI path where it is nearest to `p`: the spline is cut there, so
    /// the joint becomes a grip that drags both pieces.
    fn split_path_at(&mut self, k: TileKey, id: i64, p: DVec2) {
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(s) = doc.spline(k, id) else { return };
        let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(k));
        let n = (s.length / 0.5).ceil().max(2.0) as usize;
        let at = (0..=n).map(|i| s.length * i as f64 / n as f64).min_by(|a, b| (c.point_at(*a).truncate() - p).length().total_cmp(&(c.point_at(*b).truncate() - p).length())).unwrap_or(s.length * 0.5);
        // (a point right at an end would leave a piece too short to grab)
        if at < 1.0 || at > s.length - 1.0 {
            self.toast(tr("Too near the end: drag the end's dot instead", "Слишком близко к концу: тяните точку конца"));
            return;
        }
        let next = doc.find_spline(s.next_id).map(|(nk, _)| nk);
        self.history.begin(tr("Add a point to the path", "Точка в пути"));
        let doc = self.doc.as_ref().unwrap();
        self.history.touch_global(doc);
        self.history.touch_tile(doc, k);
        // (the path after it is linked to the new piece instead)
        if let Some(nk) = next {
            self.history.touch_tile(doc, nk);
        }
        self.doc.as_mut().unwrap().split_spline(k, id, at);
        self.history.commit();
        self.select_path_pieces(k, id);
        self.toast(tr("Point added: drag the dot where the pieces meet", "Точка добавлена: тяните кружок на стыке"));
    }

    /// The button let go after dragging a grip: the new shape goes in.
    fn finish_reshape(&mut self, k: TileKey, id: i64) {
        match self.reshape.take() {
            Some(Ok(plan)) => {
                self.history.begin(tr("Reshape road", "Изменение формы дороги"));
                self.apply_plan(&plan);
                if let Some((jid, centre, r)) = self.reshape_plate.take() {
                    let doc = self.doc.as_mut().unwrap();
                    if let Some(end) = selection::free_end_at(doc, centre.truncate()) {
                        if let Some(arm) = doc.trim(end.0, end.1, end.2, r) {
                            self.pending_arms.entry(jid).or_default().push(crate::junction::Arm { key: arm.0, id: arm.1, at_end: arm.2 });
                            self.pending_rebuild.push(jid);
                        }
                    }
                }
                self.finish_step();
                if self.editing_paths() {
                    self.select_path_pieces(k, id);
                } else {
                    self.select(Some(Sel::Spline(k, id)));
                }
            }
            Some(Err(e)) => self.error(e),
            None => {}
        }
    }

    /// The box dragged on the screen is let go: what is inside it is selected (added to the
    /// selection with `add`). `view`: the 3D view's corner and size in window points.
    pub fn finish_box(&mut self, assets: &mut Assets, cam: &crate::camera::Camera, view: (f32, f32, f32, f32), add: bool) {
        let Some([a, b]) = self.select_box.take() else { return };
        if let (Some(m), Some(gv)) = (self.mesh.as_mut().filter(|m| m.editing), self.gizmo_view.as_ref()) {
            let (vx, vy, _, _) = view;
            if (a.0 - b.0).abs() < 4.0 && (a.1 - b.1).abs() < 4.0 {
                if !add {
                    m.click(None, false, false);
                }
                return;
            }
            m.box_select(gv, DVec2::new((a.0 - vx) as f64, (a.1 - vy) as f64), DVec2::new((b.0 - vx) as f64, (b.1 - vy) as f64), add);
            return;
        }
        let Some(doc) = self.doc.as_ref() else { return };
        if (a.0 - b.0).abs() < 4.0 && (a.1 - b.1).abs() < 4.0 {
            return;
        }
        let (vx, vy, w, h) = view;
        let extras = FrameExtras { show_technical: self.show_technical || self.tool == Tool::Logic, hide: self.hide, ..Default::default() };
        let found = selection::in_box(doc, cam, (a.0 - vx, a.1 - vy), (b.0 - vx, b.1 - vy), w, h, |f| assets.object(f).map(|m| m.technical && !extras.show_technical || crate::render::hidden_layer(&m, &extras)).unwrap_or(false), !self.hide[2]);
        let mut v = if add { self.selection() } else { Vec::new() };
        for s in found {
            if !v.contains(&s) {
                v.push(s);
            }
        }
        let v = self.with_groups(v);
        let n = v.len();
        self.set_selection(v);
        if n > 0 {
            self.toast(format!("{} {n}", tr("Selected:", "Выбрано:")));
        }
    }

    /// Everything on the map (Ctrl+A).
    pub fn select_all(&mut self) {
        let Some(doc) = self.doc.as_ref() else { return };
        let v: Vec<Sel> = doc
            .tiles
            .iter()
            .flat_map(|(k, t)| t.tile.objects.iter().map(move |o| Sel::Object(*k, o.id)).chain(t.tile.splines.iter().filter(|s| !s.deleted).map(move |s| Sel::Spline(*k, s.id))))
            .collect();
        self.set_selection(v);
    }

    // ------------------------------------------------------------ clipboard

    /// Copy the selection; returns how many things were copied.
    pub fn copy(&mut self, lib: &Library) -> usize {
        let Some(doc) = self.doc.as_ref() else { return 0 };
        let clip = crate::clipboard::copy(doc, &self.selection(), |f| lib.find(f).map(|a| a.abs_height).unwrap_or(false));
        let n = clip.len();
        if n > 0 {
            self.clip = Some(clip);
            self.toast(format!("{} {n}", tr("Copied:", "Скопировано:")));
        }
        n
    }

    /// Something was copied (to paste).
    pub fn has_clip(&self) -> bool {
        self.clip.is_some()
    }

    pub fn cut(&mut self, lib: &Library) {
        if self.copy(lib) > 0 {
            self.delete_selected();
        }
    }

    /// Paste what was copied round the cursor (without one, a little beside where it was).
    pub fn paste(&mut self, lib: &Library) {
        let Some(clip) = self.clip.clone() else {
            self.toast(tr("Nothing copied yet", "Ещё ничего не скопировано"));
            return;
        };
        let at = match self.cursor {
            Some(c) => self.snap_point(c.truncate()),
            None => clip.at.truncate() + DVec2::splat(5.0),
        };
        self.paste_clip(&clip, at, tr("Paste", "Вставка"), lib);
    }

    /// Copy the selection and put the copy down beside it (the clipboard stays as it was).
    pub fn duplicate(&mut self, lib: &Library) {
        let Some(doc) = self.doc.as_ref() else { return };
        let clip = crate::clipboard::copy(doc, &self.selection(), |f| lib.find(f).map(|a| a.abs_height).unwrap_or(false));
        if clip.is_empty() {
            return;
        }
        let step = if self.snap_grid { self.grid_step.max(1.0) * (4.0 / self.grid_step.max(1.0)).ceil() } else { 4.0 };
        let at = clip.at.truncate() + DVec2::splat(step);
        self.paste_clip(&clip, at, tr("Duplicate", "Дублирование"), lib);
    }

    /// The ground under each selected object made level with its foot (where it stands at its
    /// middle), its outline and 3 m round it blending into the ground about. How many.
    pub fn level_under_objects(&mut self, assets: &mut Assets) -> usize {
        let sels = self.selection();
        let Some(doc) = self.doc.as_mut() else { return 0 };
        let mut done = 0;
        self.history.begin(tr("Level the ground", "Выравнивание земли"));
        for s in &sels {
            let Sel::Object(k, id) = *s else { continue };
            let Some(o) = doc.object(k, id).cloned() else { continue };
            let Some(model) = assets.object(&o.file) else { continue };
            if model.tree.is_some() {
                continue;
            }
            let centre = tile_origin(k) + DVec2::new(o.pos[0], o.pos[1]);
            let level = if model.abs_height { o.pos[2] } else { doc.ground(centre) as f64 };
            level_ground(doc, &mut self.history, centre, o.rot[0], (model.lo, model.hi), level);
            done += 1;
        }
        self.history.commit();
        done
    }

    /// The selected objects turned the way the nearest road runs where it passes them (within
    /// 60 m; the heading a row along that road would give them). How many were turned.
    pub fn align_to_roads(&mut self) -> usize {
        let sels = self.selection();
        let Some(doc) = self.doc.as_mut() else { return 0 };
        let mut turns: Vec<(TileKey, i64, f64)> = Vec::new();
        for s in &sels {
            let Sel::Object(k, id) = *s else { continue };
            let Some(o) = doc.object(k, id) else { continue };
            let p = tile_origin(k) + DVec2::new(o.pos[0], o.pos[1]);
            let mut best: Option<(f64, f64)> = None;
            for (sk, t) in &doc.tiles {
                let org = tile_origin(*sk);
                for sp in t.tile.splines.iter().filter(|x| !x.deleted) {
                    let start = org + DVec2::new(sp.pos[0], sp.pos[1]);
                    if (start - p).length() > sp.length + 60.0 {
                        continue;
                    }
                    let c = omsi_geometry::SplineCurve::from_map(sp, org);
                    let n = (sp.length.ceil() as usize).clamp(2, 2000);
                    for i in 0..=n {
                        let at = sp.length * i as f64 / n as f64;
                        let d = (c.point_at(at).truncate() - p).length();
                        if d < 60.0 && best.map(|b| d < b.0).unwrap_or(true) {
                            best = Some((d, c.heading_at(at)));
                        }
                    }
                }
            }
            if let Some((_, h)) = best {
                turns.push((k, id, roads::norm_deg(h)));
            }
        }
        if turns.is_empty() {
            return 0;
        }
        self.history.begin(tr("Turn objects", "Поворот объектов"));
        for (k, id, h) in &turns {
            self.history.touch_tile(doc, *k);
            if let Some(o) = doc.object_mut(*k, *id) {
                o.rot[0] = *h;
            }
            doc.tiles.get_mut(k).map(|t| t.touch_content());
        }
        self.history.commit();
        turns.len()
    }

    /// `n` copies of the selection in a row, each `step` (m) on from the one before and its
    /// objects turned `turn` degrees more (a fence, a row of houses, an avenue), in one step.
    /// All of them are selected.
    /// The selection copied `n` times a `step` on (each turned `turn` more), in `rows`
    /// rows a `row_step` apart (a grid: an orchard, a car park), one undo step.
    pub fn array(&mut self, lib: &Library, n: usize, step: DVec2, turn: f64, rows: usize, row_step: DVec2) {
        let sels = self.selection();
        let Some(doc) = self.doc.as_mut() else { return };
        let clip = crate::clipboard::copy(doc, &sels, |f| lib.find(f).map(|a| a.abs_height).unwrap_or(false));
        let rows = rows.max(1);
        if clip.is_empty() || (n == 0 && rows < 2) || (step.length() < 0.01 && turn.abs() < 1e-6 && rows < 2) {
            return;
        }
        self.history.begin(tr("Copies in a row", "Копии в ряд"));
        self.history.touch_global(doc);
        let mut all = sels.clone();
        let mut lost = 0;
        // (the original is the first of the first row)
        let places: Vec<(usize, DVec2)> = (0..rows).flat_map(|r| (0..=n).map(move |i| (i, row_step * r as f64 + step * i as f64))).filter(|(i, _)| *i > 0 || rows > 1).filter(|p| p.1.length() > 1e-9 || p.0 > 0).take(2000).collect();
        for (i, off) in places {
            let at2 = clip.at.truncate() + off;
            for k in crate::clipboard::tiles_touched(doc, &clip, at2) {
                self.history.touch_tile(doc, k);
            }
            let at = DVec3::new(at2.x, at2.y, doc.ground(at2));
            let got = crate::clipboard::paste(doc, &clip, at);
            lost += clip.len() - got.len();
            for s in &got {
                if let Sel::Object(k, id) = *s {
                    if let Some(o) = doc.object_mut(k, id) {
                        o.rot[0] = crate::roads::norm_deg(o.rot[0] + turn * i as f64);
                    }
                }
            }
            all.extend(got);
        }
        self.history.commit();
        if lost > 0 {
            self.toast(format!("{} {lost}", tr("Left out (off the map):", "Пропущено (за картой):")));
        }
        self.set_selection(all);
    }

    fn paste_clip(&mut self, clip: &crate::clipboard::Clip, at: DVec2, label: &str, lib: &Library) {
        let Some(doc) = self.doc.as_mut() else { return };
        let tiles = crate::clipboard::tiles_touched(doc, clip, at);
        if tiles.is_empty() {
            self.error(tr("There is no tile here: add one with the Tiles tool", "Здесь нет тайла: добавьте его инструментом «Тайлы»"));
            return;
        }
        self.history.begin(label);
        for k in tiles {
            self.history.touch_tile(doc, k);
        }
        self.history.touch_global(doc);
        let at = DVec3::new(at.x, at.y, doc.ground(at));
        let got = crate::clipboard::paste(doc, clip, at);
        // the ground comes to the roads pasted (the catalogue knows their widths)
        let roads: Vec<(TileKey, i64)> = got.iter().filter_map(|s| if let Sel::Spline(k, id) = s { Some((*k, *id)) } else { None }).collect();
        self.ground_to(&roads, &|f| lib.find(f).filter(|a| matches!(a.category, crate::library::Category::Roads | crate::library::Category::Tracks)).map(|a| a.half_width as f64).unwrap_or(0.0));
        self.history.commit();
        let n = got.len();
        if n < clip.len() {
            self.toast(format!("{} {n} / {}", tr("Pasted (the rest is off the map):", "Вставлено (остальное за картой):"), clip.len()));
        }
        self.set_selection(got);
    }

    // ------------------------------------------------------------ numbers

    /// A number in the properties changes: one step for as long as it is dragged or typed.
    pub fn begin_panel_step(&mut self, label: &str) {
        if !(self.panel_step && self.history.is_open()) {
            self.history.begin(label);
            self.panel_step = true;
        }
    }

    /// The properties' step ends once no number is being dragged or typed (a step the view
    /// opened, such as a move, is left alone).
    pub fn end_panel_step_if_idle(&mut self, ctx: &egui::Context) {
        if self.panel_step && self.drag == Drag::None && !ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input() {
            self.panel_step = false;
            self.history.commit();
        }
    }


    /// Move the selection by `delta` (m), as one step.
    pub fn move_selection_by(&mut self, delta: DVec2) {
        let sels = self.selection();
        let arms = self.arms_for(&sels);
        let Some(doc) = self.doc.as_mut() else { return };
        let from: Vec<Origin> = sels.iter().filter_map(|s| selection::origin_of(doc, *s)).collect();
        if from.is_empty() || delta.length() < 1e-6 {
            return;
        }
        self.history.begin(tr("Move", "Перемещение"));
        let now = selection::transform(doc, &mut self.history, &from, DVec2::ZERO, 0.0, delta);
        selection::unlink_outside(doc, &mut self.history, &now);
        self.set_selection(now);
        self.carry(&arms, DVec2::ZERO, 0.0, delta, 0.0);
        self.ground_to_selection();
        self.finish_step();
    }

    /// Put the object selected at a world position (the numbers in the properties).
    pub fn put_object(&mut self, k: TileKey, id: i64, to: DVec2) {
        let Some(o) = self.doc.as_ref().and_then(|d| selection::origin_of(d, Sel::Object(k, id))) else { return };
        self.begin_panel_step(tr("Edit object", "Изменение объекта"));
        let arms = self.arms_for(&[Sel::Object(k, id)]);
        let Some(doc) = self.doc.as_mut() else { return };
        let now = selection::transform(doc, &mut self.history, &[o], DVec2::ZERO, 0.0, to - o.pos);
        if let Some(s) = now.first() {
            if self.selected == Some(Sel::Object(k, id)) {
                self.selected = Some(*s);
            }
        }
        self.carry(&arms, DVec2::ZERO, 0.0, to - o.pos, 0.0);
        // (a number dragged moves the plate as it is: it is not made again every frame)
        self.pending_rebuild.clear();
    }

    /// Raise (or lower) the selection by `dz` metres: objects above their ground, splines'
    /// heights.
    /// The selected objects' headings: each a random one (`random`), or that of the one shown
    /// in the properties.
    /// The selected objects tilted to the ground under them (a fence or a shed on a slope),
    /// or stood upright again (`lean` false); things hanging on others are left alone.
    pub fn lean_objects(&mut self, lean: bool) {
        let sels = self.selection();
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.begin(if lean { tr("Lean with the ground", "Наклон по земле") } else { tr("Stand upright", "Поставить ровно") });
        for s in &sels {
            let Sel::Object(k, id) = *s else { continue };
            let Some(o) = doc.object(k, id) else { continue };
            let at = tile_origin(k) + DVec2::new(o.pos[0], o.pos[1]);
            let rot = if lean { lean_rotation(o.rot[0], &|p| doc.ground(p), at) } else { [o.rot[0], 0.0, 0.0] };
            self.history.touch_tile(doc, k);
            if let Some(t) = doc.tiles.get_mut(&k) {
                if let Some(o) = t.tile.objects.iter_mut().find(|o| o.id == id) {
                    o.rot = rot;
                }
                t.touch_content();
            }
        }
        self.history.commit();
    }

    pub fn turn_objects(&mut self, random: bool) {
        let sels = self.selection();
        let Some(doc) = self.doc.as_mut() else { return };
        let first = match self.selected {
            Some(Sel::Object(k, id)) => selection::any_object(doc, k, id).map(|o| o.rot[0]),
            _ => None,
        };
        if !random && first.is_none() {
            return;
        }
        self.history.begin(tr("Turn objects", "Поворот объектов"));
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0) ^ 0x9e37_79b9_7f4a_7c15;
        for s in &sels {
            let Sel::Object(k, id) = *s else { continue };
            self.history.touch_tile(doc, k);
            if let Some(o) = selection::any_object_mut(doc, k, id) {
                o.rot[0] = if random {
                    // (a hash of the IDCode: every object its own way)
                    let mut x = (id as u64).wrapping_mul(0x2545_f491_4f6c_dd1d) ^ seed;
                    x ^= x >> 33;
                    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
                    x ^= x >> 33;
                    (x % 36000) as f64 / 100.0
                } else {
                    first.unwrap_or(0.0)
                };
            }
            doc.tiles.get_mut(&k).map(|t| t.touch_content());
        }
        self.history.commit();
    }

    pub fn raise_selection(&mut self, dz: f64) {
        let sels = self.selection();
        let arms = self.arms_for(&sels);
        let Some(doc) = self.doc.as_mut() else { return };
        if sels.is_empty() || dz.abs() < 1e-9 {
            return;
        }
        self.history.begin(tr("Change height", "Изменение высоты"));
        for s in &sels {
            match *s {
                Sel::Object(k, id) => {
                    self.history.touch_tile(doc, k);
                    if let Some(o) = doc.object_mut(k, id) {
                        o.pos[2] += dz;
                    }
                    doc.tiles.get_mut(&k).map(|t| t.touch_content());
                }
                Sel::Spline(k, id) => {
                    self.history.touch_tile(doc, k);
                    if let Some(sp) = doc.tiles.get_mut(&k).and_then(|t| t.tile.splines.iter_mut().find(|x| x.id == id)) {
                        sp.pos[2] += dz;
                    }
                    doc.tiles.get_mut(&k).map(|t| t.touch_content());
                }
            }
        }
        // a road joined from outside the selection meets it again: its end there comes up too
        let ids: Vec<i64> = sels.iter().filter_map(|s| if let Sel::Spline(_, id) = s { Some(*id) } else { None }).collect();
        for s in &sels {
            let Sel::Spline(k, id) = *s else { continue };
            let Some(sp) = doc.spline(k, id).cloned() else { continue };
            let c = omsi_geometry::SplineCurve::from_map(&sp, tile_origin(k));
            for (at_end, link, z) in [(false, sp.prev_id, c.point_at(0.0).z), (true, sp.next_id, c.end_point().z)] {
                if link != 0 && !ids.contains(&link) {
                    selection::touch_spline(&mut self.history, doc, k, id);
                    reshape::set_end(doc, k, id, at_end, Some(z), None);
                }
            }
        }
        self.history.touch_global(doc);
        sync_entry_points(doc);
        self.carry(&arms, DVec2::ZERO, 0.0, DVec2::ZERO, dz);
        self.finish_step();
    }

    /// Give every selected spline another type.
    pub fn retype_splines(&mut self, file: &str) {
        let sels = self.selection();
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.begin(tr("Change road type", "Смена типа дороги"));
        for s in &sels {
            if let Sel::Spline(k, id) = *s {
                self.history.touch_tile(doc, k);
                if let Some(t) = doc.tiles.get_mut(&k) {
                    if let Some(sp) = t.tile.splines.iter_mut().find(|x| x.id == id) {
                        sp.file = file.to_string();
                    }
                    t.touch_content();
                }
            }
        }
        self.history.commit();
    }

    /// The selected splines, one chain, given one even slope from the height at one end to
    /// that at the other (a bridge, a ramp).
    pub fn even_selected_chain(&mut self) {
        let splines: Vec<(TileKey, i64)> = self.selection().into_iter().filter_map(|s| if let Sel::Spline(k, id) = s { Some((k, id)) } else { None }).collect();
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(chain) = reshape::chain_order(doc, &splines) else {
            self.error(tr("The selected roads are not one chain (they branch or leave gaps)", "Выбранные дороги не одна цепочка (ветвятся или с разрывами)"));
            return;
        };
        self.history.begin(tr("Even slope", "Ровный уклон"));
        for (k, _, _) in &chain {
            self.history.touch_tile(doc, *k);
        }
        reshape::even_chain(doc, &chain);
        self.history.commit();
    }

    /// The ground painted along the selected roads as `paint_along` says; how many texels.
    pub fn paint_along_selected(&mut self, lib: &Library) -> usize {
        let splines: Vec<(TileKey, i64)> = self.selection().into_iter().filter_map(|s| if let Sel::Spline(k, id) = s { Some((k, id)) } else { None }).collect();
        let a = self.paint_along;
        let Some(doc) = self.doc.as_mut() else { return 0 };
        crate::roadpaint::paint_along(doc, &mut self.history, &splines, |f| lib.find(f).map(|x| x.half_width as f64).unwrap_or(3.0), &a)
    }

    /// The selected roads laid on the ground again, the ground then shaped to them (with
    /// "Shape the ground" on).
    pub fn lay_selected_on_ground(&mut self) {
        let sels = self.selection();
        let Some(doc) = self.doc.as_ref() else { return };
        if !sels.iter().any(|s| matches!(s, Sel::Spline(..))) {
            return;
        }
        self.history.begin(tr("Lay on the ground", "Положить на землю"));
        for s in &sels {
            if let Sel::Spline(k, id) = s {
                self.history.touch_tile(doc, *k);
                if let Some(sp) = doc.spline(*k, *id) {
                    for l in [sp.prev_id, sp.next_id] {
                        if let Some((nk, _)) = doc.find_spline(l) {
                            self.history.touch_tile(doc, nk);
                        }
                    }
                }
            }
        }
        let Some(doc) = self.doc.as_mut() else { return };
        crate::onground::lay(doc, &sels);
        self.ground_to_selection();
        self.history.commit();
    }

    /// The selected splines turned round (each starts where it ended), the traffic on them
    /// going the other way: a one-way street, a track's direction.
    pub fn reverse_selected(&mut self) {
        let splines: Vec<(TileKey, i64)> = self.selection().into_iter().filter_map(|s| if let Sel::Spline(k, id) = s { Some((k, id)) } else { None }).collect();
        let Some(doc) = self.doc.as_mut() else { return };
        if splines.is_empty() {
            return;
        }
        // (a row of objects is measured along its spline from the start: it would move)
        let with_rows = splines.iter().any(|(k, id)| {
            let Some(t) = doc.tiles.get(k) else { return false };
            let idx = t.tile.splines.iter().position(|s| s.id == *id && !s.deleted);
            idx.map(|i| t.tile.spline_attachments.iter().any(|a| a.spline_index == i as i32)).unwrap_or(false)
        });
        if with_rows {
            self.error(tr("Remove the rows of objects along it first (they are measured from its start)", "Сначала уберите ряды объектов вдоль неё (они отмеряются от её начала)"));
            return;
        }
        // the textures run on along a chain the new way
        let mut tex: std::collections::HashMap<i64, f64> = Default::default();
        if let Some(chain) = reshape::chain_order(doc, &splines) {
            let mut at = 0.0;
            for (k, id, _) in chain.iter().rev() {
                tex.insert(*id, at);
                at += doc.spline(*k, *id).map(|s| s.length).unwrap_or(0.0);
            }
        }
        self.history.begin(tr("Reverse direction", "Разворот направления"));
        for (k, id) in &splines {
            self.history.touch_tile(doc, *k);
            let org = tile_origin(*k);
            let t = doc.tiles.get_mut(k).unwrap();
            if let Some(s) = t.tile.splines.iter_mut().find(|s| s.id == *id && !s.deleted) {
                let at = tex.get(id).copied().unwrap_or(s.tex_offset);
                *s = reshape::reversed(s, org, at);
                t.touch_content();
            }
        }
        self.history.commit();
    }

    /// The knife on or off (K); on, the Select tool takes over and the ruler goes.
    pub fn toggle_cutting(&mut self) {
        let on = !self.cutting;
        self.set_tool(Tool::Select);
        self.ruler = None;
        self.cutting = on;
    }

    /// Where a click of the knife would cut: the road under the cursor, at the point nearest
    /// it (on whole grid steps along the road with grid snapping on).
    pub fn cut_preview(&self) -> Option<CutPreview> {
        if !self.cutting || !self.cursor_in_view {
            return None;
        }
        let (k, id, at) = self.road_at(self.cursor?.truncate())?;
        let s = self.doc.as_ref()?.spline(k, id)?;
        let at = if self.snap_grid && self.grid_step > 0.0 { ((at / self.grid_step).round() * self.grid_step).clamp(0.0, s.length) } else { at };
        let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(k));
        let h = c.heading_at(at).to_radians();
        Some(CutPreview { key: k, id, at, length: s.length, pos: c.point_at(at), dir: DVec2::new(h.sin(), h.cos()), fits: at >= 1.0 && at <= s.length - 1.0 })
    }

    /// The road or track nearest to `p` (within a few metres of its middle): tile, id and the
    /// distance along it.
    pub fn road_at(&self, p: DVec2) -> Option<(TileKey, i64, f64)> {
        crate::switches::spline_at(self.doc.as_ref()?, p, 8.0)
    }

    /// The road at `p` cut in two there (to give a stretch of it another type, a bridge, a
    /// junction later); both pieces selected.
    pub fn cut_road_at(&mut self, p: DVec2) {
        let Some((k, id, at)) = self.road_at(p) else {
            self.error(tr("No road there to cut", "Здесь нет дороги, которую можно разрезать"));
            return;
        };
        self.cut_spline(k, id, at);
    }

    /// Spline `id` cut in two `at` m along it; both pieces selected.
    pub fn cut_spline(&mut self, k: TileKey, id: i64, at: f64) {
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(s) = doc.spline(k, id) else { return };
        if at < 1.0 || at > s.length - 1.0 {
            self.error(tr("Too near the road's end to cut it there", "Слишком близко к концу дороги, чтобы разрезать её здесь"));
            return;
        }
        self.history.begin(tr("Cut the road", "Разрезать дорогу"));
        self.history.touch_tile(doc, k);
        if let Some((nk, _)) = doc.find_spline(s.next_id) {
            self.history.touch_tile(doc, nk);
        }
        let Some(new) = self.doc.as_mut().unwrap().split_spline(k, id, at) else {
            self.history.cancel();
            return;
        };
        self.history.commit();
        self.set_selection(vec![Sel::Spline(k, id), Sel::Spline(k, new)]);
        self.toast(tr("Cut in two: each piece can now be changed on its own", "Разрезана надвое: каждый кусок теперь можно менять отдельно"));
    }

    /// Spline `id` and the one after it made one spline again, where they make one curve.
    pub fn join_with_next(&mut self, k: TileKey, id: i64) {
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(s) = doc.spline(k, id) else { return };
        self.history.begin(tr("Join the pieces", "Соединить куски"));
        self.history.touch_tile(doc, k);
        if let Some((nk, n)) = doc.find_spline(s.next_id) {
            self.history.touch_tile(doc, nk);
            if let Some((ck, _)) = doc.find_spline(n.next_id) {
                self.history.touch_tile(doc, ck);
            }
        }
        match self.doc.as_mut().unwrap().join_splines(k, id) {
            Ok(()) => {
                self.history.commit();
                self.set_selection(vec![Sel::Spline(k, id)]);
                self.toast(tr("Joined into one piece", "Соединены в один кусок"));
            }
            Err(why) => {
                self.history.cancel();
                self.error(why);
            }
        }
    }

    /// The selected splines copied alongside, `d` m to the right of the first's way
    /// (negative: left); the copies are selected.
    pub fn parallel_copy(&mut self, d: f64) {
        let splines: Vec<(TileKey, i64)> = self.selection().into_iter().filter_map(|s| if let Sel::Spline(k, id) = s { Some((k, id)) } else { None }).collect();
        let Some(doc) = self.doc.as_mut() else { return };
        if splines.is_empty() {
            return;
        }
        self.history.begin(tr("Parallel copy", "Параллельная копия"));
        let keys: Vec<TileKey> = doc.tiles.keys().copied().collect();
        for k in keys {
            self.history.touch_tile(doc, k);
        }
        self.history.touch_global(doc);
        match crate::parallel::copy(doc, &splines, d) {
            Ok(made) => {
                self.history.commit();
                self.set_selection(made.into_iter().map(|(k, id)| Sel::Spline(k, id)).collect());
            }
            Err(e) => {
                self.history.cancel();
                self.error(e);
            }
        }
    }

    /// A spline of type `wire` laid on each selected one (a track's overhead wire: its height
    /// is in the wire's own profile), the pieces linked as the track's are; one undo step.
    /// How many.
    pub fn wire_along(&mut self, wire: &str) -> usize {
        let splines: Vec<(TileKey, i64)> = self.selection().into_iter().filter_map(|s| if let Sel::Spline(k, id) = s { Some((k, id)) } else { None }).collect();
        let Some(doc) = self.doc.as_mut() else { return 0 };
        if splines.is_empty() {
            return 0;
        }
        self.history.begin(tr("Overhead wire", "Контактный провод"));
        self.history.touch_global(doc);
        let ids: std::collections::HashMap<i64, i64> = splines.iter().map(|(_, id)| (*id, doc.new_id())).collect();
        let link = |id: i64| ids.get(&id).copied().unwrap_or(0);
        for (k, id) in &splines {
            let Some(s) = doc.spline(*k, *id).cloned() else { continue };
            self.history.touch_tile(doc, *k);
            // (a wire cuts no hole in the ground under it)
            let w = MapSpline { file: wire.to_string(), id: ids[id], prev_id: link(s.prev_id), next_id: link(s.next_id), mirror: false, terrain_align: None, ..s };
            let t = doc.tiles.get_mut(k).unwrap();
            t.tile.splines.push(w);
            t.touch_content();
        }
        self.history.commit();
        ids.len()
    }

    /// Every object (or spline) of the same type as `s` on the map, selected; `s` stays the
    /// one in the properties. How many.
    pub fn select_alike(&mut self, s: Sel) -> usize {
        let Some(doc) = self.doc.as_ref() else { return 0 };
        let file = match s {
            Sel::Object(k, id) => selection::any_object(doc, k, id).map(|o| o.file.clone()),
            Sel::Spline(k, id) => doc.spline(k, id).map(|x| x.file.clone()),
        };
        let Some(file) = file else { return 0 };
        let mut v = vec![s];
        for (k, t) in &doc.tiles {
            match s {
                Sel::Object(..) => v.extend(t.tile.objects.iter().chain(&t.tile.attach_objects).filter(|o| o.file.eq_ignore_ascii_case(&file)).map(|o| Sel::Object(*k, o.id)).filter(|x| *x != s)),
                Sel::Spline(..) => v.extend(t.tile.splines.iter().filter(|x| !x.deleted && x.file.eq_ignore_ascii_case(&file)).map(|x| Sel::Spline(*k, x.id)).filter(|x| *x != s)),
            }
        }
        let n = v.len();
        self.set_selection(v);
        n
    }

    /// Objects and splines a chrono scenario had, put into the map's tile `k` (they are the
    /// map's again, from the first day on). Inside one step.
    pub fn put_back(&mut self, k: TileKey, objects: Vec<MapObject>, hanging: Vec<MapObject>, splines: Vec<MapSpline>) {
        let Some(doc) = self.doc.as_mut() else { return };
        if !doc.tiles.contains_key(&k) {
            return;
        }
        self.history.begin(tr("Back into the map", "Обратно в карту"));
        self.history.touch_tile(doc, k);
        let t = doc.tiles.get_mut(&k).unwrap();
        t.tile.objects.extend(objects);
        t.tile.attach_objects.extend(hanging);
        t.tile.splines.extend(splines);
        t.touch_content();
        self.history.commit();
    }

    /// The object `file` hung on point `at` of object `id` (`[attachObj]`); it is selected.
    pub fn attach_object(&mut self, k: TileKey, parent: i64, at: usize, file: &str) {
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.begin(tr("Hang an object", "Подвесить объект"));
        self.history.touch_tile(doc, k);
        self.history.touch_global(doc);
        let id = doc.new_id();
        if let Some(t) = doc.tiles.get_mut(&k) {
            t.tile.attach_objects.push(MapObject { file: file.to_string(), id, parent_id: Some(parent), attach_index: at, ..Default::default() });
            t.touch_content();
        }
        self.history.commit();
        self.select(Some(Sel::Object(k, id)));
    }

    /// Object `id` takes its state from object `parent` (`[varparent]`: a signal head from
    /// its crossing's light program, a stop's display from its stop), or from none.
    pub fn set_var_parent(&mut self, k: TileKey, id: i64, parent: Option<i64>) {
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.begin(if parent.is_some() { tr("Link to an object", "Связать с объектом") } else { tr("Unlink", "Отвязать") });
        self.history.touch_tile(doc, k);
        if let Some(o) = selection::any_object_mut(doc, k, id) {
            o.var_parent = parent;
        }
        if let Some(t) = doc.tiles.get_mut(&k) {
            t.touch_content();
        }
        self.history.commit();
    }

    /// The object clicked while linking: `id` takes its state from it, unless that would
    /// close a ring (it takes its own state, through others, from `id`).
    fn link_to(&mut self, k: TileKey, id: i64, parent: i64) {
        let Some(doc) = self.doc.as_ref() else { return };
        let mut at = Some(parent);
        for _ in 0..64 {
            match at {
                Some(p) if p == id => {
                    self.toast(tr("That object already takes its state from this one", "Тот объект сам берёт состояние от этого"));
                    return;
                }
                Some(p) => at = selection::var_parent_of(doc, p),
                None => break,
            }
        }
        self.set_var_parent(k, id, Some(parent));
        self.toast(tr("Linked", "Связано"));
    }

    /// Which light of its parent's program a signal head shows (its first label).
    pub fn set_light_index(&mut self, k: TileKey, id: i64, light: usize) {
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.begin(tr("Signal head's light", "Сигнал светофора"));
        self.history.touch_tile(doc, k);
        if let Some(o) = selection::any_object_mut(doc, k, id) {
            match o.extra.first_mut() {
                Some(l) => *l = light.to_string(),
                None => o.extra.push(light.to_string()),
            }
            o.flag = o.flag.max(o.extra.len() as i32);
        }
        if let Some(t) = doc.tiles.get_mut(&k) {
            t.touch_content();
        }
        self.history.commit();
    }

    /// The rules of an object's or a spline's paths (one step while a number is dragged).
    pub fn set_rules(&mut self, sel: Sel, rules: Vec<omsi_map::MapRule>) {
        self.begin_panel_step(tr("Path rules", "Правила путей"));
        let Some(doc) = self.doc.as_mut() else { return };
        let k = match sel {
            Sel::Object(k, _) | Sel::Spline(k, _) => k,
        };
        self.history.touch_tile(doc, k);
        match sel {
            Sel::Object(k, id) => {
                if let Some(o) = selection::any_object_mut(doc, k, id) {
                    o.rules = rules;
                }
            }
            Sel::Spline(k, id) => {
                if let Some(s) = doc.tiles.get_mut(&k).and_then(|t| t.tile.splines.iter_mut().find(|s| s.id == id && !s.deleted)) {
                    s.rules = rules;
                }
            }
        }
        doc.tiles.get_mut(&k).map(|t| t.touch_content());
    }

    /// The selected objects made ones of the type `file`, where they stand.
    pub fn replace_selected_objects(&mut self, file: &str) {
        let sels = self.selection();
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.begin(tr("Replace objects", "Замена объектов"));
        for s in &sels {
            let Sel::Object(k, id) = *s else { continue };
            self.history.touch_tile(doc, k);
            if let Some(o) = selection::any_object_mut(doc, k, id) {
                o.file = file.to_string();
            }
            doc.tiles.get_mut(&k).map(|t| t.touch_content());
        }
        self.history.commit();
    }

    /// Every object (or spline) of the type `from` made one of `to`, in one step. How many.
    pub fn replace_everywhere(&mut self, from: &str, to: &str, splines: bool) -> usize {
        let Some(doc) = self.doc.as_mut() else { return 0 };
        self.history.begin(if splines { tr("Change road type", "Смена типа дороги") } else { tr("Replace objects", "Замена объектов") });
        let mut n = 0;
        let keys: Vec<TileKey> = doc.tiles.keys().copied().collect();
        for k in keys {
            let t = &doc.tiles[&k];
            let here = if splines { t.tile.splines.iter().filter(|s| !s.deleted && s.file.eq_ignore_ascii_case(from)).count() } else { t.tile.objects.iter().chain(t.tile.attach_objects.iter()).filter(|o| o.file.eq_ignore_ascii_case(from)).count() };
            if here == 0 {
                continue;
            }
            self.history.touch_tile(doc, k);
            let t = doc.tiles.get_mut(&k).unwrap();
            if splines {
                for s in t.tile.splines.iter_mut().filter(|s| !s.deleted && s.file.eq_ignore_ascii_case(from)) {
                    s.file = to.to_string();
                }
            } else {
                for o in t.tile.objects.iter_mut().chain(t.tile.attach_objects.iter_mut()).filter(|o| o.file.eq_ignore_ascii_case(from)) {
                    o.file = to.to_string();
                }
            }
            t.touch_content();
            n += here;
        }
        self.history.commit();
        n
    }

    /// Height and gradient at one end of a spline (the properties); the neighbour linked
    /// there follows. Kept open as one step while a number is dragged.
    pub fn set_spline_end(&mut self, k: TileKey, id: i64, at_end: bool, z: Option<f64>, g: Option<f64>) {
        self.begin_panel_step(tr("Edit road", "Изменение дороги"));
        let Some(doc) = self.doc.as_mut() else { return };
        selection::touch_spline(&mut self.history, doc, k, id);
        reshape::set_end(doc, k, id, at_end, z, g);
    }

    /// A road's length and radius by numbers: its start stays, the road joined at its end
    /// follows, tangent, its far end kept.
    fn set_spline_shape(&mut self, assets: &mut Assets, k: TileKey, id: i64, length: f64, radius: f64) {
        let joined = self.helper_anchors(assets);
        let locked = |q: DVec2| joined.iter().any(|j| (j.truncate() - q).length() < 0.6);
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(s) = doc.spline(k, id).cloned() else { return };
        let org = tile_origin(k);
        let old = omsi_geometry::SplineCurve::from_map(&s, org);
        let mut new = s.clone();
        new.length = length;
        new.radius = radius;
        let c = omsi_geometry::SplineCurve::from_map(&new, org);
        let p = c.end_point();
        let mut plan = None;
        if let Some((nk, n)) = doc.find_spline(s.next_id) {
            let at_end = if n.prev_id == id { false } else if n.next_id == id { true } else { return };
            match reshape::carry_end(doc, nk, n.id, at_end, p.truncate(), crate::roads::norm_deg(c.heading_at(length) + 180.0), p.z - old.end_point().z, &locked) {
                Ok(pl) => plan = Some(pl),
                Err(e) => {
                    self.toast(e);
                    return;
                }
            }
        }
        self.begin_panel_step(tr("Edit road", "Изменение дороги"));
        let Some(doc) = self.doc.as_mut() else { return };
        selection::touch_spline(&mut self.history, doc, k, id);
        if let Some(t) = doc.tiles.get_mut(&k) {
            if let Some(x) = t.tile.splines.iter_mut().find(|x| x.id == id && !x.deleted) {
                x.length = length;
                x.radius = radius;
            }
            t.touch_content();
        }
        if let Some(plan) = plan {
            self.apply_plan(&plan);
        }
    }

    /// A spline's skew at its ends and how it cuts the ground (0: not, 1: under it, 2–4: the
    /// hole held at the end, the start, both: `[spline_terrain_align_2]`).
    pub fn set_spline_extra(&mut self, k: TileKey, id: i64, skew_start: f64, skew_end: f64, ground: u8) {
        self.begin_panel_step(tr("Edit road", "Изменение дороги"));
        let Some(doc) = self.doc.as_mut() else { return };
        selection::touch_spline(&mut self.history, doc, k, id);
        let Some(t) = doc.tiles.get_mut(&k) else { return };
        if let Some(s) = t.tile.splines.iter_mut().find(|s| s.id == id && !s.deleted) {
            s.skew_start = skew_start;
            s.skew_end = skew_end;
            s.terrain_align_flag = ground == 1;
            s.terrain_align = (ground >= 2).then_some(ground as f64);
        }
        t.touch_content();
    }

    /// A road's cant (%) at one end; the road joined there follows.
    pub fn set_spline_cant(&mut self, k: TileKey, id: i64, at_end: bool, cant: f64) {
        self.begin_panel_step(tr("Edit road", "Изменение дороги"));
        let Some(doc) = self.doc.as_mut() else { return };
        selection::touch_spline(&mut self.history, doc, k, id);
        reshape::set_cant(doc, k, id, at_end, cant);
    }

    // ------------------------------------------------------------ junctions and ground

    /// Learn the types of what is selected, as their models come in.
    fn learn(&mut self, assets: &mut Assets) {
        let Some(doc) = self.doc.as_ref() else { return };
        let sels = self.selection();
        if sels.len() > 5000 {
            return;
        }
        let mut splines: Vec<String> = Vec::new();
        for s in &sels {
            match *s {
                Sel::Object(k, id) => {
                    let Some(o) = doc.object(k, id) else { continue };
                    let key = o.file.to_lowercase();
                    if !self.known_helpers.contains_key(&key) {
                        if let Some(m) = assets.object(&o.file) {
                            self.known_helpers.insert(key.clone(), (m.abs_height, m.helpers.iter().map(|h| h.0).collect()));
                        }
                    }
                    // (and the roads it holds)
                    if let Some((abs, h)) = self.known_helpers.get(&key).filter(|h| !h.1.is_empty()) {
                        for a in selection::arms_of(doc, &selection::helper_points(doc, k, id, *abs, h)) {
                            if let Some((ak, aid, _)) = selection::free_end_at(doc, a.pos.truncate()) {
                                if let Some(sp) = doc.spline(ak, aid) {
                                    splines.push(sp.file.clone());
                                }
                            }
                        }
                    }
                }
                Sel::Spline(k, id) => {
                    if let Some(sp) = doc.spline(k, id) {
                        splines.push(sp.file.clone());
                        // (and the junctions at its ends, which a delete makes again)
                        let c = omsi_geometry::SplineCurve::from_map(sp, tile_origin(k));
                        let ends = [c.point_at(0.0).truncate(), c.end_point().truncate()];
                        for (jk, t) in &doc.tiles {
                            for o in t.tile.objects.iter().filter(|o| is_own_junction(&o.file)) {
                                let key = o.file.to_lowercase();
                                if self.known_helpers.contains_key(&key) {
                                    continue;
                                }
                                let at = tile_origin(*jk) + DVec2::new(o.pos[0], o.pos[1]);
                                if ends.iter().any(|e| (*e - at).length() < 80.0) {
                                    if let Some(m) = assets.object(&o.file) {
                                        self.known_helpers.insert(key, (m.abs_height, m.helpers.iter().map(|h| h.0).collect()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        for f in splines {
            let key = f.to_lowercase();
            if !self.known_half.contains_key(&key) {
                if let Some(m) = assets.spline(&f) {
                    self.known_half.insert(key, if m.joinable() { m.half_width as f64 } else { 0.0 });
                }
            }
        }
    }

    /// Half the width of a spline type the ground is brought to (0: left alone).
    fn half_of(&self, file: &str) -> f64 {
        self.known_half.get(&file.to_lowercase()).copied().unwrap_or(0.0)
    }

    /// The road ends the selected junctions hold (not those of roads selected with them,
    /// which move along).
    fn arms_for(&self, sels: &[Sel]) -> Vec<(Sel, Vec<selection::Arm>)> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let ids: Vec<i64> = sels.iter().filter_map(|s| if let Sel::Spline(_, id) = s { Some(*id) } else { None }).collect();
        let mut out = Vec::new();
        for s in sels {
            let Sel::Object(k, id) = *s else { continue };
            let Some(o) = doc.object(k, id) else { continue };
            let Some((abs, h)) = self.known_helpers.get(&o.file.to_lowercase()).filter(|h| !h.1.is_empty()) else { continue };
            let arms: Vec<selection::Arm> = selection::arms_of(doc, &selection::helper_points(doc, k, id, *abs, h))
                .into_iter()
                .filter(|a| selection::free_end_at(doc, a.pos.truncate()).map(|e| !ids.contains(&e.1)).unwrap_or(false))
                .collect();
            if !arms.is_empty() {
                out.push((*s, arms));
            }
        }
        out
    }

    /// The roads junctions hold follow them (turned by `deg` round `pivot`, shifted by
    /// `delta`, raised by `dz`), and the ground comes to the junctions' plates. Inside an open
    /// history step.
    fn carry(&mut self, groups: &[(Sel, Vec<selection::Arm>)], pivot: DVec2, deg: f64, delta: DVec2, dz: f64) {
        let mut failed = 0;
        for (_, arms) in groups {
            for a in arms {
                // (one at a time against the map as it now is: two arms may be one road)
                let plan = self.doc.as_ref().and_then(|d| selection::carry_plans(d, &[*a], pivot, deg, delta, dz, &|_| false).pop());
                match plan {
                    Some(Ok(p)) => self.apply_plan(&p),
                    Some(Err(_)) => failed += 1,
                    None => {}
                }
            }
        }
        if self.follow_ground && (delta.length() > 1e-6 || deg.abs() > 1e-9 || dz.abs() > 1e-9) {
            for (sel, arms) in groups {
                let Sel::Object(_, id) = *sel else { continue };
                let Some(doc) = self.doc.as_mut() else { return };
                let Some((k, o)) = doc.tiles.iter().find_map(|(k, t)| t.tile.objects.iter().find(|o| o.id == id).map(|o| (*k, o.clone()))) else { continue };
                let abs = self.known_helpers.get(&o.file.to_lowercase()).map(|h| h.0).unwrap_or(false);
                let centre = doc.object_world(k, &o, abs);
                let ends: Vec<DVec3> = arms.iter().map(|a| (selection::turn_point(a.pos.truncate(), pivot, deg) + delta).extend(a.pos.z + dz)).collect();
                let r = ends.iter().map(|e| (e.truncate() - centre.truncate()).length()).fold(3.0, f64::max) * 1.05;
                self.touch_around(&[centre.truncate()], r + 9.0);
                let Some(doc) = self.doc.as_mut() else { return };
                conform_disc(doc, centre, r, &ends);
            }
        }
        if delta.length() > 1e-6 || deg.abs() > 1e-9 || dz.abs() > 1e-9 {
            for (sel, _) in groups {
                if let Sel::Object(_, id) = *sel {
                    if self.doc.as_ref().and_then(|d| d.tiles.values().find_map(|t| t.tile.objects.iter().find(|o| o.id == id))).map(|o| is_own_junction(&o.file)).unwrap_or(false) {
                        self.pending_rebuild.push(id);
                    }
                }
            }
        }
        if failed > 0 {
            self.error(format!("{} {failed}", tr("Roads that could not follow the junction (too sharp a bend):", "Дороги, которые не смогли последовать за перекрёстком (слишком крутой изгиб):")));
        }
    }

    /// Close the open step, unless junction plates are still to be made again in it (the
    /// next tick does that and closes it).
    fn finish_step(&mut self) {
        if self.pending_rebuild.is_empty() {
            self.history.commit();
        }
    }

    /// openMapper's junctions (known ones) holding an end of a spline in `sels`, not
    /// themselves among them.
    fn junctions_at_ends(&self, sels: &[Sel]) -> Vec<i64> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let mut ends: Vec<DVec2> = Vec::new();
        for s in sels {
            if let Sel::Spline(k, id) = *s {
                if let Some(sp) = doc.spline(k, id) {
                    let c = omsi_geometry::SplineCurve::from_map(sp, tile_origin(k));
                    ends.extend([c.point_at(0.0).truncate(), c.end_point().truncate()]);
                }
            }
        }
        let mut out = Vec::new();
        for (k, t) in &doc.tiles {
            for o in t.tile.objects.iter().filter(|o| is_own_junction(&o.file) && !sels.contains(&Sel::Object(*k, o.id))) {
                let Some((abs, h)) = self.known_helpers.get(&o.file.to_lowercase()) else { continue };
                let pts = selection::helper_points(doc, *k, o.id, *abs, h);
                if pts.iter().any(|(p, _)| ends.iter().any(|e| (*e - p.truncate()).length() < 0.6)) {
                    out.push(o.id);
                }
            }
        }
        out
    }

    /// openMapper's junction plate a point lies on: (its IDCode, centre, radius out to its
    /// arms' ends).
    fn plate_at(&self, assets: &mut Assets, p: DVec2) -> Option<(i64, DVec3, f64)> {
        let doc = self.doc.as_ref()?;
        for (k, t) in &doc.tiles {
            for o in t.tile.objects.iter().filter(|o| is_own_junction(&o.file)) {
                let at = tile_origin(*k) + DVec2::new(o.pos[0], o.pos[1]);
                if (at - p).length() > 80.0 {
                    continue;
                }
                let Some(m) = assets.object(&o.file) else { continue };
                let r = m.helpers.iter().map(|h| DVec2::new(h.0[0] as f64, h.0[1] as f64).length()).fold(0.0, f64::max);
                if r > 0.0 && (at - p).length() < r * 0.85 {
                    return Some((o.id, doc.object_world(*k, o, m.abs_height), r));
                }
            }
        }
        None
    }

    /// The headings out of a junction of the roads it holds (its known helpers).
    pub fn junction_arms(&self, k: TileKey, id: i64) -> Vec<f64> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let Some(o) = doc.object(k, id) else { return Vec::new() };
        let Some((abs, h)) = self.known_helpers.get(&o.file.to_lowercase()) else { return Vec::new() };
        selection::arms_of(doc, &selection::helper_points(doc, k, id, *abs, h)).iter().map(|a| a.out).collect()
    }

    /// Change a junction's lights and crossings: it is made again with them.
    pub fn set_junction_options(&mut self, id: i64, opts: crate::junction::Options) {
        if self.doc.is_none() {
            return;
        }
        self.history.begin(tr("Edit junction", "Изменение перекрёстка"));
        self.junction_options.insert(id, opts.clone());
        self.pending_options.insert(id, opts);
        self.pending_rebuild.push(id);
    }

    /// After a save: the junction files of this map that nothing uses any more (not a tile,
    /// not a tile undo or redo may bring back, not an earlier version kept by a save, and not
    /// one made in this session) are deleted.
    /// Returns how many went.
    pub fn clean_junction_files(&self) -> usize {
        let (Some(doc), Some(content)) = (self.doc.as_ref(), self.content_dir.as_ref()) else { return 0 };
        let mut used: std::collections::HashSet<String> = doc
            .tiles
            .values()
            .flat_map(|t| t.tile.objects.iter().chain(t.chrono.iter().flat_map(|c| c.objects.iter())))
            .map(|o| o.file.as_str())
            // (and those undo or redo can bring back)
            .chain(self.history.object_files())
            .filter(|f| is_own_junction(f))
            .map(|f| f.to_lowercase())
            .collect();
        // (and those the earlier versions kept by saves name: going back to one brings them back)
        for b in crate::doc::safe::backups(&doc.dir) {
            for e in std::fs::read_dir(&b).into_iter().flatten().flatten() {
                if !e.file_name().to_string_lossy().to_lowercase().ends_with(".map") {
                    continue;
                }
                let Ok(bytes) = std::fs::read(e.path()) else { continue };
                used.extend(omsi_cfg::decode_text(&bytes).lines().map(str::trim).filter(|l| is_own_junction(l)).map(str::to_lowercase));
            }
        }
        let folder: String = doc.dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "map".into()).chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let dir = crate::doc::safe::own_path(content, &format!("Sceneryobjects\\openMapper\\{folder}"));
        crate::junction::remove_unused(&dir, &folder, &used, self.session_first_id)
    }

    /// One of openMapper's junction plates made again at the next tick (its file lost: the
    /// map moved without it); false if it is not on the map.
    pub fn remake_junction(&mut self, id: i64) -> bool {
        let Some(doc) = self.doc.as_ref() else { return false };
        if !doc.tiles.values().any(|t| t.tile.objects.iter().any(|o| o.id == id && is_own_junction(&o.file))) {
            return false;
        }
        self.history.begin(tr("Make the junction again", "Перекрёсток заново"));
        self.pending_rebuild.push(id);
        true
    }

    /// Every junction plate of openMapper's made again at the next tick, for the roads it has
    /// now (with the newest asphalt and kerbs); one undo step. How many.
    pub fn remake_all_junctions(&mut self) -> usize {
        let Some(doc) = self.doc.as_ref() else { return 0 };
        let ids: Vec<i64> = doc.tiles.values().flat_map(|t| t.tile.objects.iter().filter(|o| is_own_junction(&o.file)).map(|o| o.id)).collect();
        if !ids.is_empty() {
            self.history.begin(tr("Make the junctions again", "Перестроить перекрёстки"));
            self.pending_rebuild.extend(ids.iter().copied());
        }
        ids.len()
    }

    /// Make the queued junctions' plates again for the arms they have now.
    fn rebuild_pending(&mut self, assets: &mut Assets) {
        let ids = std::mem::take(&mut self.pending_rebuild);
        for id in ids {
            self.rebuild_junction(assets, id);
        }
    }

    /// One of openMapper's junctions made again in place for the roads now ending at its
    /// helper points: three or more, a new plate; two, a smooth joint between them; fewer,
    /// nothing. Inside an open history step.
    fn rebuild_junction(&mut self, assets: &mut Assets, id: i64) {
        let Some(content) = self.content_dir.clone() else { return };
        let asphalt = self.asphalt.clone();
        let was_selected = self.selection().iter().any(|s| matches!(s, Sel::Object(_, x) if *x == id));
        let Some(doc) = self.doc.as_mut() else { return };
        let Some((jk, o)) = doc.tiles.iter().find_map(|(k, t)| t.tile.objects.iter().find(|o| o.id == id).map(|o| (*k, o.clone()))) else { return };
        let mut arms: Vec<crate::junction::Arm> = Vec::new();
        // (a plate whose file is gone, a map moved without them: the roads that end facing
        // its middle are its arms; it stood at its own height)
        let abs = match plate_helpers(&content, assets, &o.file) {
            Some((abs, helpers)) => {
                let pts = selection::helper_points(doc, jk, id, abs, &helpers);
                for a in selection::arms_of(doc, &pts) {
                    if let Some((k, sid, at_end)) = selection::free_end_at(doc, a.pos.truncate()) {
                        if !arms.iter().any(|x| x.id == sid && x.at_end == at_end) {
                            arms.push(crate::junction::Arm { key: k, id: sid, at_end });
                        }
                    }
                }
                abs
            }
            None if is_own_junction(&o.file) => {
                arms = ends_facing(doc, tile_origin(jk) + DVec2::new(o.pos[0], o.pos[1]), 30.0);
                true
            }
            None => return,
        };
        for a in self.pending_arms.remove(&id).unwrap_or_default() {
            if doc.spline(a.key, a.id).is_some() && !arms.iter().any(|x| x.id == a.id && x.at_end == a.at_end) {
                arms.push(a);
            }
        }
        let opts = self.pending_options.remove(&id).unwrap_or_else(|| crate::junction::Options::read(&omsi_cfg::resolve_path(&content, &o.file)));
        let centre = doc.object_world(jk, &o, abs);
        self.history.touch_tile(doc, jk);
        for a in &arms {
            self.history.touch_tile(doc, a.key);
        }
        self.history.touch_global(doc);
        // its signal heads go with it (new ones are put up for the new plate)
        for (k, c) in crate::junction::children(doc, id) {
            self.history.touch_tile(doc, k);
            doc.delete_object(k, c);
        }
        doc.delete_object(jk, id);
        self.junction_options.remove(&id);
        match arms.len() {
            n if n >= 3 => match crate::junction::make_with(doc, assets, &content, asphalt.as_deref(), centre, &arms, &opts) {
                Ok(new) => {
                    if was_selected {
                        let k = doc.tiles.iter().find(|(_, t)| t.tile.objects.iter().any(|o| o.id == new)).map(|(k, _)| *k).unwrap_or(jk);
                        let v: Vec<Sel> = self.selection().into_iter().map(|s| if s == Sel::Object(jk, id) { Sel::Object(k, new) } else { s }).collect();
                        self.set_selection(v);
                    }
                }
                Err(e) => {
                    // (the plate as it was, rather than none)
                    log::warn!("junction: {e}");
                    if let Some(t) = doc.tiles.get_mut(&jk) {
                        t.tile.objects.push(o);
                        t.touch_content();
                    }
                }
            },
            2 => {
                join_arms(doc, &arms[0], &arms[1]);
                if was_selected {
                    self.select(None);
                }
            }
            _ => {
                if was_selected {
                    self.select(None);
                }
            }
        }
    }

    /// Keep every tile within `reach` of the points before the ground there changes.
    fn touch_around(&mut self, pts: &[DVec2], reach: f64) {
        let Some(doc) = self.doc.as_ref() else { return };
        let mut keys: Vec<TileKey> = Vec::new();
        for p in pts {
            let (a, b) = (tile_of(*p - DVec2::splat(reach)), tile_of(*p + DVec2::splat(reach)));
            for x in a.0..=b.0 {
                for y in a.1..=b.1 {
                    if doc.tiles.contains_key(&(x, y)) && !keys.contains(&(x, y)) {
                        keys.push((x, y));
                    }
                }
            }
        }
        for k in keys {
            self.history.touch_tile(doc, k);
        }
    }

    /// Put a reshape into the map, the ground following it. Inside an open history step.
    fn apply_plan(&mut self, plan: &reshape::Plan) {
        let Some(doc) = self.doc.as_ref() else { return };
        for t in reshape::tiles_of(doc, plan) {
            self.history.touch_tile(doc, t);
        }
        self.history.touch_global(doc);
        let half = plan.pieces.first().and_then(|p| doc.spline(p.key, p.id)).map(|s| self.half_of(&s.file)).unwrap_or(0.0);
        let curves: Vec<omsi_geometry::SplineCurve> = reshape::preview(doc, plan).iter().map(|s| omsi_geometry::SplineCurve::from_map(s, DVec2::ZERO)).collect();
        let follow = self.follow_ground && half > 0.0;
        if follow {
            let pts: Vec<DVec2> = curves.iter().flat_map(|c| (0..=8).map(move |i| c.point_at(c.length * i as f64 / 8.0).truncate())).collect();
            self.touch_around(&pts, half + 12.0);
        }
        let Some(doc) = self.doc.as_mut() else { return };
        reshape::apply(doc, plan);
        if follow {
            conform_ground(doc, &curves, half);
        }
    }

    /// Bring the ground to roads ("Shape the ground" on), each type with its own width
    /// (`half`: 0 leaves one alone). Inside an open history step.
    fn ground_to(&mut self, splines: &[(TileKey, i64)], half: &dyn Fn(&str) -> f64) {
        if !self.follow_ground {
            return;
        }
        let Some(doc) = self.doc.as_ref() else { return };
        let mut groups: Vec<(f64, Vec<omsi_geometry::SplineCurve>)> = Vec::new();
        for (k, id) in splines {
            let Some(s) = doc.spline(*k, *id) else { continue };
            let h = half(&s.file);
            if h <= 0.0 {
                continue;
            }
            let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(*k));
            match groups.iter_mut().find(|g| (g.0 - h).abs() < 1e-6) {
                Some(g) => g.1.push(c),
                None => groups.push((h, vec![c])),
            }
        }
        for (h, curves) in groups {
            let pts: Vec<DVec2> = curves.iter().flat_map(|c| (0..=8).map(move |i| c.point_at(c.length * i as f64 / 8.0).truncate())).collect();
            self.touch_around(&pts, h + 12.0);
            let Some(doc) = self.doc.as_mut() else { return };
            conform_ground(doc, &curves, h);
        }
    }

    /// The ground to the selected roads (after they moved or turned).
    fn ground_to_selection(&mut self) {
        let roads: Vec<(TileKey, i64)> = self.selection().iter().filter_map(|s| if let Sel::Spline(k, id) = s { Some((*k, *id)) } else { None }).collect();
        let known = self.known_half.clone();
        self.ground_to(&roads, &|f| known.get(&f.to_lowercase()).copied().unwrap_or(0.0));
    }

    // ------------------------------------------------------------ objects along roads

    /// Put the object type chosen in the catalogue along the selected roads.
    pub fn place_rows(&mut self) {
        let Some(file) = self.object_type.clone() else {
            self.toast(tr("Pick an object in the Objects tool's catalogue first", "Сначала выберите объект в каталоге инструмента «Объекты»"));
            return;
        };
        let sels = self.selection();
        let Some(doc) = self.doc.as_mut() else { return };
        let chains = crate::rows::chains(doc, &sels);
        let rows: Vec<_> = chains.iter().flat_map(|c| crate::rows::plan(doc, c, &file, &self.row_params)).collect();
        if rows.is_empty() {
            self.toast(tr("The road is too short for that", "Дорога для этого слишком короткая"));
            return;
        }
        self.history.begin(tr("Objects along the road", "Объекты вдоль дороги"));
        for (k, _, _) in &rows {
            self.history.touch_tile(doc, *k);
        }
        self.history.touch_global(doc);
        let n = crate::rows::add(doc, rows);
        self.history.commit();
        self.toast(format!("{} {n}", tr("Rows of objects added:", "Добавлено рядов объектов:")));
    }

    /// Change a row (the road's properties): one step while a number is dragged.
    pub fn edit_row(&mut self, k: TileKey, row: usize, f: impl FnOnce(&mut omsi_map::SplineAttachment)) {
        self.begin_panel_step(tr("Edit row of objects", "Изменение ряда объектов"));
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.touch_tile(doc, k);
        if let Some(t) = doc.tiles.get_mut(&k) {
            if let Some(a) = t.tile.spline_attachments.get_mut(row) {
                f(a);
                t.touch_content();
            }
        }
    }

    pub fn remove_row(&mut self, k: TileKey, row: usize) {
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.begin(tr("Remove row of objects", "Удаление ряда объектов"));
        self.history.touch_tile(doc, k);
        crate::rows::remove(doc, k, row);
        self.history.commit();
    }

    /// Take every row off the selected roads.
    pub fn remove_rows_of_selection(&mut self) {
        let sels = self.selection();
        let Some(doc) = self.doc.as_mut() else { return };
        self.history.begin(tr("Remove rows of objects", "Удаление рядов объектов"));
        for s in &sels {
            let Sel::Spline(k, id) = *s else { continue };
            self.history.touch_tile(doc, k);
            // (from the last: the places of the others stay)
            for r in crate::rows::rows_on(doc, k, id).into_iter().rev() {
                crate::rows::remove(doc, k, r);
            }
        }
        self.history.commit();
    }

    // ------------------------------------------------------------ drawing by numbers

    /// The numbers typed for the road being drawn.
    pub fn typed(&self) -> crate::draw::Typed {
        let t = &self.typed_text;
        crate::draw::Typed { length: crate::draw::parse(&t[0]).filter(|v| *v > 0.0), radius: crate::draw::parse(&t[1]), heading: crate::draw::parse(&t[2]).map(roads::norm_deg) }
    }

    /// How much a road that keeps its height climbs to its end (m).
    pub fn typed_rise(&self) -> f64 {
        crate::draw::parse(&self.typed_text[3]).unwrap_or(0.0)
    }

    /// The road being drawn, as it would be built: length, radius of its last arc (0:
    /// straight), heading at its end, rise.
    pub fn road_plan_facts(&self) -> Option<(f64, f64, f64, f64)> {
        let (segs, z0, z1, _) = self.road_plan.as_ref()?;
        let last = segs.last()?;
        Some((roads::total_length(segs), last.radius, roads::norm_deg(last.end_heading()), z1 - z0))
    }

    /// Build the road drawn so far (Enter).
    pub fn confirm_road(&mut self, assets: &mut Assets, lib: &Library) {
        if self.tool == Tool::Objects && !self.plant_outline.is_empty() {
            self.finish_plant_outline(assets);
            return;
        }
        if self.acts_as() == Tool::Roads && self.road_start.is_some() {
            self.plan_road(assets);
            self.road_click(assets, lib);
        }
    }

    // ------------------------------------------------------------ roundabouts

    /// The ring's radius for the cursor now (None: no centre yet).
    fn ring_radius(&self) -> Option<(DVec3, f64)> {
        let c = self.ring_centre?;
        let r = (self.cursor?.truncate() - c.truncate()).length();
        // (nor a ring round half the world, the cursor looking at the horizon)
        (r * std::f64::consts::TAU <= crate::draw::MAX_LENGTH).then_some((c, r.max(crate::roundabout::MIN_RADIUS)))
    }

    /// The ring goes the way traffic goes round: anticlockwise where it keeps right, unless
    /// the road type's lanes run backwards.
    fn ring_clockwise(&self, assets: &mut Assets, rel: &str) -> bool {
        let left = self.doc.as_ref().map(|d| d.global.left_hand_traffic).unwrap_or(false);
        let backwards = assets.spline(rel).map(|m| {
            let streets: Vec<i32> = m.def.paths.iter().filter(|p| p.kind == 0).map(|p| p.direction).collect();
            !streets.is_empty() && streets.iter().all(|d| *d == 1)
        });
        left != backwards.unwrap_or(false)
    }

    fn ring_click(&mut self, assets: &mut Assets) {
        let Some(c) = self.cursor else { return };
        if self.ring_centre.is_none() {
            self.ring_centre = Some(c);
            return;
        }
        let Some((centre, r)) = self.ring_radius() else { return };
        self.ring_centre = None;
        self.build_roundabout(assets, centre, r);
    }

    /// Build a roundabout of the chosen road type: the ring, the roads inside it cut back
    /// to its edge, junctions where they meet it.
    pub fn build_roundabout(&mut self, assets: &mut Assets, centre: DVec3, r: f64) {
        let Some(rel) = self.road_type.clone() else {
            self.toast(tr("Pick a road type in the catalogue first", "Сначала выберите тип дороги в каталоге"));
            return;
        };
        let Some(model) = assets.spline(&rel) else { return };
        let two_way = {
            let dirs: Vec<i32> = model.def.paths.iter().filter(|p| p.kind == 0).map(|p| p.direction).collect();
            dirs.contains(&2) || (dirs.contains(&0) && dirs.contains(&1))
        };
        let half = model.half_width as f64;
        let clockwise = self.ring_clockwise(assets, &rel);
        let segs = crate::roundabout::ring(centre.truncate(), r, clockwise);
        let Some(doc) = self.doc.as_ref() else { return };
        let z = doc.ground(segs[0].start);
        // (the roads one sees that come into the circle: they are cut back to it)
        let mut inside: Vec<(TileKey, i64)> = Vec::new();
        for (k, t) in &doc.tiles {
            let org = tile_origin(*k);
            for s in t.tile.splines.iter().filter(|s| !s.deleted) {
                if !assets.spline(&s.file).map(|m| m.joinable()).unwrap_or(false) {
                    continue;
                }
                let c = omsi_geometry::SplineCurve::from_map(s, org);
                let n = (s.length / 2.0).ceil().max(2.0) as usize;
                if (0..=n).any(|i| (c.point_at(s.length * i as f64 / n as f64).truncate() - centre.truncate()).length() < r) {
                    inside.push((*k, s.id));
                }
            }
        }
        let Some(made) = self.build_road(&rel, &segs, z, z, Anchor::default(), None, half) else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        // the ring closes on itself
        let (first, last) = (made[0], *made.last().unwrap());
        if let Some(s) = doc.tiles.get_mut(&first.0).and_then(|t| t.tile.splines.iter_mut().find(|s| s.id == first.1)) {
            s.prev_id = last.1;
        }
        if let Some(s) = doc.tiles.get_mut(&last.0).and_then(|t| t.tile.splines.iter_mut().find(|s| s.id == last.1)) {
            s.next_id = first.1;
        }
        // (cut a metre inside the ring's centre line, so each road still crosses it there)
        crate::roundabout::clear_inside(doc, &inside, centre.truncate(), r - 1.0);
        // openMapper's junctions inside go with the roads they joined
        crate::roundabout::remove_junctions_inside(doc, centre.truncate(), r - 1.0);
        for k in made.iter().map(|m| m.0).collect::<Vec<_>>() {
            doc.tiles.get_mut(&k).map(|t| t.touch_content());
        }
        let n = if self.auto_junctions { self.make_junctions(assets, &made, half, None, None) } else { 0 };
        self.history.commit();
        self.toast(format!("{} {n}", tr("Roundabout built; roads joined to it:", "Кольцо построено; дорог присоединено:")));
        if two_way {
            self.error(tr("This road type carries traffic both ways: pick a one-way road for a proper roundabout", "У этого типа дороги движение в обе стороны: для настоящего кольца выберите одностороннюю дорогу"));
        }
    }

    // ------------------------------------------------------------ ruler

    /// The ruler on or off.
    pub fn toggle_ruler(&mut self) {
        self.ruler = if self.ruler.is_some() { None } else { Some((None, None)) };
    }

    /// What the ruler measures: from its first point to its second (or the cursor).
    pub fn ruler_line(&self) -> Option<(DVec3, DVec3)> {
        let (Some(a), b) = self.ruler? else { return None };
        Some((a, b.or(self.cursor)?))
    }

    // ------------------------------------------------------------ snapping

    /// A point to the grid, when the grid snaps.
    pub fn snap_point(&self, p: DVec2) -> DVec2 {
        if self.snap_grid && self.grid_step > 0.0 {
            (p / self.grid_step).round() * self.grid_step
        } else {
            p
        }
    }

    /// A heading to whole 15 degrees, when angles snap.
    pub fn snap_heading(&self, h: f64) -> f64 {
        if self.snap_angle { snap_deg(h, self.angle_step) } else { h }
    }

    // ------------------------------------------------------------ drawing

    /// The selection drawn over the view: splines' centre lines, a ring round each object
    /// selected with others, the grips of a spline selected alone, a reshape's new shape.
    fn selection_extras(&self, x: &mut FrameExtras, cam: &crate::camera::Camera, w: f32, assets: &mut Assets) {
        let Some(doc) = self.doc.as_ref() else { return };
        let sels = self.selection();
        for s in &sels {
            match *s {
                Sel::Spline(k, id) => {
                    let Some(s) = doc.spline(k, id) else { continue };
                    let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(k));
                    // (the outline: the two edges of the road, thin (not across the ends: the grips sit there), nothing laid over it)
                    let half = assets.spline(&s.file).map(|m| m.half_width as f64).unwrap_or(1.0).max(0.1);
                    let n = (s.length / 2.0).ceil().max(1.0) as usize;
                    let at = |i: usize| c.point_at(s.length * i as f64 / n as f64);
                    // (as thick as the road is wide, never thinner than a hairline on the screen)
                    let thick = (half as f32 * 0.04).max(w * 0.12);
                    let side = |i: usize, sign: f64| {
                        let (a, b) = if i == 0 { (at(0), at(1)) } else { (at(i - 1), at(i)) };
                        let t = (b - a).truncate().normalize_or_zero();
                        at(i) + DVec3::new(-t.y, t.x, 0.0) * half * sign + DVec3::Z * 0.2
                    };
                    for i in 0..n {
                        for sign in [-1.0, 1.0] {
                            line(&mut x.overlay, cam, side(i, sign), side(i + 1, sign), thick, ORANGE);
                        }
                    }
                }
                // (a small ball on the object: the object itself stays uncovered)
                Sel::Object(k, id) => {
                    let Some(o) = doc.object(k, id) else { continue };
                    ball(&mut x.overlay_depth, doc.object_world(k, o, false) + DVec3::Z * w as f64 * 0.5, w as f64 * 0.8, ORANGE);
                }
            }
        }
        // what one object takes its state from (cyan) and what takes it from it
        if let (Some(Sel::Object(k, id)), 1) = (self.selected, sels.len()) {
            let at = |k: TileKey, o: &MapObject| (o.parent_id.is_none()).then(|| doc.object_world(k, o, false) + DVec3::Z * 0.5);
            if let Some(me) = doc.object(k, id).and_then(|o| at(k, o)) {
                let parent = doc.object(k, id).and_then(|o| o.var_parent).and_then(|p| selection::object_by_id(doc, p));
                let others = parent.into_iter().map(|p| (p, true)).chain(selection::var_children(doc, id).into_iter().map(|c| (c, false)));
                for ((pk, o), up) in others {
                    let Some(there) = at(pk, o) else { continue };
                    line(&mut x.overlay, cam, me, there, w * if up { 0.9 } else { 0.6 }, LINK);
                    disc(&mut x.overlay, cam, there, w * if up { 3.0 } else { 2.0 }, LINK);
                }
            }
        }
        if self.acts_as() == Tool::Select {
            let eye = cam.eye();
            for (sp, h, p) in self.handles() {
                let on = (self.hover_handle == Some(h) && self.hover_handle_of == Some(sp)) || matches!(self.drag, Drag::Reshape(k, i, d) if d == h && (k, i) == sp);
                let picked = self.path_point == Some((sp, h));
                let (c, r) = self.grip(assets, sp, h, p, (p - eye).length());
                ball(&mut x.overlay_depth, c, r, if picked { ORANGE } else if on { GREEN } else { WHITE });
            }
        }
        for plan in &self.carry_preview {
            for mut s in reshape::preview(doc, plan) {
                s.pos[2] += 0.03;
                let file = s.file.clone();
                x.ghost_splines.push((file, s, [0.35, 0.85, 0.45, 0.35]));
            }
        }
        match &self.reshape {
            Some(Ok(plan)) => {
                for s in reshape::preview(doc, plan) {
                    let mut s = s;
                    s.pos[2] += 0.03;
                    let file = s.file.clone();
                    x.ghost_splines.push((file, s, [0.35, 0.85, 0.45, 0.35]));
                }
            }
            Some(Err(_)) => {
                if let Some(c) = self.cursor {
                    disc(&mut x.overlay, cam, c, w * 3.0, [1.0, 0.3, 0.3, 0.9]);
                }
            }
            None => {}
        }
    }
}

/// A shaded ball (the light from above and a little aside), for the grips.
fn ball(out: &mut Vec<OverlayPoint>, c: DVec3, r: f64, color: [f32; 4]) {
    let (rows, cols) = (8, 14);
    let light = DVec3::new(0.35, -0.45, 0.82).normalize();
    let at = |i: usize, j: usize| {
        let (lat, lon) = (std::f64::consts::PI * i as f64 / rows as f64, std::f64::consts::TAU * j as f64 / cols as f64);
        let n = DVec3::new(lat.sin() * lon.cos(), lat.sin() * lon.sin(), lat.cos());
        let k = (0.5 + 0.5 * n.dot(light).max(0.0)) as f32;
        OverlayPoint { pos: c + n * r, color: [color[0] * k, color[1] * k, color[2] * k, 1.0] }
    };
    for i in 0..rows {
        for j in 0..cols {
            let (a, b, c2, d) = (at(i, j), at(i, j + 1), at(i + 1, j), at(i + 1, j + 1));
            out.extend([a, b, c2, b, d, c2]);
        }
    }
}

/// Join two road ends that were cut back for a junction that turned out to have only these
/// two (the others too short to keep): a smooth piece of the first one's road between them.
/// A new junction's options from its roads' styles: lights if any road asks for them, a
/// zebra on every arm if any road does.
fn style_options(doc: &Document, styles: &crate::road_maker::Styles, arms: &[crate::junction::Arm], centre: DVec3) -> crate::junction::Options {
    let st: Vec<crate::road_maker::JunctionStyle> = arms.iter().filter_map(|a| doc.spline(a.key, a.id)).map(|s| styles.get(&s.file)).collect();
    let mut o = crate::junction::Options { lights: st.iter().any(|s| s.lights), ..Default::default() };
    if st.iter().any(|s| s.crossings) {
        o.crossings = arm_ends_world(doc, arms).iter().map(|e| roads::heading_of(e.truncate() - centre.truncate())).collect();
    }
    o
}

fn join_arms(doc: &mut Document, a: &crate::junction::Arm, b: &crate::junction::Arm) {
    let end = |arm: &crate::junction::Arm| -> Option<(DVec3, f64, String, f64)> {
        let s = doc.spline(arm.key, arm.id)?;
        let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(arm.key));
        let at = if arm.at_end { s.length } else { 0.0 };
        // heading towards the gap, and the gradient going that way
        let h = if arm.at_end { c.heading_at(at) } else { c.heading_at(at) + 180.0 };
        let g = c.slope_at(at) * 100.0 * if arm.at_end { 1.0 } else { -1.0 };
        Some((c.point_at(at), roads::norm_deg(h), s.file.clone(), g))
    };
    let (Some((pa, ha, file, ga)), Some((pb, hb, _, gb))) = (end(a), end(b)) else { return };
    let Some(segs) = roads::biarc(pa.truncate(), ha, pb.truncate(), roads::norm_deg(hb + 180.0)).or_else(|| roads::straight(pa.truncate(), pb.truncate()).map(|s| vec![s])) else { return };
    let total: f64 = segs.iter().map(|s| s.length).sum::<f64>().max(0.01);
    let mut ids = Vec::new();
    let mut acc = 0.0;
    for sg in &segs {
        let z0 = pa.z + (pb.z - pa.z) * acc / total;
        acc += sg.length;
        let z1 = pa.z + (pb.z - pa.z) * acc / total;
        let k = tile_of(sg.start);
        if !doc.tiles.contains_key(&k) {
            return;
        }
        let id = doc.new_id();
        let org = tile_origin(k);
        let t0 = if ids.is_empty() { ga } else { (z1 - z0) / sg.length.max(0.01) * 100.0 };
        let t1 = if acc >= total - 1e-6 { -gb } else { (z1 - z0) / sg.length.max(0.01) * 100.0 };
        let sp = MapSpline {
            file: file.clone(),
            id,
            pos: [sg.start.x - org.x, sg.start.y - org.y, z0],
            heading: sg.heading,
            length: sg.length,
            radius: sg.radius,
            grad_start: t0,
            grad_end: t1,
            delta_h: Some(z1 - z0),
            is_h: true,
            ..Default::default()
        };
        let t = doc.tiles.get_mut(&k).unwrap();
        t.tile.splines.push(sp);
        t.touch_content();
        ids.push((k, id));
    }
    // link the chain: a's end -> new pieces -> b's end
    let set = |doc: &mut Document, k: TileKey, id: i64, at_end: bool, other: i64| {
        if let Some(t) = doc.tiles.get_mut(&k) {
            if let Some(s) = t.tile.splines.iter_mut().find(|s| s.id == id) {
                if at_end { s.next_id = other } else { s.prev_id = other }
            }
            t.touch_content();
        }
    };
    for (i, (k, id)) in ids.iter().enumerate() {
        let prev = if i == 0 { a.id } else { ids[i - 1].1 };
        let next = if i + 1 == ids.len() { b.id } else { ids[i + 1].1 };
        set(doc, *k, *id, false, prev);
        set(doc, *k, *id, true, next);
    }
    set(doc, a.key, a.id, a.at_end, ids[0].1);
    set(doc, b.key, b.id, b.at_end, ids[ids.len() - 1].1);
}

/// Keep the entry points' records in step with their objects.
/// The angles that tilt an object facing `heading` at `at` to the ground under it: its
/// forward and right axes, as the view turns them, climbing as the ground climbs along them
/// (sampled 2 m either side; the signs found by trying, so they follow OMSI's convention).
pub fn lean_rotation(heading: f64, ground: &dyn Fn(DVec2) -> f64, at: DVec2) -> [f64; 3] {
    let axes = |rot: [f64; 3]| {
        let m = object_matrix(DVec3::ZERO, rot);
        (m.transform_vector3(glam::Vec3::Y).as_dvec3(), m.transform_vector3(glam::Vec3::X).as_dvec3())
    };
    let (f0, r0) = axes([heading, 0.0, 0.0]);
    let grade = |d: DVec3| {
        let d = d.truncate().normalize_or_zero();
        (ground(at + d * 2.0) - ground(at - d * 2.0)) / 4.0
    };
    let (gf, gr) = (grade(f0), grade(r0));
    let (a, b) = (gf.atan().to_degrees(), gr.atan().to_degrees());
    let rise = |v: DVec3| v.z / v.truncate().length().max(1e-9);
    [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)]
        .into_iter()
        .map(|(s1, s2)| [heading, s1 * a, s2 * b])
        .min_by(|x, y| {
            let err = |rot: [f64; 3]| {
                let (f, r) = axes(rot);
                (rise(f) - gf).abs() + (rise(r) - gr).abs()
            };
            err(*x).total_cmp(&err(*y))
        })
        .unwrap()
}

pub fn sync_entry_points(doc: &mut Document) {
    let order = doc.order.clone();
    let mut eps = std::mem::take(&mut doc.global.entry_points);
    for e in &mut eps {
        for (k, t) in &doc.tiles {
            if let Some((i, o)) = t.tile.objects.iter().enumerate().find(|(_, o)| o.id == e.object_id) {
                e.index = i as i32;
                e.pos = [o.pos[0], o.pos[1], o.pos[2]];
                let h = (o.rot[0] / 2.0).to_radians();
                e.quat = [0.0, h.sin(), 0.0, h.cos()];
                e.group = order.iter().position(|x| x == k).unwrap_or(0) as i32;
            }
        }
    }
    doc.global.entry_points = eps;
    doc.global_dirty = true;
}

/// The tiles a sample of the map-wide grid (tile × 60 + i) belongs to along one axis.
fn owners(g: i64, n: i64) -> Vec<i64> {
    if g.rem_euclid(n) == 0 { vec![g.div_euclid(n), g.div_euclid(n) - 1] } else { vec![g.div_euclid(n)] }
}

/// A terrain sample on the map-wide grid.
fn world_sample(doc: &Document, gx: i64, gy: i64, n: i64) -> Option<f32> {
    for tx in owners(gx, n) {
        for ty in owners(gy, n) {
            if let Some(t) = doc.tiles.get(&(tx as i32, ty as i32)) {
                let (i, j) = ((gx - tx * n) as usize, (gy - ty * n) as usize);
                return Some(t.terrain.height_at(i, j));
            }
        }
    }
    None
}

/// The ground under an outline (`lo`..`hi` in the frame of something at `centre` turned
/// `heading`) made `level`, blending into the ground about over 3 m.
fn level_ground(doc: &mut Document, hist: &mut History, centre: DVec2, heading: f64, (lo, hi): (Vec3, Vec3), level: f64) {
    const MARGIN: f64 = 3.0;
    let cell = tile_size() / (TERRAIN_SAMPLES - 1) as f64;
    let n = (TERRAIN_SAMPLES - 1) as i64;
    let inv = object_matrix(DVec3::ZERO, [heading, 0.0, 0.0]).inverse();
    let r = ((hi.x - lo.x).hypot(hi.y - lo.y) as f64) * 0.5 + MARGIN + DVec2::new(((lo.x + hi.x) * 0.5) as f64, ((lo.y + hi.y) * 0.5) as f64).length();
    let (gx0, gx1) = (((centre.x - r) / cell).floor() as i64, ((centre.x + r) / cell).ceil() as i64);
    let (gy0, gy1) = (((centre.y - r) / cell).floor() as i64, ((centre.y + r) / cell).ceil() as i64);
    for gy in gy0..=gy1 {
        for gx in gx0..=gx1 {
            let p = DVec2::new(gx as f64 * cell, gy as f64 * cell) - centre;
            let l = inv.transform_point3(Vec3::new(p.x as f32, p.y as f32, 0.0));
            // how far outside the outline (0 inside)
            let dx = (lo.x - l.x).max(l.x - hi.x).max(0.0) as f64;
            let dy = (lo.y - l.y).max(l.y - hi.y).max(0.0) as f64;
            let out = dx.hypot(dy);
            if out >= MARGIN {
                continue;
            }
            let Some(h) = world_sample(doc, gx, gy, n) else { continue };
            let t = out / MARGIN;
            let w = 1.0 - t * t * (3.0 - 2.0 * t);
            set_world_sample(doc, hist, gx, gy, n, (h as f64 + (level - h as f64) * w) as f32);
        }
    }
}

/// Set a sample of the map-wide grid in every tile that has it (edges are shared).
fn set_world_sample(doc: &mut Document, hist: &mut History, gx: i64, gy: i64, n: i64, h: f32) {
    let txs = owners(gx, n);
    let tys = owners(gy, n);
    for &tx in &txs {
        for &ty in &tys {
            let key = (tx as i32, ty as i32);
            if !doc.tiles.contains_key(&key) {
                continue;
            }
            hist.touch_tile(doc, key);
            let t = doc.tiles.get_mut(&key).unwrap();
            let i = (gx - tx * n) as usize;
            let j = (gy - ty * n) as usize;
            let s = TERRAIN_SAMPLES;
            if i < s && j < s {
                t.terrain.heights[j * s + i] = h;
                t.touch_terrain();
            }
        }
    }
}

/// The heights of a planned road at the joints of its pieces, (height, gradient %): the
/// ground's, smoothed along the road (so it climbs hills instead of cutting through them),
/// from `z0` at the start to `z1` at the end, no steeper than 10 %.
/// An even climb from `z0` to `z1` (a road that keeps its height: a bridge, an embankment).
fn even_profile(pieces: &[Seg], z0: f64, z1: f64) -> Vec<(f64, f64)> {
    let total = roads::total_length(pieces).max(0.1);
    let g = (z1 - z0) / total * 100.0;
    let mut acc = 0.0;
    let mut out = vec![(z0, g)];
    for p in pieces {
        acc += p.length;
        out.push((z0 + (z1 - z0) * acc / total, g));
    }
    out
}

fn road_profile(doc: &Document, pieces: &[Seg], z0: f64, z1: f64) -> Vec<(f64, f64)> {
    let mut nodes = vec![0.0];
    for p in pieces {
        nodes.push(nodes.last().unwrap() + p.length);
    }
    let total = *nodes.last().unwrap();
    // the ground every 4 m along the road
    let mut samples: Vec<(f64, f64)> = Vec::new();
    let mut acc = 0.0;
    for p in pieces {
        let c = p.curve();
        let n = (p.length / 4.0).ceil().max(1.0) as usize;
        for i in 0..n {
            let s = p.length * i as f64 / n as f64;
            samples.push((acc + s, doc.ground(c.point_at(s).truncate())));
        }
        acc += p.length;
    }
    let window = 20.0;
    let mut z: Vec<f64> = nodes
        .iter()
        .map(|&s| {
            let near: Vec<f64> = samples.iter().filter(|q| (q.0 - s).abs() <= window).map(|q| q.1).collect();
            if near.is_empty() { doc.ground(DVec2::ZERO) } else { near.iter().sum::<f64>() / near.len() as f64 }
        })
        .collect();
    let last = z.len() - 1;
    z[0] = z0;
    z[last] = z1;
    // no steeper than 10 % (a few passes both ways)
    for _ in 0..4 {
        for i in 1..=last {
            let d = (nodes[i] - nodes[i - 1]).max(0.1) * 0.10;
            if i != last {
                z[i] = z[i].clamp(z[i - 1] - d, z[i - 1] + d);
            }
        }
        for i in (0..last).rev() {
            let d = (nodes[i + 1] - nodes[i]).max(0.1) * 0.10;
            if i != 0 {
                z[i] = z[i].clamp(z[i + 1] - d, z[i + 1] + d);
            }
        }
    }
    let _ = total;
    (0..=last)
        .map(|i| {
            let (a, b) = (i.saturating_sub(1), (i + 1).min(last));
            let g = if b > a { (z[b] - z[a]) / (nodes[b] - nodes[a]).max(0.1) * 100.0 } else { 0.0 };
            (z[i], g)
        })
        .collect()
}

/// Piece `i` of a planned road as a `[spline_h]` (its height a smooth cubic between the
/// joints' heights and gradients), tile-local to `org`.
/// Bring a track that goes on beyond a switch round to the switch's straight heel: where
/// it bent away under the (straight) switch it is cut back a little more and joined to the
/// heel by a pair of arcs, tangent at both ends. `end`: the track's spline and whether its
/// end (else its start) is where it stops; `heel`: the heel's place and heading out.
fn rejoin_track(doc: &mut Document, end: (TileKey, i64, bool), heel: (DVec3, f64)) {
    let at = |doc: &Document, e: (TileKey, i64, bool)| -> Option<(DVec3, f64, String)> {
        let s = doc.spline(e.0, e.1)?;
        let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(e.0));
        // (the heading on along the track, away from the switch)
        Some(if e.2 { (c.end_point(), roads::norm_deg(c.heading_at(s.length) + 180.0), s.file.clone()) } else { (c.point_at(0.0), s.heading, s.file.clone()) })
    };
    let Some((p, h, _)) = at(doc, end) else { return };
    let off = (p.truncate() - heel.0.truncate()).length();
    let turn = (roads::norm_deg(h - heel.1 + 180.0) - 180.0).abs();
    if off < 0.05 && turn < 0.2 {
        return;
    }
    // (room for the arcs to bring it round as gently as a track bends, some 300 m: an S of
    // that radius moves sideways L² / 4R over its length L, and turns L / R)
    const R: f64 = 300.0;
    let room = (4.0 * R * off).sqrt() + R * turn.to_radians();
    let Some(end) = doc.trim(end.0, end.1, end.2, room.clamp(10.0, 150.0)) else { return };
    let Some((q, hq, file)) = at(doc, end) else { return };
    let Some(segs) = roads::biarc(heel.0.truncate(), heel.1, q.truncate(), hq) else { return };
    let pieces = roads::split(&segs, 50.0);
    let profile = even_profile(&pieces, heel.0.z.max(q.z.min(heel.0.z + 5.0)), q.z);
    let ids: Vec<i64> = pieces.iter().map(|_| doc.new_id()).collect();
    for (i, piece) in pieces.iter().enumerate() {
        let k = tile_of(piece.start);
        let Some(t) = doc.tiles.get_mut(&k) else { continue };
        let mut sp = piece_spline(&file, piece, &profile, i, tile_origin(k));
        sp.id = ids[i];
        sp.prev_id = if i > 0 { ids[i - 1] } else { 0 };
        sp.next_id = if i + 1 < ids.len() { ids[i + 1] } else { 0 };
        t.tile.splines.push(sp);
        t.touch_content();
    }
    // the last piece runs on into the track
    let last = *ids.last().unwrap();
    if let Some((k, s)) = doc.find_spline(end.1).map(|(k, s)| (k, s.id)) {
        if let Some(t) = doc.tiles.get_mut(&k) {
            if let Some(sp) = t.tile.splines.iter_mut().find(|x| x.id == s) {
                if end.2 { sp.next_id = last } else { sp.prev_id = last }
            }
            t.touch_content();
        }
        if let Some(pk) = doc.find_spline(last).map(|(k, _)| k) {
            if let Some(sp) = doc.tiles.get_mut(&pk).and_then(|t| t.tile.splines.iter_mut().find(|x| x.id == last)) {
                sp.next_id = s;
            }
        }
    }
}

fn piece_spline(rel: &str, p: &Seg, profile: &[(f64, f64)], i: usize, org: DVec2) -> MapSpline {
    let (za, ga) = profile[i];
    let (zb, gb) = profile[i + 1];
    MapSpline {
        file: rel.to_string(),
        pos: [p.start.x - org.x, p.start.y - org.y, za],
        heading: p.heading,
        length: p.length,
        radius: p.radius,
        grad_start: ga,
        grad_end: gb,
        delta_h: Some(zb - za),
        is_h: true,
        ..Default::default()
    }
}

/// Bring the ground to a road: flat under it, blending back to the old ground beside it.
/// The whole road at once (each ground point follows the road's nearest point), so the
/// blend beside one piece never lifts the ground back over the piece next to it.
fn conform_ground(doc: &mut Document, curves: &[omsi_geometry::SplineCurve], half: f64) {
    let cell = tile_size() / (TERRAIN_SAMPLES - 1) as f64;
    let n = (TERRAIN_SAMPLES - 1) as i64;
    let core = half + 0.5;
    let blend = 8.0;
    // (the ground is drawn as triangles between its samples: a sample one cell beside the
    // road, left high on a slope, lifts the triangle it shares with one under the road up
    // through the road's edge. Within a cell's diagonal of the edge the ground is only ever
    // taken down to the road)
    let below = half + cell * std::f64::consts::SQRT_2 + 0.1;
    let reach = (core + blend).max(below);
    let mut pts: Vec<(DVec2, f64)> = Vec::new();
    for c in curves {
        let steps = (c.length / 1.0).ceil().max(2.0) as usize;
        for i in 0..=steps {
            let q = c.point_at(c.length * i as f64 / steps as f64);
            pts.push((q.truncate(), q.z));
        }
    }
    if pts.is_empty() {
        return;
    }
    let (mut lo, mut hi) = (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN));
    for (q, _) in &pts {
        lo = lo.min(*q);
        hi = hi.max(*q);
    }
    lo -= DVec2::splat(reach);
    hi += DVec2::splat(reach);
    let mut dummy = History::default();
    for gy in (lo.y / cell).floor() as i64..=(hi.y / cell).ceil() as i64 {
        for gx in (lo.x / cell).floor() as i64..=(hi.x / cell).ceil() as i64 {
            let q = DVec2::new(gx as f64 * cell, gy as f64 * cell);
            let (d, z) = pts.iter().map(|(a, z)| ((*a - q).length(), *z)).fold((f64::MAX, 0.0), |b, x| if x.0 < b.0 { x } else { b });
            if d > reach {
                continue;
            }
            let Some(old) = world_sample(doc, gx, gy, n) else { continue };
            let target = z - 0.12;
            let w = if d <= core { 1.0 } else { 1.0 - ((d - core) / blend).clamp(0.0, 1.0) };
            let w = w * w * (3.0 - 2.0 * w);
            let h = old as f64 + (target - old as f64) * w;
            let h = if d <= below { h.min(target) } else { h };
            set_world_sample(doc, &mut dummy, gx, gy, n, h as f32);
        }
    }
}

/// Points along the visible roads within `within` of `c`, every metre: (point, height,
/// half width).
fn road_points_near(doc: &Document, assets: &mut Assets, c: DVec2, within: f64) -> Vec<(DVec2, f64, f64)> {
    let mut out = Vec::new();
    for (k, t) in &doc.tiles {
        let org = tile_origin(*k);
        for s in t.tile.splines.iter().filter(|s| !s.deleted) {
            let start = DVec2::new(org.x + s.pos[0], org.y + s.pos[1]);
            if (start - c).length() > s.length + within + 20.0 {
                continue;
            }
            let Some(m) = assets.spline(&s.file) else { continue };
            if !m.joinable() {
                continue;
            }
            let hw = m.half_width as f64;
            let curve = omsi_geometry::SplineCurve::from_map(s, org);
            let steps = s.length.ceil().max(1.0) as usize;
            for i in 0..=steps {
                let q = curve.point_at(s.length * i as f64 / steps as f64);
                if (q.truncate() - c).length() <= within + hw {
                    out.push((q.truncate(), q.z, hw));
                }
            }
        }
    }
    out
}

/// The highest the ground sample at `p` may lie without its triangles showing through a
/// road beside it (see `conform_ground`), if a road is that near.
fn ground_cap(roads: &[(DVec2, f64, f64)], p: DVec2, cell: f64) -> Option<f64> {
    roads
        .iter()
        .filter(|(q, _, hw)| (*q - p).length() <= hw + cell * std::f64::consts::SQRT_2 + 0.1)
        .map(|(_, z, _)| z - 0.12)
        .min_by(|a, b| a.total_cmp(b))
}

/// Where the arms of a junction end (world, with their heights).
fn arm_ends_world(doc: &Document, arms: &[crate::junction::Arm]) -> Vec<DVec3> {
    arms.iter()
        .filter_map(|a| {
            let s = doc.spline(a.key, a.id)?;
            let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(a.key));
            Some(if a.at_end { c.end_point() } else { c.point_at(0.0) })
        })
        .collect()
}

/// Bring the ground under a junction to its plate: within `r` of the centre onto the plane
/// through its arms' ends (a junction on a hill slopes with its roads), blending back to
/// the old ground over the next 8 m.
fn conform_disc(doc: &mut Document, centre: DVec3, r: f64, ends: &[DVec3]) {
    // the plane z = a + b x + c y through the ends (least squares; level at the centre's
    // height if they say nothing)
    let plane = {
        let n = ends.len() as f64;
        let (mx, my, mz) = (ends.iter().map(|p| p.x - centre.x).sum::<f64>() / n.max(1.0), ends.iter().map(|p| p.y - centre.y).sum::<f64>() / n.max(1.0), ends.iter().map(|p| p.z).sum::<f64>() / n.max(1.0));
        let (mut sxx, mut sxy, mut syy, mut sxz, mut syz) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for p in ends {
            let (x, y, z) = (p.x - centre.x - mx, p.y - centre.y - my, p.z - mz);
            sxx += x * x;
            sxy += x * y;
            syy += y * y;
            sxz += x * z;
            syz += y * z;
        }
        let det = sxx * syy - sxy * sxy;
        let (b, c) = if ends.len() >= 3 && det.abs() > 1e-6 { ((sxz * syy - syz * sxy) / det, (syz * sxx - sxz * sxy) / det) } else { (0.0, 0.0) };
        let mz = if ends.is_empty() { centre.z } else { mz };
        move |q: DVec2| mz + b * (q.x - centre.x - mx) + c * (q.y - centre.y - my)
    };
    let cell = tile_size() / (TERRAIN_SAMPLES - 1) as f64;
    let n = (TERRAIN_SAMPLES - 1) as i64;
    let blend = 8.0;
    let reach = r + blend;
    let c = centre.truncate();
    let mut dummy = History::default();
    for gy in ((c.y - reach) / cell).floor() as i64..=((c.y + reach) / cell).ceil() as i64 {
        for gx in ((c.x - reach) / cell).floor() as i64..=((c.x + reach) / cell).ceil() as i64 {
            let q = DVec2::new(gx as f64 * cell, gy as f64 * cell);
            let d = (q - c).length();
            if d > reach {
                continue;
            }
            let Some(old) = world_sample(doc, gx, gy, n) else { continue };
            let w = if d <= r { 1.0 } else { 1.0 - ((d - r) / blend).clamp(0.0, 1.0) };
            let w = w * w * (3.0 - 2.0 * w);
            let z = plane(q) - 0.12;
            let h = old as f64 + (z - old as f64) * w;
            set_world_sample(doc, &mut dummy, gx, gy, n, h as f32);
        }
    }
}

/// Set a selected thing's height (an object's above its ground, or absolute; a road's
/// start), found by IDCode wherever it is now.
fn set_height(doc: &mut Document, hist: &mut History, s: Sel, z: f64) {
    match s {
        Sel::Object(_, id) => {
            let Some((k, _)) = selection::object_by_id(doc, id) else { return };
            hist.touch_tile(doc, k);
            if let Some(o) = doc.object_mut(k, id) {
                o.pos[2] = z;
            }
            if let Some(t) = doc.tiles.get_mut(&k) {
                t.touch_content();
            }
        }
        Sel::Spline(_, id) => {
            let Some((k, _)) = doc.find_spline(id) else { return };
            selection::touch_spline(hist, doc, k, id);
            if let Some(t) = doc.tiles.get_mut(&k) {
                if let Some(sp) = t.tile.splines.iter_mut().find(|s| s.id == id) {
                    sp.pos[2] = z;
                }
                t.touch_content();
            }
        }
    }
}

/// Put an object (by IDCode) at a world point, turned so: its height is kept above the
/// ground there unless its type has an absolute one.
fn put_object_3d(doc: &mut Document, hist: &mut History, id: i64, p: DVec3, rot: [f64; 3], known: &std::collections::HashMap<String, (bool, Vec<[f32; 4]>)>) -> Option<Sel> {
    let (cur, o) = selection::object_by_id(doc, id)?;
    let abs = known.get(&o.file.to_lowercase()).map(|h| h.0).unwrap_or(false);
    hist.touch_tile(doc, cur);
    let target = tile_of(p.truncate());
    if doc.tiles.contains_key(&target) {
        hist.touch_tile(doc, target);
    }
    let placed = selection::place_object_at(doc, cur, id, p.truncate()).unwrap_or(Sel::Object(cur, id));
    let Sel::Object(k, _) = placed else { return None };
    let z = if abs { p.z } else { p.z - doc.ground(p.truncate()) };
    if let Some(ob) = doc.object_mut(k, id) {
        ob.pos[2] = z;
        ob.rot = rot;
    }
    if let Some(t) = doc.tiles.get_mut(&k) {
        t.touch_content();
    }
    Some(placed)
}

/// A line drawn as a flat ribbon facing the camera.
pub fn line(out: &mut Vec<OverlayPoint>, cam: &crate::camera::Camera, a: DVec3, b: DVec3, width: f32, color: [f32; 4]) {
    let eye = cam.eye();
    let dir = (b - a).normalize_or_zero();
    let to_eye = (eye - a).normalize_or_zero();
    let side = dir.cross(to_eye).normalize_or_zero() * (width as f64 * 0.5);
    let v = |p: DVec3| OverlayPoint { pos: p, color };
    out.extend([v(a - side), v(a + side), v(b + side), v(a - side), v(b + side), v(b - side)]);
}

/// A flat disc on the ground.
pub fn disc(out: &mut Vec<OverlayPoint>, _cam: &crate::camera::Camera, c: DVec3, r: f32, color: [f32; 4]) {
    let n = 20;
    let c = c + DVec3::Z * 0.15;
    let v = |p: DVec3| OverlayPoint { pos: p, color };
    for i in 0..n {
        let a0 = i as f64 / n as f64 * std::f64::consts::TAU;
        let a1 = (i + 1) as f64 / n as f64 * std::f64::consts::TAU;
        let p0 = c + DVec3::new(a0.cos(), a0.sin(), 0.0) * r as f64;
        let p1 = c + DVec3::new(a1.cos(), a1.sin(), 0.0) * r as f64;
        out.extend([v(c), v(p0), v(p1)]);
    }
}

/// An asset's default things to place when it is chosen.
pub fn asset_hint(a: &Asset) -> &'static str {
    if a.is_entry_point {
        tr("Start point: where the bus appears in openOMSI", "Точка старта: здесь автобус появляется в openOMSI")
    } else if a.is_bus_stop {
        tr("Bus stop: name it in the panel on the right", "Остановка: задайте имя в панели справа")
    } else {
        ""
    }
}

/// Rolls of the natural ground at `p`: smooth value noise of two sizes (about 40 m and
/// 12 m across), between -1 and 1.
fn rolls(p: DVec2) -> f64 {
    let hash = |x: i64, y: i64| {
        let mut h = (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
        h ^= h >> 31;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        (h % 20_001) as f64 / 10_000.0 - 1.0
    };
    let noise = |q: DVec2| {
        let (x0, y0) = (q.x.floor(), q.y.floor());
        let (fx, fy) = (q.x - x0, q.y - y0);
        let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let (ix, iy) = (x0 as i64, y0 as i64);
        let a = hash(ix, iy) + (hash(ix + 1, iy) - hash(ix, iy)) * sx;
        let b = hash(ix, iy + 1) + (hash(ix + 1, iy + 1) - hash(ix, iy + 1)) * sx;
        a + (b - a) * sy
    };
    noise(p / 40.0) * 0.7 + noise(p / 12.0 + DVec2::splat(17.3)) * 0.3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::tests::{add_object, add_spline, flat_map};

    #[test]
    fn a_ramp_makes_one_even_slope_as_wide_as_the_brush() {
        let mut ed = Editor::default();
        ed.open(flat_map(1));
        ed.brush_radius = 20.0;
        let grade = ed.ramp(DVec3::new(50.0, 150.0, 0.0), DVec3::new(250.0, 150.0, 20.0));
        assert!((grade - 10.0).abs() < 1e-9);
        let d = ed.doc.as_ref().unwrap();
        assert!((d.ground(DVec2::new(150.0, 150.0)) - 10.0).abs() < 0.3);
        assert!((d.ground(DVec2::new(200.0, 155.0)) - 15.0).abs() < 0.3);
        // past its edge and its ends the ground is as it was
        assert!(d.ground(DVec2::new(150.0, 180.0)).abs() < 1e-6);
        assert!(d.ground(DVec2::new(10.0, 150.0)).abs() < 1e-6);
        ed.undo();
        assert!(ed.doc.as_ref().unwrap().ground(DVec2::new(150.0, 150.0)).abs() < 1e-6);
    }

    #[test]
    fn a_layer_painted_finer_keeps_its_paint_and_undoes() {
        let mut doc = flat_map(2);
        let g = |p: f32| omsi_map::GroundTex { texture: "x.bmp".into(), detail_texture: String::new(), params: [p, 16.0, 4.0] };
        doc.global.ground_textures = vec![g(0.0), g(8.0)];
        doc.sync_mask_slots();
        let mut m = vec![0u8; 256 * 256];
        m[..256 * 128].iter_mut().for_each(|a| *a = 255);
        doc.tiles.get_mut(&(0, 0)).unwrap().masks[1] = Some(m.clone());
        let mut ed = Editor::default();
        ed.open(doc);
        ed.set_paint_detail(1, 10.0);
        let d = ed.doc.as_ref().unwrap();
        let big = d.tiles[&(0, 0)].masks[1].as_ref().unwrap();
        assert_eq!(big.len(), 1024 * 1024);
        assert_eq!((big[0], big[1024 * 1023]), (255, 0));
        assert!(d.tiles[&(1, 0)].masks[1].is_none());
        ed.undo();
        let d = ed.doc.as_ref().unwrap();
        assert_eq!(d.global.ground_textures[1].params[0], 8.0);
        assert_eq!(d.tiles[&(0, 0)].masks[1].as_ref(), Some(&m));
    }

    #[test]
    fn a_signal_head_put_by_a_crossing_takes_its_light_from_it() {
        let Some(root) = crate::install::find_omsi_root(&crate::install::Prefs::default()) else { return };
        let roots = crate::install::Roots { omsi: root, content: std::env::temp_dir().join(format!("om-head-{}", std::process::id())) };
        let mut lib = Library::default();
        lib.start_scan(&roots);
        while lib.scanning {
            lib.poll();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut doc = flat_map(1);
        add_object(&mut doc, (0, 0), 1, 100.0, 100.0);
        doc.tiles.get_mut(&(0, 0)).unwrap().tile.objects[0].file = "Sceneryobjects\\Kreuz_MC\\Kreuz_See_Elsflether.sco".into();
        let mut ed = Editor::default();
        ed.open(doc);
        ed.object_type = Some("Sceneryobjects\\Verkehrszeichen_MC\\Ampel_Kfz_1.sco".into());
        ed.ghost_pos = Some((DVec3::new(112.0, 100.0, 0.0), 0.0));
        ed.place_object(&lib, false);
        let d = ed.doc.as_ref().unwrap();
        let head = d.tiles[&(0, 0)].tile.objects.iter().find(|o| o.file.contains("Ampel")).unwrap();
        assert_eq!((head.var_parent, head.extra.clone()), (Some(1), vec!["0".to_string()]));
        // (far from any crossing: linked to nothing)
        ed.ghost_pos = Some((DVec3::new(250.0, 250.0, 0.0), 0.0));
        ed.place_object(&lib, false);
        assert!(ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.objects.iter().filter(|o| o.file.contains("Ampel")).any(|o| o.var_parent.is_none()));
    }

    #[test]
    fn a_group_is_selected_and_moved_as_one() {
        let dir = std::env::temp_dir().join(format!("om-groups-{}", std::process::id()));
        let mut doc = flat_map(1);
        doc.dir = dir.join("maps").join("G");
        for (id, x) in [(1, 100.0), (2, 110.0), (3, 200.0)] {
            add_object(&mut doc, (0, 0), id, x, 100.0);
        }
        let mut ed = Editor::default();
        ed.open(doc);
        let mut assets = Assets::new(std::path::PathBuf::new(), std::path::PathBuf::new());
        let lib = Library::default();
        ed.set_selection(vec![Sel::Object((0, 0), 1), Sel::Object((0, 0), 2)]);
        ed.group_selection();
        assert_eq!(ed.groups().len(), 1);
        assert_eq!(crate::mapgroups::load(&ed.doc.as_ref().unwrap().dir), ed.groups(), "kept beside the map");
        // a click on one: both, and a drag moves both
        ed.select(None);
        ed.hover = Some(Sel::Object((0, 0), 2));
        ed.cursor = Some(DVec3::new(110.0, 100.0, 0.0));
        ed.left_press(&mut assets, &lib, false);
        assert_eq!(ed.selection().len(), 2);
        assert_eq!(ed.selected_group(), Some(0));
        ed.cursor = Some(DVec3::new(110.0, 120.0, 0.0));
        ed.tick(&mut assets, 0.0, true);
        ed.tick(&mut assets, 0.0, false);
        let d = ed.doc.as_ref().unwrap();
        assert!((d.object((0, 0), 1).unwrap().pos[1] - 120.0).abs() < 0.6 && (d.object((0, 0), 3).unwrap().pos[1] - 100.0).abs() < 1e-9);
        // Ctrl + click: one alone; a right click on one selects its group and asks for the menu
        ed.select(None);
        ed.ctrl_held = true;
        ed.left_press(&mut assets, &lib, false);
        ed.tick(&mut assets, 0.0, false);
        assert_eq!(ed.selection(), [Sel::Object((0, 0), 2)]);
        ed.ctrl_held = false;
        ed.select(None);
        assert!(ed.right_click());
        assert_eq!(ed.selection().len(), 2);
        ed.ungroup_selection();
        assert!(ed.groups().is_empty() && crate::mapgroups::load(&ed.doc.as_ref().unwrap().dir).is_empty());
        // (grouping and ungrouping are undone like any edit, the file following)
        ed.undo();
        assert_eq!(ed.groups().len(), 1);
        assert_eq!(crate::mapgroups::load(&ed.doc.as_ref().unwrap().dir).len(), 1);
        ed.redo();
        assert!(ed.groups().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_ruler_takes_two_clicks_wherever_they_land() {
        let mut doc = flat_map(1);
        add_object(&mut doc, (0, 0), 1, 100.0, 100.0);
        let mut ed = Editor::default();
        ed.open(doc);
        let mut assets = Assets::new(std::path::PathBuf::new(), std::path::PathBuf::new());
        let lib = Library::default();
        ed.toggle_ruler();
        // (the first on an object: it is measured from, not selected)
        ed.hover = Some(Sel::Object((0, 0), 1));
        ed.cursor = Some(DVec3::new(100.0, 100.0, 0.0));
        ed.left_press(&mut assets, &lib, false);
        ed.hover = None;
        ed.cursor = Some(DVec3::new(130.0, 140.0, 0.0));
        ed.left_press(&mut assets, &lib, false);
        let (a, b) = ed.ruler_line().unwrap();
        assert!(((b - a).truncate().length() - 50.0).abs() < 1e-9);
        assert_eq!(ed.selected, None);
        // the angle step is what R turns a selection by and what snapping comes to
        assert_eq!(snap_deg(37.0, 10.0), 40.0);
        assert_eq!(snap_deg(37.0, 45.0), 45.0);
    }

    #[test]
    fn the_knife_cuts_a_road_wherever_it_is_clicked_until_turned_off() {
        let mut doc = flat_map(1);
        crate::selection::tests::add_spline(&mut doc, (0, 0), 1, 50.0, 100.0, 90.0, 100.0, 0, 0);
        let mut ed = Editor::default();
        ed.open(doc);
        let mut assets = Assets::new(std::path::PathBuf::new(), std::path::PathBuf::new());
        let lib = Library::default();
        ed.toggle_cutting();
        ed.cursor_in_view = true;
        // beside the road: cut at the point of it nearest the cursor
        ed.cursor = Some(DVec3::new(80.3, 103.0, 0.0));
        let c = ed.cut_preview().unwrap();
        assert!((c.at - 30.3).abs() < 0.2 && c.fits, "{c:?}");
        // whole metres along it with grid snapping on
        ed.snap_grid = true;
        ed.grid_step = 1.0;
        assert_eq!(ed.cut_preview().unwrap().at, 30.0);
        ed.left_press(&mut assets, &lib, false);
        ed.cursor = Some(DVec3::new(120.0, 99.0, 0.0));
        ed.left_press(&mut assets, &lib, false);
        let lengths: Vec<f64> = ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.splines.iter().map(|s| s.length).collect();
        assert_eq!(lengths, vec![30.0, 40.0, 30.0]);
        // a right click puts the knife away; a click then cuts nothing
        ed.right_click();
        assert!(!ed.cutting && ed.cut_preview().is_none());
        ed.history.undo(ed.doc.as_mut().unwrap());
        assert_eq!(ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.splines.len(), 2);
    }

    #[test]
    fn game_logic_selects_and_moves_a_stop_already_there() {
        let mut doc = flat_map(1);
        add_object(&mut doc, (0, 0), 1, 100.0, 100.0);
        let o = &mut doc.tiles.get_mut(&(0, 0)).unwrap().tile.objects[0];
        o.flag = 7;
        o.extra = vec!["Market".into(), "0".into(), "0".into(), "10".into()];
        let mut ed = Editor::default();
        ed.open(doc);
        ed.set_tool(Tool::Logic);
        ed.set_logic(LogicMode::Stops);
        ed.object_type = Some("stop.sco".into());
        let mut assets = Assets::new(std::path::PathBuf::new(), std::path::PathBuf::new());
        let lib = Library::default();
        ed.hover = Some(Sel::Object((0, 0), 1));
        ed.cursor = Some(DVec3::new(100.0, 100.0, 0.0));
        ed.left_press(&mut assets, &lib, false);
        assert_eq!(ed.selected, Some(Sel::Object((0, 0), 1)));
        assert_eq!(ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.objects.len(), 1, "nothing new put");
        // (dragged: it moves)
        ed.cursor = Some(DVec3::new(110.0, 100.0, 0.0));
        ed.tick(&mut assets, 0.0, true);
        ed.tick(&mut assets, 0.0, false);
        assert!((ed.doc.as_ref().unwrap().object((0, 0), 1).unwrap().pos[0] - 110.0).abs() < 0.6);
    }

    #[test]
    fn ground_layers_change_places_with_their_paint_and_undo() {
        let mut doc = flat_map(2);
        let g = |t: &str| omsi_map::GroundTex { texture: t.into(), detail_texture: String::new(), params: [8.0, 16.0, 4.0] };
        doc.global.ground_textures = vec![g("base.bmp"), g("sand.bmp"), g("rock.bmp")];
        doc.sync_mask_slots();
        let sand = vec![7u8; 256 * 256];
        doc.tiles.get_mut(&(0, 0)).unwrap().masks[1] = Some(sand.clone());
        let mut ed = Editor::default();
        ed.open(doc);
        ed.paint_layer = 1;
        // sand after rock: drawn over it, its paint along; the base stays first
        ed.move_ground_layer(1, true);
        let d = ed.doc.as_ref().unwrap();
        assert_eq!(d.global.ground_textures.iter().map(|g| g.texture.as_str()).collect::<Vec<_>>(), ["base.bmp", "rock.bmp", "sand.bmp"]);
        assert_eq!((d.tiles[&(0, 0)].masks[1].as_ref(), d.tiles[&(0, 0)].masks[2].as_ref()), (None, Some(&sand)));
        assert!(d.tiles[&(1, 0)].dirty);
        assert_eq!(ed.paint_layer, 2);
        ed.move_ground_layer(2, true);
        ed.move_ground_layer(1, false);
        assert_eq!(ed.doc.as_ref().unwrap().global.ground_textures[0].texture, "base.bmp");
        ed.set_ground_texture(2, true, "Texture\\noise_low.bmp");
        assert_eq!(ed.doc.as_ref().unwrap().global.ground_textures[2].detail_texture, "Texture\\noise_low.bmp");
        ed.undo();
        ed.undo();
        let d = ed.doc.as_ref().unwrap();
        assert_eq!(d.global.ground_textures[1].texture, "sand.bmp");
        assert_eq!(d.tiles[&(0, 0)].masks[1].as_ref(), Some(&sand));
    }

    #[test]
    fn an_object_leans_as_the_ground_slopes_under_it() {
        // the ground climbs 10 % to the east and 5 % to the north
        let ground = |p: DVec2| p.x * 0.10 + p.y * 0.05;
        for heading in [0.0, 37.0, 90.0, 200.0, 315.0] {
            let rot = lean_rotation(heading, &ground, DVec2::new(100.0, 100.0));
            assert_eq!(rot[0], heading);
            let m = object_matrix(DVec3::ZERO, rot);
            for v in [glam::Vec3::Y, glam::Vec3::X] {
                let w = m.transform_vector3(v).as_dvec3();
                let flat = w.truncate().normalize();
                let want = flat.x * 0.10 + flat.y * 0.05;
                let got = w.z / w.truncate().length();
                assert!((got - want).abs() < 0.01, "heading {heading}: {got} {want}");
            }
        }
        // flat ground: upright
        assert_eq!(lean_rotation(45.0, &|_| 3.0, DVec2::ZERO), [45.0, 0.0, 0.0]);
    }

    fn editor() -> Editor {
        let mut doc = flat_map(1);
        add_object(&mut doc, (0, 0), 1, 100.0, 100.0);
        add_object(&mut doc, (0, 0), 2, 110.0, 100.0);
        add_spline(&mut doc, (0, 0), 3, 50.0, 50.0, 0.0, 30.0, 0, 0);
        let mut ed = Editor::default();
        ed.open(doc);
        ed
    }

    /// A road made longer by numbers: the road joined at its end starts where it now ends,
    /// tangent, and still ends where it did.
    #[test]
    fn a_length_typed_carries_the_next_road() {
        let mut doc = flat_map(1);
        add_spline(&mut doc, (0, 0), 1, 50.0, 50.0, 0.0, 30.0, 0, 2);
        add_spline(&mut doc, (0, 0), 2, 50.0, 80.0, 0.0, 60.0, 1, 0);
        let mut ed = Editor::default();
        ed.open(doc);
        let mut assets = Assets::new(std::env::temp_dir().join("openmapper-no-omsi"), std::env::temp_dir().join("openmapper-no-content"));
        ed.shape_request = Some(((0, 0), 1, 40.0, 0.0));
        ed.tick(&mut assets, 0.016, false);
        let doc = ed.doc.as_ref().unwrap();
        assert_eq!(doc.spline((0, 0), 1).unwrap().length, 40.0);
        let c = omsi_geometry::SplineCurve::from_map(doc.spline((0, 0), 2).unwrap(), DVec2::ZERO);
        assert!((c.point_at(0.0).truncate() - DVec2::new(50.0, 90.0)).length() < 1e-6, "{:?}", c.point_at(0.0));
        assert!((c.end_point().truncate() - DVec2::new(50.0, 140.0)).length() < 1e-3, "{:?}", c.end_point());
        // and back with one undo
        ed.undo();
        assert_eq!(ed.doc.as_ref().unwrap().spline((0, 0), 1).unwrap().length, 30.0);
    }

    #[test]
    fn objects_turn_at_random_or_alike() {
        let mut ed = editor();
        ed.set_selection(vec![Sel::Object((0, 0), 1), Sel::Object((0, 0), 2)]);
        ed.doc.as_mut().unwrap().object_mut((0, 0), 1).unwrap().rot[0] = 33.0;
        ed.turn_objects(false);
        assert_eq!(ed.doc.as_ref().unwrap().object((0, 0), 2).unwrap().rot[0], 33.0);
        ed.turn_objects(true);
        let (a, b) = (ed.doc.as_ref().unwrap().object((0, 0), 1).unwrap().rot[0], ed.doc.as_ref().unwrap().object((0, 0), 2).unwrap().rot[0]);
        assert!(a != b && (0.0..360.0).contains(&a) && (0.0..360.0).contains(&b));
    }

    #[test]
    fn rolls_are_smooth_and_bounded() {
        let mut last = rolls(DVec2::ZERO);
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for i in 1..2000 {
            let v = rolls(DVec2::new(i as f64 * 0.5, i as f64 * 0.21));
            assert!((-1.0..=1.0).contains(&v));
            assert!((v - last).abs() < 0.2, "a step of half a metre stays small");
            (lo, hi, last) = (lo.min(v), hi.max(v), v);
        }
        assert!(hi - lo > 0.6, "it does roll: {lo}..{hi}");
    }

    /// A slope under a 20 × 10 m house is level with its foot, inside its outline and not
    /// far off it.
    #[test]
    fn the_ground_is_levelled_under_an_outline() {
        let mut doc = flat_map(1);
        let t = doc.tiles.get_mut(&(0, 0)).unwrap();
        let s = TERRAIN_SAMPLES;
        let cell = tile_size() / (s - 1) as f64;
        for j in 0..s {
            for i in 0..s {
                t.terrain.heights[j * s + i] = (i as f64 * cell * 0.1) as f32;
            }
        }
        let mut hist = History::default();
        let centre = DVec2::new(150.0, 150.0);
        level_ground(&mut doc, &mut hist, centre, 0.0, (Vec3::new(-10.0, -5.0, 0.0), Vec3::new(10.0, 5.0, 8.0)), 15.0);
        for p in [DVec2::new(141.0, 146.0), DVec2::new(159.0, 154.0), centre] {
            assert!((doc.ground(p) as f64 - 15.0).abs() < 0.3, "{p}: {}", doc.ground(p));
        }
        // well outside, the slope as it was
        assert!((doc.ground(DVec2::new(180.0, 150.0)) as f64 - 18.0).abs() < 0.1);
    }

    #[test]
    fn an_object_hangs_on_another() {
        let mut ed = editor();
        ed.attach_object((0, 0), 1, 2, "Sceneryobjects\\sign.sco");
        let t = &ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile;
        let a = t.attach_objects.last().unwrap();
        assert_eq!((a.parent_id, a.attach_index), (Some(1), 2));
        assert_eq!(ed.selected, Some(Sel::Object((0, 0), a.id)));
        ed.undo();
        assert!(ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.attach_objects.is_empty());
    }

    #[test]
    fn path_rules_are_set_in_one_step() {
        let mut ed = editor();
        let r = omsi_map::MapRule { path_index: 1, kind: "speedlimit".into(), value: 30.0, extra: 0.0, kill: false };
        ed.set_rules(Sel::Spline((0, 0), 3), vec![r.clone()]);
        ed.set_rules(Sel::Spline((0, 0), 3), vec![omsi_map::MapRule { value: 40.0, ..r }]);
        assert_eq!(ed.doc.as_ref().unwrap().spline((0, 0), 3).unwrap().rules[0].value, 40.0);
        ed.undo();
        assert!(ed.doc.as_ref().unwrap().spline((0, 0), 3).unwrap().rules.is_empty());
    }

    #[test]
    fn all_alike_are_selected() {
        let mut ed = editor();
        assert_eq!(ed.select_alike(Sel::Object((0, 0), 2)), 2);
        assert_eq!(ed.selected, Some(Sel::Object((0, 0), 2)));
        assert_eq!(ed.select_alike(Sel::Spline((0, 0), 3)), 1);
    }

    #[test]
    fn objects_align_to_the_road_beside_them() {
        let mut ed = editor();
        // (the road from (50, 50) runs north; turn it east)
        ed.doc.as_mut().unwrap().tiles.get_mut(&(0, 0)).unwrap().tile.splines[0].heading = 90.0;
        ed.set_selection(vec![Sel::Object((0, 0), 1)]);
        ed.doc.as_mut().unwrap().object_mut((0, 0), 1).unwrap().pos = [70.0, 60.0, 0.0];
        assert_eq!(ed.align_to_roads(), 1);
        assert!((ed.doc.as_ref().unwrap().object((0, 0), 1).unwrap().rot[0] - 90.0).abs() < 1e-6);
    }

    #[test]
    fn copies_in_a_row_stand_a_step_apart() {
        let mut ed = editor();
        ed.select(Some(Sel::Object((0, 0), 1)));
        ed.array(&Library::default(), 3, DVec2::new(10.0, 0.0), 90.0, 1, DVec2::ZERO);
        let doc = ed.doc.as_ref().unwrap();
        let xs: Vec<(f64, f64)> = doc.tiles[&(0, 0)].tile.objects.iter().skip(2).map(|o| (o.pos[0], o.rot[0])).collect();
        assert_eq!(xs, vec![(110.0, 90.0), (120.0, 180.0), (130.0, 270.0)]);
        assert_eq!(ed.selection().len(), 4);
        ed.undo();
        assert_eq!(ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.objects.len(), 2);
    }

    #[test]
    fn a_wire_is_laid_on_a_track_piece_for_piece() {
        let mut doc = flat_map(1);
        add_spline(&mut doc, (0, 0), 10, 50.0, 50.0, 0.0, 30.0, 0, 11);
        add_spline(&mut doc, (0, 0), 11, 50.0, 80.0, 0.0, 30.0, 10, 0);
        let mut ed = Editor::default();
        ed.open(doc);
        ed.set_selection(vec![Sel::Spline((0, 0), 10), Sel::Spline((0, 0), 11)]);
        assert_eq!(ed.wire_along("Splines\\wire.sli"), 2);
        let t = &ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile;
        let w: Vec<&MapSpline> = t.splines.iter().filter(|s| s.file.ends_with("wire.sli")).collect();
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].next_id, w[1].prev_id), (w[1].id, w[0].id));
        assert_eq!((w[0].pos, w[0].length), ([50.0, 50.0, 0.0], 30.0));
        // (the track itself as it was)
        assert_eq!(t.splines.iter().find(|s| s.id == 10).unwrap().next_id, 11);
    }

    #[test]
    fn copies_in_rows_make_a_grid() {
        let mut ed = editor();
        ed.select(Some(Sel::Object((0, 0), 1)));
        ed.array(&Library::default(), 2, DVec2::new(10.0, 0.0), 0.0, 3, DVec2::new(0.0, 20.0));
        let doc = ed.doc.as_ref().unwrap();
        let mut at: Vec<(i64, i64)> = doc.tiles[&(0, 0)].tile.objects.iter().skip(2).map(|o| (o.pos[0] as i64, o.pos[1] as i64)).collect();
        at.sort();
        assert_eq!(at, vec![(100, 120), (100, 140), (110, 100), (110, 120), (110, 140), (120, 100), (120, 120), (120, 140)]);
    }

    #[test]
    fn a_ring_of_tiles_is_one_step() {
        let mut ed = editor();
        assert_eq!(ed.grow_map(), 8);
        assert_eq!(ed.doc.as_ref().unwrap().tiles.len(), 9);
        assert_eq!(ed.grow_map(), 16);
        ed.undo();
        assert_eq!(ed.doc.as_ref().unwrap().tiles.len(), 9);
    }

    #[test]
    fn replacing_a_type_everywhere_is_one_step() {
        let mut ed = editor();
        let from = ed.doc.as_ref().unwrap().object((0, 0), 1).unwrap().file.clone();
        let n = ed.replace_everywhere(&from.to_uppercase(), "Sceneryobjects\\x\\new.sco", false);
        assert_eq!(n, 2);
        assert_eq!(ed.doc.as_ref().unwrap().object((0, 0), 2).unwrap().file, "Sceneryobjects\\x\\new.sco");
        ed.undo();
        assert_eq!(ed.doc.as_ref().unwrap().object((0, 0), 2).unwrap().file, from);
    }

    #[test]
    fn shift_click_adds_and_takes_away() {
        let mut ed = editor();
        ed.select(Some(Sel::Object((0, 0), 1)));
        ed.toggle_selected(Sel::Spline((0, 0), 3));
        assert_eq!(ed.selection().len(), 2);
        assert_eq!(ed.selected, Some(Sel::Spline((0, 0), 3)), "the one added shows in the properties");
        ed.toggle_selected(Sel::Spline((0, 0), 3));
        assert_eq!(ed.selection(), vec![Sel::Object((0, 0), 1)]);
        // nothing selected: what `also` held does not count
        ed.toggle_selected(Sel::Object((0, 0), 2));
        ed.selected = None;
        assert!(ed.selection().is_empty());
        ed.toggle_selected(Sel::Object((0, 0), 1));
        assert_eq!(ed.selection(), vec![Sel::Object((0, 0), 1)]);
    }

    #[test]
    fn a_selection_turns_deletes_and_comes_back_in_single_steps() {
        let mut ed = editor();
        ed.select_all();
        assert_eq!(ed.selection().len(), 3);
        ed.rotate(90.0);
        let doc = ed.doc.as_ref().unwrap();
        assert!((doc.spline((0, 0), 3).unwrap().heading - 90.0).abs() < 1e-9);
        assert!((doc.object((0, 0), 1).unwrap().rot[0] - 90.0).abs() < 1e-9);
        ed.delete_selected();
        let doc = ed.doc.as_ref().unwrap();
        assert!(doc.tiles[&(0, 0)].tile.objects.is_empty() && doc.tiles[&(0, 0)].tile.splines.is_empty());
        let d = ed.doc.as_mut().unwrap();
        ed.history.undo(d);
        assert_eq!(ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.objects.len(), 2);
        let d = ed.doc.as_mut().unwrap();
        ed.history.undo(d);
        let doc = ed.doc.as_ref().unwrap();
        assert_eq!(doc.spline((0, 0), 3).unwrap().heading, 0.0);
    }

    #[test]
    fn copy_paste_and_duplicate_make_new_things_round_the_cursor() {
        let mut ed = editor();
        let lib = Library::default();
        ed.set_selection(vec![Sel::Object((0, 0), 1), Sel::Object((0, 0), 2)]);
        assert_eq!(ed.copy(&lib), 2);
        ed.cursor = Some(DVec3::new(200.0, 200.0, 0.0));
        ed.paste(&lib);
        let pasted = ed.selection();
        assert_eq!(pasted.len(), 2);
        let doc = ed.doc.as_ref().unwrap();
        let xs: Vec<f64> = pasted.iter().map(|s| match *s { Sel::Object(k, id) => doc.object(k, id).unwrap().pos[0], _ => 0.0 }).collect();
        assert_eq!(xs, vec![195.0, 205.0], "kept 10 m apart round the cursor");
        ed.duplicate(&lib);
        assert_eq!(ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.objects.len(), 6);
        // one undo takes the duplicate away again
        let d = ed.doc.as_mut().unwrap();
        ed.history.undo(d);
        assert_eq!(ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.objects.len(), 4);
    }

    /// A junction at (150, 150) holding, 10 m north of it, the start of a road running on north.
    fn junction_editor() -> Editor {
        let mut doc = flat_map(1);
        add_object(&mut doc, (0, 0), 1, 150.0, 150.0);
        add_spline(&mut doc, (0, 0), 3, 150.0, 160.0, 0.0, 40.0, 0, 0);
        let mut ed = Editor::default();
        ed.known_helpers.insert("sceneryobjects\\x.sco".into(), (false, vec![[0.0, 10.0, 0.0, 0.0]]));
        ed.open(doc);
        ed.select(Some(Sel::Object((0, 0), 1)));
        ed
    }

    fn start_of(ed: &Editor, id: i64) -> (DVec2, f64) {
        let (k, s) = ed.doc.as_ref().unwrap().find_spline(id).unwrap();
        let c = omsi_geometry::SplineCurve::from_map(s, tile_origin(k));
        (c.point_at(0.0).truncate(), roads::norm_deg(c.heading_at(0.0)))
    }

    #[test]
    fn a_moved_junction_carries_its_roads_ends_along() {
        let mut ed = junction_editor();
        ed.follow_ground = false;
        ed.move_selection_by(DVec2::new(5.0, 0.0));
        let (p, h) = start_of(&ed, 3);
        assert!((p - DVec2::new(155.0, 160.0)).length() < 1e-6, "{p:?}");
        assert!(h < 1e-6 || h > 360.0 - 1e-6, "leaves the junction northwards: {h}");
        // the far end stays
        let s = ed.doc.as_ref().unwrap().spline((0, 0), 3).unwrap().clone();
        let mut cur = s;
        while cur.next_id != 0 {
            cur = ed.doc.as_ref().unwrap().find_spline(cur.next_id).unwrap().1.clone();
        }
        let c = omsi_geometry::SplineCurve::from_map(&cur, DVec2::ZERO);
        assert!((c.end_point().truncate() - DVec2::new(150.0, 200.0)).length() < 1e-6);
        // and one undo puts it all back
        let d = ed.doc.as_mut().unwrap();
        ed.history.undo(d);
        assert!((start_of(&ed, 3).0 - DVec2::new(150.0, 160.0)).length() < 1e-9);
    }

    #[test]
    fn a_turned_junction_turns_its_roads_ends() {
        let mut ed = junction_editor();
        ed.follow_ground = false;
        ed.rotate(90.0);
        let (p, h) = start_of(&ed, 3);
        assert!((p - DVec2::new(160.0, 150.0)).length() < 1e-6, "{p:?}");
        assert!((h - 90.0).abs() < 1e-6, "{h}");
    }

    #[test]
    fn raising_a_road_brings_the_neighbour_joined_outside_with_it() {
        let mut doc = flat_map(1);
        add_spline(&mut doc, (0, 0), 1, 100.0, 50.0, 0.0, 50.0, 0, 2);
        add_spline(&mut doc, (0, 0), 2, 100.0, 100.0, 0.0, 50.0, 1, 0);
        let mut ed = Editor::default();
        ed.open(doc);
        ed.select(Some(Sel::Spline((0, 0), 2)));
        ed.raise_selection(1.5);
        let doc = ed.doc.as_ref().unwrap();
        let c1 = omsi_geometry::SplineCurve::from_map(doc.spline((0, 0), 1).unwrap(), DVec2::ZERO);
        assert!((c1.end_point().z - 1.5).abs() < 1e-9 && c1.point_at(0.0).z.abs() < 1e-9);
    }

    #[test]
    fn deleting_an_arm_queues_its_junction_to_be_made_again() {
        let mut doc = flat_map(1);
        let file = "Sceneryobjects\\openMapper\\T\\junction_1.sco";
        doc.tiles.get_mut(&(0, 0)).unwrap().tile.objects.push(omsi_map::MapObject { file: file.into(), id: 1, pos: [150.0, 150.0, 0.0], ..Default::default() });
        add_spline(&mut doc, (0, 0), 3, 150.0, 160.0, 0.0, 40.0, 0, 0);
        let mut ed = Editor::default();
        ed.known_helpers.insert(file.to_lowercase(), (true, vec![[0.0, 10.0, 0.0, 0.0]]));
        ed.open(doc);
        assert!(is_own_junction(file) && !is_own_junction("Sceneryobjects\\Kreuz_MC\\x.sco"));
        assert_eq!(ed.junctions_at_ends(&[Sel::Spline((0, 0), 3)]), vec![1]);
        // (the junction deleted with it is not made again)
        assert!(ed.junctions_at_ends(&[Sel::Spline((0, 0), 3), Sel::Object((0, 0), 1)]).is_empty());
        ed.select(Some(Sel::Spline((0, 0), 3)));
        ed.delete_selected();
        assert_eq!(ed.pending_rebuild, vec![1]);
        assert!(ed.history.is_open(), "the step stays open for the plate");
    }

    #[test]
    fn snapping_rounds_to_the_grid_and_to_whole_steps() {
        let mut ed = Editor::default();
        assert_eq!(ed.snap_point(DVec2::new(1.26, 3.74)), DVec2::new(1.26, 3.74));
        ed.snap_grid = true;
        ed.grid_step = 0.5;
        assert_eq!(ed.snap_point(DVec2::new(1.26, 3.74)), DVec2::new(1.5, 3.5));
        assert_eq!(ed.snap_heading(52.0), 52.0);
        ed.snap_angle = true;
        assert_eq!(ed.snap_heading(52.0), 45.0);
        assert_eq!(ed.snap_heading(358.0), 0.0);
    }

    #[test]
    fn numbers_in_the_properties_move_an_object_over_into_the_next_tile() {
        let mut doc = flat_map(2);
        add_object(&mut doc, (0, 0), 1, 100.0, 100.0);
        let mut ed = Editor::default();
        ed.open(doc);
        ed.select(Some(Sel::Object((0, 0), 1)));
        ed.put_object((0, 0), 1, DVec2::new(350.0, 20.0));
        assert_eq!(ed.selected, Some(Sel::Object((1, 0), 1)));
        let o = ed.doc.as_ref().unwrap().object((1, 0), 1).unwrap();
        assert!((o.pos[0] - 50.0).abs() < 1e-9 && (o.pos[1] - 20.0).abs() < 1e-9);
    }

    #[test]
    fn a_point_put_into_an_ai_path_splits_it_and_undoes() {
        let mut ed = editor();
        ed.tool = Tool::Logic;
        ed.logic = LogicMode::Paths;
        ed.path_edit = true;
        // the spline runs north from (50, 50) for 30 m
        ed.split_path_at((0, 0), 3, DVec2::new(51.0, 62.0));
        let doc = ed.doc.as_ref().unwrap();
        let a = doc.spline((0, 0), 3).unwrap();
        assert!((a.length - 12.0).abs() < 0.3, "{}", a.length);
        let b = doc.spline((0, 0), a.next_id).unwrap();
        assert_eq!(b.prev_id, 3);
        assert!((a.length + b.length - 30.0).abs() < 1e-6);
        // too near an end: nothing happens
        ed.split_path_at((0, 0), 3, DVec2::new(50.0, 50.3));
        assert_eq!(ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.splines.len(), 2);
        let d = ed.doc.as_mut().unwrap();
        ed.history.undo(d);
        let doc = ed.doc.as_ref().unwrap();
        assert_eq!(doc.tiles[&(0, 0)].tile.splines.len(), 1);
        assert!((doc.spline((0, 0), 3).unwrap().length - 30.0).abs() < 1e-9);
    }

    /// A junction of an earlier session deleted, then the map saved: its files stay while
    /// undo can still bring it back.
    #[test]
    fn junction_files_undo_can_bring_back_are_kept() {
        let content = std::env::temp_dir().join(format!("openmapper-test-jfiles-{}", std::process::id()));
        let mut doc = flat_map(1);
        doc.dir = content.join("maps").join("J");
        let file = "Sceneryobjects\\openMapper\\J\\junction_5.sco";
        doc.tiles.get_mut(&(0, 0)).unwrap().tile.objects.push(omsi_map::MapObject { file: file.into(), id: 5, pos: [150.0, 150.0, 0.0], ..Default::default() });
        let dir = content.join("Sceneryobjects").join("openMapper").join("J");
        std::fs::create_dir_all(dir.join("model")).unwrap();
        std::fs::write(dir.join("junction_5.sco"), "x").unwrap();
        let mut ed = Editor::default();
        ed.content_dir = Some(content.clone());
        ed.open(doc);
        ed.select(Some(Sel::Object((0, 0), 5)));
        ed.delete_selected();
        ed.history.commit();
        assert_eq!(ed.clean_junction_files(), 0);
        assert!(dir.join("junction_5.sco").exists());
        // (nothing can bring it back any more: it goes)
        ed.history.clear();
        assert_eq!(ed.clean_junction_files(), 1);
        let _ = std::fs::remove_dir_all(&content);
    }

    /// A junction file an earlier version of the map (kept by a save) names stays: going
    /// back to that version brings the junction back.
    #[test]
    fn junction_files_an_earlier_version_names_are_kept() {
        let content = std::env::temp_dir().join(format!("openmapper-test-jbackup-{}", std::process::id()));
        let mut doc = flat_map(1);
        doc.dir = content.join("maps").join("J");
        let dir = content.join("Sceneryobjects").join("openMapper").join("J");
        std::fs::create_dir_all(dir.join("model")).unwrap();
        std::fs::write(dir.join("junction_5.sco"), "x").unwrap();
        std::fs::create_dir_all(&doc.dir).unwrap();
        std::fs::write(doc.dir.join("tile_0_0.map"), "[object]\n0\nSceneryobjects\\openMapper\\J\\junction_5.sco\n5\n").unwrap();
        // a save that replaced that tile keeps it in a backup
        crate::doc::safe::backup(&doc.dir, &[crate::doc::safe::Op::Write(doc.dir.join("tile_0_0.map"), Vec::new())], &[]).unwrap();
        let mut ed = Editor::default();
        ed.content_dir = Some(content.clone());
        ed.open(doc);
        assert_eq!(ed.clean_junction_files(), 0);
        assert!(dir.join("junction_5.sco").exists());
        let _ = std::fs::remove_dir_all(&content);
    }

    /// Undo while drawing on from a road just built: the drawing stops (it went on from an
    /// end the undo took away, and the next road would have been linked to a spline that
    /// is not there).
    #[test]
    fn undo_stops_drawing_from_what_it_takes_away() {
        let mut ed = editor();
        ed.set_tool(Tool::Roads);
        ed.history.begin("Build road");
        let d = ed.doc.as_mut().unwrap();
        ed.history.touch_tile(d, (0, 0));
        add_spline(d, (0, 0), 7, 50.0, 100.0, 0.0, 30.0, 0, 0);
        ed.history.commit();
        ed.road_start = Some((Anchor { pos: DVec3::new(50.0, 130.0, 0.0), heading: 0.0, spline: Some(((0, 0), 7, true)), mid: None }, true));
        ed.undo();
        assert!(ed.doc.as_ref().unwrap().spline((0, 0), 7).is_none());
        assert!(ed.road_start.is_none());
        ed.redo();
        assert!(ed.doc.as_ref().unwrap().spline((0, 0), 7).is_some());
    }

    /// With Auto-Terrain off a new road runs level from where it starts, over a slope too (on,
    /// it takes the ground's height along its way).
    #[test]
    fn a_road_runs_level_over_a_slope_with_auto_terrain_off() {
        let rise = |auto: bool| {
            let mut ed = editor();
            ed.follow_ground = false;
            ed.auto_terrain = auto;
            {
                let t = ed.doc.as_mut().unwrap().tiles.get_mut(&(0, 0)).unwrap();
                let n = (t.terrain.heights.len() as f64).sqrt() as usize;
                for (i, h) in t.terrain.heights.iter_mut().enumerate() {
                    *h = ((i % n) + (i / n)) as f32 * 0.3;
                }
            }
            let seg = roads::straight(DVec2::new(60.0, 120.0), DVec2::new(60.0, 220.0)).unwrap();
            let start = Anchor { pos: DVec3::new(60.0, 120.0, 4.0), heading: 0.0, ..Default::default() };
            let made = ed.build_road("Splines\\road.sli", &[seg], 4.0, 4.0, start, None, 4.0).unwrap();
            ed.history.commit();
            let doc = ed.doc.as_ref().unwrap();
            let zs: Vec<f64> = made.iter().map(|(k, id)| doc.spline(*k, *id).unwrap().pos[2]).collect();
            zs.iter().cloned().fold(f64::MIN, f64::max) - zs.iter().cloned().fold(f64::MAX, f64::min)
        };
        assert!(rise(false) < 1e-6, "{}", rise(false));
        assert!(rise(true) > 0.5, "{}", rise(true));
    }

    /// A road drawn from an end that is gone meanwhile (deleted while drawing) starts free:
    /// it is not linked to a spline that is not there.
    #[test]
    fn a_road_from_an_end_that_went_is_not_linked_to_it() {
        let mut ed = editor();
        ed.follow_ground = false;
        let seg = roads::straight(DVec2::new(50.0, 130.0), DVec2::new(50.0, 160.0)).unwrap();
        let start = Anchor { pos: DVec3::new(50.0, 130.0, 0.0), heading: 0.0, spline: Some(((0, 0), 99, true)), mid: None };
        let made = ed.build_road("Splines\\road.sli", &[seg], 0.0, 0.0, start, None, 4.0).unwrap();
        ed.history.commit();
        let (k, id) = made[0];
        assert_eq!(ed.doc.as_ref().unwrap().spline(k, id).unwrap().prev_id, 0);
    }

    /// The junction options shown in the properties are those of the map open: another
    /// map's junction of the same IDCode does not show (and get made again with) them.
    #[test]
    fn junction_options_are_not_carried_over_to_another_map() {
        let mut ed = editor();
        ed.junction_options.insert(5, crate::junction::Options { lights: true, ..Default::default() });
        ed.open(flat_map(1));
        assert!(ed.junction_options.is_empty());
    }

    /// The installation's spline, loaded (None without an OMSI 2 here).
    fn installed_spline(rel: &str) -> Option<Assets> {
        let omsi = crate::install::find_omsi_root(&Default::default())?;
        let mut a = Assets::new(omsi, std::env::temp_dir().join("openmapper-test-empty-content"));
        for _ in 0..500 {
            if a.spline(rel).is_some() {
                return Some(a);
            }
            if a.spline_failed(rel) {
                return None;
            }
            a.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn a_track_drawn_from_a_track_lays_a_switch() {
        let rail = "Splines\\Ruede\\rail_concrete_01.sli";
        let Some(mut assets) = installed_spline(rail) else {
            eprintln!("no OMSI 2 here: skipped");
            return;
        };
        // (the switch's model loaded too: the plan waits for it)
        let sw = crate::switches::object_for(rail, false);
        for _ in 0..500 {
            if assets.object(&sw).is_some() || assets.object_failed(&sw) {
                break;
            }
            assets.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(assets.object(&sw).is_some(), "{sw} is in the installation");
        let mut doc = flat_map(1);
        add_spline(&mut doc, (0, 0), 3, 50.0, 50.0, 0.0, 150.0, 0, 0);
        doc.tiles.get_mut(&(0, 0)).unwrap().tile.splines[0].file = rail.into();
        let mut ed = Editor::default();
        ed.open(doc);
        ed.set_tool(Tool::Roads);
        ed.road_type = Some(rail.into());
        ed.inherit_type = false;
        let lib = Library::default();
        // start on the track's middle, go off to the north-east (to the right)
        ed.cursor = Some(DVec3::new(50.3, 100.0, 0.0));
        ed.plan_road(&mut assets);
        assert!(ed.road_snap.map(|a| a.mid.is_some()).unwrap_or(false), "the track's middle is a place to start");
        ed.road_click(&mut assets, &lib);
        ed.cursor = Some(DVec3::new(80.0, 180.0, 0.0));
        ed.plan_road(&mut assets);
        assert_eq!(ed.switch_plans.len(), 1);
        ed.road_click(&mut assets, &lib);
        let doc = ed.doc.as_ref().unwrap();
        let t = &doc.tiles[&(0, 0)].tile;
        // a right-hand switch at the toe, facing north
        let o = t.objects.iter().find(|o| o.file.contains("EW190_R")).expect("a switch object");
        assert!(o.rot[0].abs() < 1.0 || (o.rot[0] - 360.0).abs() < 1.0, "{}", o.rot[0]);
        // the main track: up to the toe, then on again from the heels, 27 m on
        let old = doc.spline((0, 0), 3).unwrap();
        assert!((old.length - 50.0).abs() < 0.5, "{}", old.length);
        assert!(t.splines.iter().any(|s| (s.pos[1] - 127.0).abs() < 0.6 && s.pos[0].abs() - 50.0 < 0.1), "the track goes on from the straight heel");
        // the branch leaves the diverging heel, a little right and turned a little right
        let branch = t.splines.iter().find(|s| s.pos[0] > 51.0).expect("the new track");
        assert!((branch.pos[0] - 51.82).abs() < 0.2 && (branch.pos[1] - 126.9).abs() < 0.3, "{:?}", branch.pos);
        assert!((branch.heading - 6.34).abs() < 0.5, "{}", branch.heading);
    }

    #[test]
    fn a_track_drawn_off_a_track_to_the_left_lays_a_left_switch() {
        let rail = "Splines\\Ruede\\rail_concrete_01.sli";
        let Some(mut assets) = installed_spline(rail) else {
            eprintln!("no OMSI 2 here: skipped");
            return;
        };
        for left in [false, true] {
            let sw = crate::switches::object_for(rail, left);
            for _ in 0..500 {
                if assets.object(&sw).is_some() || assets.object_failed(&sw) {
                    break;
                }
                assets.poll();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let mut doc = flat_map(1);
        add_spline(&mut doc, (0, 0), 3, 50.0, 50.0, 0.0, 150.0, 0, 0);
        doc.tiles.get_mut(&(0, 0)).unwrap().tile.splines[0].file = rail.into();
        let mut ed = Editor::default();
        ed.open(doc);
        ed.set_tool(Tool::Roads);
        ed.road_type = Some(rail.into());
        ed.inherit_type = false;
        let lib = Library::default();
        ed.cursor = Some(DVec3::new(50.3, 100.0, 0.0));
        ed.plan_road(&mut assets);
        ed.road_click(&mut assets, &lib);
        // off to the north-west (to the left)
        ed.cursor = Some(DVec3::new(20.0, 180.0, 0.0));
        ed.plan_road(&mut assets);
        eprintln!("plans {} plan {:?}", ed.switch_plans.len(), ed.road_plan.as_ref().map(|p| p.0.len()));
        assert_eq!(ed.switch_plans.len(), 1);
        ed.road_click(&mut assets, &lib);
        let doc = ed.doc.as_ref().unwrap();
        let t = &doc.tiles[&(0, 0)].tile;
        for s in &t.splines {
            eprintln!("spline {} ({:.1},{:.1}) h {:.1} len {:.1} prev {} next {}", s.id, s.pos[0], s.pos[1], s.heading, s.length, s.prev_id, s.next_id);
        }
        let o = t.objects.iter().find(|o| o.file.contains("EW190_L")).expect("a left switch object");
        assert!(o.rot[0].abs() < 1.0 || (o.rot[0] - 360.0).abs() < 1.0, "{}", o.rot[0]);
        let branch = t.splines.iter().find(|s| s.pos[0] < 49.0).expect("the new track");
        assert!((branch.pos[0] - 48.18).abs() < 0.3, "{:?}", branch.pos);
    }

    #[test]
    fn a_roads_lanes_become_paths_of_their_own() {
        let road = "Splines\\Marcel\\str_2spur_8m_borkumer1.sli";
        let Some(mut assets) = installed_spline(road) else { return };
        let omsi = crate::install::find_omsi_root(&Default::default()).unwrap();
        let content = std::env::temp_dir().join(format!("om-detach-{}", std::process::id()));
        std::fs::create_dir_all(&content).unwrap();
        let roots = crate::install::Roots { omsi, content: content.clone() };
        for f in ["Splines\\Marcel\\invis_street.sli", "Splines\\invis_sidewalk.sli"] {
            while assets.spline(f).is_none() && !assets.spline_failed(f) {
                assets.poll();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        let mut doc = flat_map(1);
        add_spline(&mut doc, (0, 0), 3, 80.0, 80.0, 30.0, 60.0, 0, 0);
        doc.tiles.get_mut(&(0, 0)).unwrap().tile.splines[0].file = road.into();
        doc.tiles.get_mut(&(0, 0)).unwrap().tile.splines[0].radius = -150.0;
        let mut ed = Editor::default();
        ed.open(doc);
        // the lanes as they are, as (where traffic starts, where it ends)
        let ways = |ed: &Editor, assets: &mut Assets| -> Vec<(i64, i64, i64, i64)> {
            let doc = ed.doc.as_ref().unwrap();
            let r = |v: f64| (v * 10.0).round() as i64;
            let mut out = Vec::new();
            for l in crate::lanes::near(doc, assets, DVec2::new(100.0, 100.0), 100.0).0 {
                let (a, b) = (l.pts[0], *l.pts.last().unwrap());
                let fwd = (r(a.x), r(a.y), r(b.x), r(b.y));
                let back = (r(b.x), r(b.y), r(a.x), r(a.y));
                match l.direction {
                    0 => out.push(fwd),
                    1 => out.push(back),
                    _ => {
                        out.push(fwd);
                        out.push(back);
                    }
                }
            }
            out.sort();
            out
        };
        let before = ways(&ed, &mut assets);
        assert!(before.len() >= 4, "{before:?}");
        ed.select(Some(Sel::Spline((0, 0), 3)));
        ed.detach_lanes(&mut assets, &roots);
        for _ in 0..200 {
            assets.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let after = ways(&ed, &mut assets);
        assert_eq!(before.len(), after.len(), "{before:?}\n{after:?}");
        for (b, a) in before.iter().zip(&after) {
            // (within a decimetre or two: the lanes' ends are the same)
            assert!((b.0 - a.0).abs() <= 2 && (b.1 - a.1).abs() <= 2 && (b.2 - a.2).abs() <= 2 && (b.3 - a.3).abs() <= 2, "{b:?} vs {a:?}");
        }
        let t = &ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile;
        assert!(t.splines.iter().find(|s| s.id == 3).unwrap().file.contains("no_lanes"));
        assert!(t.splines.len() >= 5);
        assert!(content.join("Splines/openMapper/str_2spur_8m_borkumer1_no_lanes.sli").is_file());
        // one undo gives the road its lanes back
        let d = ed.doc.as_mut().unwrap();
        ed.history.undo(d);
        assert_eq!(ways(&ed, &mut assets), before);
        let _ = std::fs::remove_dir_all(content);
    }

    #[test]
    fn the_water_brush_digs_a_bed_and_lays_water_on_the_tile() {
        let mut assets = Assets::new(std::path::PathBuf::new(), std::env::temp_dir().join("openmapper-test-empty-content"));
        let mut ed = editor();
        ed.set_tool(Tool::Terrain);
        ed.terrain_mode = TerrainMode::Water;
        ed.brush_radius = 12.0;
        ed.water_depth = 2.0;
        ed.water_level = Some(-0.3);
        ed.history.begin("water");
        for _ in 0..40 {
            ed.brush_terrain(&mut assets, DVec2::new(100.0, 100.0), 0.05);
        }
        ed.history.commit();
        let doc = ed.doc.as_ref().unwrap();
        assert!(doc.ground(DVec2::new(100.0, 100.0)) < -2.0, "{}", doc.ground(DVec2::new(100.0, 100.0)));
        // (beyond the brush the ground is as it was)
        assert!(doc.ground(DVec2::new(130.0, 100.0)).abs() < 1e-3);
        assert_eq!(doc.tiles[&(0, 0)].water, Some([-0.3; 4]));
    }

    #[test]
    fn an_ai_path_is_bent_and_moved_not_lost() {
        let file = "Splines\\Marcel\\invis_street.sli";
        let Some(mut assets) = installed_spline(file) else { return };
        let lib = Library::default();
        let mut doc = flat_map(1);
        add_spline(&mut doc, (0, 0), 3, 50.0, 50.0, 0.0, 40.0, 0, 0);
        doc.tiles.get_mut(&(0, 0)).unwrap().tile.splines[0].file = file.into();
        let mut ed = Editor::default();
        ed.open(doc);
        ed.set_tool(Tool::Logic);
        ed.set_logic(LogicMode::Paths);
        ed.path_edit = true;
        let n = |ed: &Editor| ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.splines.iter().filter(|s| !s.deleted).count();
        let report = |ed: &Editor, what: &str| {
            let t = &ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile;
            let v: Vec<String> = t.splines.iter().filter(|s| !s.deleted).map(|s| format!("[{} ({:.1},{:.1}) h{:.0} l{:.1} r{:.0} {}>{}]", s.id, s.pos[0], s.pos[1], s.heading, s.length, s.radius, s.prev_id, s.next_id)).collect();
            eprintln!("{what}: {}", v.join(" "));
        };
        // a click on the path picks it (as the app does: pick under the cursor, then the press)
        ed.cursor = Some(DVec3::new(50.5, 70.0, 0.0));
        ed.hover = ed.pick(&mut assets, DVec3::new(50.5, 70.0, 80.0), DVec3::new(0.0, 0.0, -1.0), Some(DVec3::new(50.5, 70.0, 0.0)));
        eprintln!("hover {:?}", ed.hover);
        ed.left_press(&mut assets, &lib, false);
        ed.tick(&mut assets, 0.0, false);
        eprintln!("selected {:?}", ed.selected);
        // drag its far end sideways
        ed.cursor = Some(DVec3::new(50.0, 90.0, 0.0));
        let found = ed.handle_at_ray(&mut assets, DVec3::new(50.0, 90.0, 40.0), -DVec3::Z);
        ed.hover_handle = found.map(|f| f.1);
        ed.hover_handle_of = found.map(|f| f.0);
        eprintln!("handle {:?}", ed.hover_handle);
        ed.left_press(&mut assets, &lib, false);
        for x in [52.0, 56.0, 62.0] {
            ed.hover_point(&mut assets, &lib, DVec3::new(x, 88.0, 0.0));
            ed.tick(&mut assets, 0.05, true);
        }
        ed.tick(&mut assets, 0.0, false);
        report(&ed, "after the end drag");
        assert_eq!(n(&ed), 1, "still one path");
        // drag its middle
        ed.select(Some(Sel::Spline((0, 0), 3)));
        let mid = ed.handles().iter().find(|h| h.1 == Handle::Mid).map(|h| h.2).unwrap();
        ed.cursor = Some(mid);
        let found = ed.handle_at_ray(&mut assets, mid + DVec3::new(0.0, 0.0, 40.0), -DVec3::Z);
        ed.hover_handle = found.map(|f| f.1);
        ed.hover_handle_of = found.map(|f| f.0);
        ed.left_press(&mut assets, &lib, false);
        for d in [3.0, 8.0, 12.0] {
            ed.hover_point(&mut assets, &lib, DVec3::new(mid.x - d, mid.y, 0.0));
            ed.tick(&mut assets, 0.05, true);
        }
        ed.tick(&mut assets, 0.0, false);
        report(&ed, "after the middle drag");
        assert!(n(&ed) >= 1);
        // the whole path moved by a drag of its body (the path is picked whole, as a click picks it)
        let before = ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.splines.iter().filter(|s| !s.deleted).map(|s| s.pos[0]).fold(f64::MAX, f64::min);
        ed.select_chain((0, 0), 3);
        let at = ed.doc.as_ref().map(|d| omsi_geometry::SplineCurve::from_map(d.spline((0, 0), 3).unwrap(), tile_origin((0, 0))).point_at(5.0)).unwrap();
        ed.cursor = Some(at);
        ed.hover_handle = None;
        ed.hover = Some(Sel::Spline((0, 0), 3));
        ed.last_click = None;
        ed.left_press(&mut assets, &lib, false);
        for d in [2.0, 5.0, 9.0] {
            ed.hover_point(&mut assets, &lib, DVec3::new(at.x + d, at.y, 0.0));
            ed.tick(&mut assets, 0.05, true);
        }
        ed.tick(&mut assets, 0.0, false);
        report(&ed, "after the body drag");
        let after = ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.splines.iter().filter(|s| !s.deleted).map(|s| s.pos[0]).fold(f64::MAX, f64::min);
        assert!(after > before + 5.0, "{before} -> {after}");
        // a point put in with Ctrl + click, then that point dragged
        ed.split_path_at((0, 0), 3, DVec2::new(55.0, 70.0));
        report(&ed, "after a point put in");
        assert_eq!(n(&ed), 3);
    }

    #[test]
    fn an_ai_path_has_a_grip_at_every_point_and_a_point_can_be_taken_out() {
        let file = "Splines\\Marcel\\invis_street.sli";
        let Some(mut assets) = installed_spline(file) else { return };
        let lib = Library::default();
        let mut doc = flat_map(1);
        add_spline(&mut doc, (0, 0), 3, 50.0, 50.0, 0.0, 60.0, 0, 0);
        doc.tiles.get_mut(&(0, 0)).unwrap().tile.splines[0].file = file.into();
        let mut ed = Editor::default();
        ed.open(doc);
        ed.set_tool(Tool::Logic);
        ed.set_logic(LogicMode::Paths);
        ed.path_edit = true;
        // two points put in: three pieces
        ed.split_path_at((0, 0), 3, DVec2::new(50.0, 70.0));
        let second = ed.doc.as_ref().unwrap().spline((0, 0), 3).unwrap().next_id;
        ed.split_path_at((0, 0), second, DVec2::new(50.0, 90.0));
        let pieces = ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.splines.iter().filter(|s| !s.deleted).count();
        assert_eq!(pieces, 3);
        // a click on the path picks all of it, a grip for each point (4) and each piece's middle (3)
        ed.hover = Some(Sel::Spline((0, 0), 3));
        ed.last_click = None;
        ed.left_press(&mut assets, &lib, false);
        ed.tick(&mut assets, 0.0, false);
        assert_eq!(ed.selection().len(), 3);
        assert_eq!(ed.handles().iter().filter(|h| h.1 != Handle::Mid).count(), 4);
        assert_eq!(ed.handles().iter().filter(|h| h.1 == Handle::Mid).count(), 3);
        // the first joint picked (a press on its grip) and taken out with Del: two pieces, still one path
        let joint = ed.handles().iter().find(|h| h.1 == Handle::End).unwrap().clone();
        ed.cursor = Some(joint.2);
        ed.hover_handle = Some(joint.1);
        ed.hover_handle_of = Some(joint.0);
        ed.left_press(&mut assets, &lib, false);
        ed.tick(&mut assets, 0.0, false);
        assert!(ed.path_point.is_some());
        ed.delete_selected();
        let doc = ed.doc.as_ref().unwrap();
        let left: Vec<_> = doc.tiles[&(0, 0)].tile.splines.iter().filter(|s| !s.deleted).collect();
        assert_eq!(left.len(), 2, "{:?}", left.iter().map(|s| (s.id, s.prev_id, s.next_id, s.length)).collect::<Vec<_>>());
        assert!((left.iter().map(|s| s.length).sum::<f64>() - 60.0).abs() < 1.0);
        assert!(left.iter().any(|s| s.next_id != 0) && left.iter().any(|s| s.prev_id != 0));
        // undone, the path has its three pieces
        let d = ed.doc.as_mut().unwrap();
        ed.history.undo(d);
        assert_eq!(ed.doc.as_ref().unwrap().tiles[&(0, 0)].tile.splines.iter().filter(|s| !s.deleted).count(), 3);
    }

    #[test]
    fn flattening_the_terrain_levels_every_tile_and_lays_the_roads_on_it() {
        let mut ed = editor();
        {
            let t = ed.doc.as_mut().unwrap().tiles.get_mut(&(0, 0)).unwrap();
            for (i, h) in t.terrain.heights.iter_mut().enumerate() {
                *h = 5.0 + (i % 7) as f32;
            }
            t.tile.splines[0].pos[2] = 9.0;
        }
        let n = ed.flatten_terrain();
        assert!(n >= 1);
        let doc = ed.doc.as_ref().unwrap();
        let t = &doc.tiles[&(0, 0)];
        let first = t.terrain.heights[0];
        assert!(t.terrain.heights.iter().all(|h| (*h - first).abs() < 1e-6));
        assert!((first - 8.0).abs() < 0.2, "{first}");
        assert!((t.tile.splines[0].pos[2] - first as f64).abs() < 0.2, "{}", t.tile.splines[0].pos[2]);
        // one undo brings the relief back
        let d = ed.doc.as_mut().unwrap();
        ed.history.undo(d);
        let t = &ed.doc.as_ref().unwrap().tiles[&(0, 0)];
        assert!(t.terrain.heights.iter().any(|h| (*h - 5.0).abs() < 1e-6));
    }

    #[test]
    fn a_press_on_a_roads_own_lane_asks_for_its_lanes_to_be_made_editable() {
        let road = "Splines\\Marcel\\str_2spur_8m_borkumer1.sli";
        let Some(mut assets) = installed_spline(road) else { return };
        let lib = Library::default();
        let mut doc = flat_map(1);
        add_spline(&mut doc, (0, 0), 3, 50.0, 50.0, 0.0, 60.0, 0, 0);
        doc.tiles.get_mut(&(0, 0)).unwrap().tile.splines[0].file = road.into();
        let mut ed = Editor::default();
        ed.open(doc);
        ed.set_tool(Tool::Logic);
        ed.set_logic(LogicMode::Paths);
        ed.path_edit = true;
        // (a car lane of this road lies 2 m right of its centre line: east of x 50)
        ed.cursor = Some(DVec3::new(52.0, 80.0, 0.0));
        ed.hover = None;
        ed.hover_handle = None;
        ed.left_press(&mut assets, &lib, false);
        let (pieces, at) = ed.lane_detach_request.clone().expect("a request");
        assert_eq!(pieces, vec![((0, 0), 3)]);
        assert!((at - DVec2::new(52.0, 80.0)).length() < 1e-9);
    }

    #[test]
    fn each_track_gets_the_switch_that_looks_like_it() {
        let Some(mut assets) = installed_spline("Splines\\Ruede\\rail_wood_S49.sli") else {
            eprintln!("no OMSI 2 here: skipped");
            return;
        };
        let tracks = ["Splines\\Ruede\\rail_wood_S49.sli", "Splines\\Ruede\\rail_concrete_S49.sli", "Splines\\Ruede\\rail_wood_S41_kleinprofil_0.sli", "Splines\\Ruede\\rail_concrete_01.sli"];
        let names = ["Beton", "Klp_0_0", "Klp_0_1", "Klp_1_0", "Klp_1_1", "S49_0_0", "S49_0_1", "S49_1_0", "S49_1_1"];
        let mut ed = Editor::default();
        ed.switch_catalogue = names.iter().map(|n| crate::switches::Candidate { rel: format!("Sceneryobjects\\Railroad_RUE\\EW190_L_{n}.sco"), left: true }).collect();
        // (everything loaded first)
        for _ in 0..800 {
            let mut all = true;
            for c in ed.switch_catalogue.clone() {
                all &= assets.object(&c.rel).is_some() || assets.object_failed(&c.rel);
            }
            for t in tracks {
                all &= assets.spline(t).is_some() || assets.spline_failed(t);
            }
            if all {
                break;
            }
            assets.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ed.best_switch(&mut assets, tracks[0], true).ends_with("EW190_L_S49_0_0.sco"));
        assert!(ed.best_switch(&mut assets, tracks[1], true).ends_with("EW190_L_Beton.sco"));
        assert!(ed.best_switch(&mut assets, tracks[2], true).ends_with("EW190_L_Klp_0_0.sco"));
        assert!(ed.best_switch(&mut assets, tracks[3], true).ends_with("EW190_L_Beton.sco"));
    }

    #[test]
    fn a_road_into_another_makes_a_t_junction() {
        let road_s = std::env::var("TJ_MAIN").unwrap_or("Splines\\Marcel\\str_2spur_8m_borkumer1.sli".into());
        let road = road_s.as_str();
        let Some(mut assets) = installed_spline(road) else { return };
        let content = std::env::temp_dir().join(format!("om-tj-{}", std::process::id()));
        std::fs::create_dir_all(&content).unwrap();
        let mut doc = flat_map(1);
        doc.dir = content.join("maps").join("TJ");
        let mut ed = Editor::default();
        ed.content_dir = Some(content.clone());
        ed.open(doc);
        ed.set_tool(Tool::Roads);
        ed.road_type = Some(road.into());
        let lib = Library::default();
        let click = |ed: &mut Editor, a: &mut Assets, x: f64, y: f64| {
            ed.cursor = Some(DVec3::new(x, y, 0.0));
            ed.plan_road(a);
            ed.road_click(a, &lib);
            ed.tick(a, 0.0, false);
        };
        click(&mut ed, &mut assets, 60.0, 150.0);
        click(&mut ed, &mut assets, 240.0, 150.0);
        ed.right_click();
        if let Ok(b) = std::env::var("TJ_BRANCH") {
            while assets.spline(&b).is_none() && !assets.spline_failed(&b) { assets.poll(); std::thread::sleep(std::time::Duration::from_millis(10)); }
            ed.road_type = Some(b);
            ed.inherit_type = false;
        }
        click(&mut ed, &mut assets, 150.0, 260.0);
        click(&mut ed, &mut assets, 150.0, 151.0);
        ed.right_click();
        for _ in 0..50 { assets.poll(); ed.tick(&mut assets, 0.0, false); std::thread::sleep(std::time::Duration::from_millis(10)); }
        let doc = ed.doc.as_ref().unwrap();
        let t = &doc.tiles[&(0, 0)].tile;
        // one junction at the meeting point, three roads ending at its edge
        let j: Vec<_> = t.objects.iter().filter(|o| o.file.contains("junction_")).collect();
        assert_eq!(j.len(), 1);
        let centre = DVec2::new(j[0].pos[0], j[0].pos[1]);
        let ends = crate::selection::tests::free_ends_near(doc, centre, 25.0);
        assert_eq!(ends, 3, "three arms");
        // its file lost (the map moved without it): made again for the roads facing it
        let (old, old_file) = (j[0].id, j[0].file.clone());
        let file = omsi_cfg::resolve_path(&content, &old_file);
        std::fs::remove_file(&file).unwrap();
        let Some(mut fresh) = installed_spline(road) else { return };
        assert!(ed.remake_junction(old));
        for _ in 0..50 { fresh.poll(); ed.tick(&mut fresh, 0.0, false); std::thread::sleep(std::time::Duration::from_millis(10)); }
        let doc = ed.doc.as_ref().unwrap();
        let j: Vec<_> = doc.tiles[&(0, 0)].tile.objects.iter().filter(|o| o.file.contains("junction_")).collect();
        assert_eq!(j.len(), 1);
        assert_ne!(j[0].id, old);
        assert!(omsi_cfg::resolve_path(&content, &j[0].file).is_file());
        assert_eq!(crate::selection::tests::free_ends_near(doc, DVec2::new(j[0].pos[0], j[0].pos[1]), 25.0), 3);
        // every junction made again at once (one step), still one, for the same three roads
        assert_eq!(ed.remake_all_junctions(), 1);
        for _ in 0..50 { fresh.poll(); ed.tick(&mut fresh, 0.0, false); std::thread::sleep(std::time::Duration::from_millis(10)); }
        let doc = ed.doc.as_ref().unwrap();
        let j: Vec<_> = doc.tiles[&(0, 0)].tile.objects.iter().filter(|o| o.file.contains("junction_")).collect();
        assert_eq!(j.len(), 1);
        assert_eq!(crate::selection::tests::free_ends_near(doc, DVec2::new(j[0].pos[0], j[0].pos[1]), 25.0), 3);
        let _ = std::fs::remove_dir_all(content);
    }

    #[test]
    fn a_switch_on_a_bending_track_is_joined_again_gently() {
        let rail = "Splines\\Ruede\\rail_concrete_01.sli";
        let Some(mut assets) = installed_spline(rail) else { return };
        let sw = crate::switches::object_for(rail, true);
        for s in [sw.clone(), crate::switches::object_for(rail, false)] { for _ in 0..500 { if assets.object(&s).is_some() { break; } assets.poll(); std::thread::sleep(std::time::Duration::from_millis(10)); } }
        let mut doc = flat_map(1);
        add_spline(&mut doc, (0, 0), 3, 150.0, 30.0, 0.0, 200.0, 0, 0);
        { let sp = &mut doc.tiles.get_mut(&(0, 0)).unwrap().tile.splines[0]; sp.file = rail.into(); sp.radius = -300.0; }
        let mut ed = Editor::default();
        ed.open(doc);
        ed.set_tool(Tool::Roads);
        ed.road_type = Some(rail.into());
        ed.inherit_type = false;
        let lib = Library::default();
        let c = omsi_geometry::SplineCurve::from_map(ed.doc.as_ref().unwrap().spline((0,0),3).unwrap(), DVec2::ZERO);
        let p = c.point_at(80.0);
        ed.cursor = Some(p);
        ed.plan_road(&mut assets);
        ed.road_click(&mut assets, &lib);
        ed.cursor = Some(DVec3::new(p.x + 20.0, p.y + 60.0, 0.0));
        ed.plan_road(&mut assets);
        ed.road_click(&mut assets, &lib);
        let doc = ed.doc.as_ref().unwrap();
        let t = &doc.tiles[&(0, 0)].tile;
        let o = t.objects.iter().find(|o| o.file.contains("EW190_R")).expect("a switch");
        // the main track goes on from the straight heel, tangent, and bends back to its own
        // curve no tighter than a track should
        let model = assets.object(&o.file).unwrap();
        let shape = crate::switches::Shape::of(&model).unwrap();
        let (heel, hh) = shape.world(shape.straight, DVec3::new(o.pos[0], o.pos[1], 0.0), o.rot[0]);
        let first = t.splines.iter().find(|s| (DVec2::new(s.pos[0], s.pos[1]) - heel.truncate()).length() < 0.05).expect("a track from the heel");
        assert!((roads::norm_deg(first.heading - hh + 180.0) - 180.0).abs() < 0.1);
        let mut cur = first.clone();
        for _ in 0..8 {
            assert!(cur.radius == 0.0 || cur.radius.abs() >= 100.0, "{}", cur.radius);
            let Some(n) = doc.spline((0, 0), cur.next_id) else { break };
            if (n.radius + 300.0).abs() < 1e-6 {
                break;
            }
            cur = n.clone();
        }
    }
}

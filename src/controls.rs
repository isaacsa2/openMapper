//! The keyboard commands and the keys they are on. Every one has a default; the ones the
//! user changes are kept in the settings (`Prefs::keys`, command id → key as text).

use std::collections::BTreeMap;
use winit::keyboard::KeyCode;

use crate::i18n::tr;

/// A key with the modifiers held with it (Ctrl is Cmd on a Mac).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub code: KeyCode,
    pub ctrl: bool,
    pub shift: bool,
}

impl Chord {
    const fn key(code: KeyCode) -> Chord {
        Chord { code, ctrl: false, shift: false }
    }
    const fn ctrl(code: KeyCode) -> Chord {
        Chord { code, ctrl: true, shift: false }
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s += "Ctrl+";
        }
        if self.shift {
            s += "Shift+";
        }
        s + code_name(self.code)
    }

    fn parse(text: &str) -> Option<Chord> {
        let (mut ctrl, mut shift) = (false, false);
        let mut rest = text;
        loop {
            if let Some(r) = rest.strip_prefix("Ctrl+") {
                ctrl = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("Shift+") {
                shift = true;
                rest = r;
            } else {
                break;
            }
        }
        KEYS.iter().find(|(_, n)| *n == rest).map(|(code, _)| Chord { code: *code, ctrl, shift })
    }
}

macro_rules! keys {
    ($($c:ident $n:expr_2021),* $(,)?) => { &[$((KeyCode::$c, $n)),*] };
}

/// The keys that can be chosen, with the names shown and kept.
const KEYS: &[(KeyCode, &str)] = keys![
    KeyA "A", KeyB "B", KeyC "C", KeyD "D", KeyE "E", KeyF "F", KeyG "G", KeyH "H", KeyI "I", KeyJ "J", KeyK "K", KeyL "L", KeyM "M",
    KeyN "N", KeyO "O", KeyP "P", KeyQ "Q", KeyR "R", KeyS "S", KeyT "T", KeyU "U", KeyV "V", KeyW "W", KeyX "X", KeyY "Y", KeyZ "Z",
    Digit0 "0", Digit1 "1", Digit2 "2", Digit3 "3", Digit4 "4", Digit5 "5", Digit6 "6", Digit7 "7", Digit8 "8", Digit9 "9",
    F1 "F1", F2 "F2", F3 "F3", F4 "F4", F5 "F5", F6 "F6", F7 "F7", F8 "F8", F9 "F9", F10 "F10", F11 "F11", F12 "F12",
    Home "Home", End "End", PageUp "PageUp", PageDown "PageDown", Insert "Insert", Delete "Del", Backspace "Backspace",
    Enter "Enter", Space "Space", Tab "Tab", BracketLeft "[", BracketRight "]", Minus "-", Equal "=", Comma ",", Period ".",
    Slash "/", Semicolon ";", Quote "'", Backquote "`", Backslash "\\",
];

pub fn code_name(code: KeyCode) -> &'static str {
    KEYS.iter().find(|(c, _)| *c == code).map(|(_, n)| *n).unwrap_or("?")
}

/// Whether a pressed key can be chosen for a command.
pub fn bindable(code: KeyCode) -> bool {
    KEYS.iter().any(|(c, _)| *c == code)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Forward,
    Back,
    Left,
    Right,
    TurnLeft,
    TurnRight,
    Undo,
    Redo,
    Save,
    Copy,
    Cut,
    Paste,
    Duplicate,
    SelectAll,
    Find,
    Commands,
    ToolSelect,
    ToolRoads,
    ToolObjects,
    ToolLogic,
    ToolTerrain,
    ToolPaint,
    ToolTiles,
    Rotate,
    XformMove,
    XformRotate,
    XformScale,
    EditMesh,
    JoinObjects,
    Ruler,
    Knife,
    BuildRoad,
    TileGrid,
    Group,
    Ungroup,
    LookDown,
    View2d,
    Assets,
    Overview,
    Lines,
    Traffic,
    Imagery,
    BrushSmaller,
    BrushLarger,
    Delete,
    Focus,
    TestDrive,
    Picture,
}

use Cmd::*;

pub const ALL: &[Cmd] = &[
    Forward, Back, Left, Right, TurnLeft, TurnRight, LookDown, View2d, Focus, TileGrid, Overview, Assets, Lines, Traffic, Imagery, ToolSelect, ToolRoads, ToolObjects, ToolLogic,
    ToolTerrain, ToolPaint, ToolTiles, XformMove, XformRotate, XformScale, EditMesh, JoinObjects, Rotate, Ruler, Knife, BuildRoad, BrushSmaller, BrushLarger, Delete, Group, Ungroup, Copy, Cut, Paste, Duplicate, SelectAll, Undo,
    Redo, Save, Find, Commands, TestDrive, Picture,
];

impl Cmd {
    /// The name kept in the settings.
    pub fn id(self) -> &'static str {
        match self {
            Forward => "forward",
            Back => "back",
            Left => "left",
            Right => "right",
            TurnLeft => "turn_left",
            TurnRight => "turn_right",
            Undo => "undo",
            Redo => "redo",
            Save => "save",
            Copy => "copy",
            Cut => "cut",
            Paste => "paste",
            Duplicate => "duplicate",
            SelectAll => "select_all",
            Find => "find",
            Commands => "commands",
            ToolSelect => "tool_select",
            ToolRoads => "tool_roads",
            ToolObjects => "tool_objects",
            ToolLogic => "tool_logic",
            ToolTerrain => "tool_terrain",
            ToolPaint => "tool_paint",
            ToolTiles => "tool_tiles",
            Rotate => "rotate",
            XformMove => "gizmo_move",
            XformRotate => "gizmo_rotate",
            XformScale => "gizmo_scale",
            EditMesh => "edit_mesh",
            JoinObjects => "join_objects",
            Ruler => "ruler",
            Knife => "knife",
            BuildRoad => "build_road",
            TileGrid => "tile_grid",
            Group => "group",
            Ungroup => "ungroup",
            LookDown => "look_down",
            View2d => "view_2d",
            Assets => "assets",
            Overview => "overview",
            Lines => "lines",
            Traffic => "traffic",
            Imagery => "imagery",
            BrushSmaller => "brush_smaller",
            BrushLarger => "brush_larger",
            Delete => "delete",
            Focus => "focus",
            TestDrive => "test_drive",
            Picture => "picture",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Forward => tr("Move forward", "Вперёд"),
            Back => tr("Move back", "Назад"),
            Left => tr("Move left", "Влево"),
            Right => tr("Move right", "Вправо"),
            TurnLeft => tr("Turn the camera left", "Повернуть камеру влево"),
            TurnRight => tr("Turn the camera right", "Повернуть камеру вправо"),
            Undo => tr("Undo", "Отменить"),
            Redo => tr("Redo", "Повторить"),
            Save => tr("Save", "Сохранить"),
            Copy => tr("Copy", "Копировать"),
            Cut => tr("Cut", "Вырезать"),
            Paste => tr("Paste at the cursor", "Вставить у курсора"),
            Duplicate => tr("Duplicate", "Дублировать"),
            SelectAll => tr("Select all", "Выбрать всё"),
            Find => tr("Find in the map", "Поиск по карте"),
            Commands => tr("Search for a command", "Поиск команды"),
            ToolSelect => tr("Tool: Select", "Инструмент: Выбор"),
            ToolRoads => tr("Tool: Roads", "Инструмент: Дороги"),
            ToolObjects => tr("Tool: Objects", "Инструмент: Объекты"),
            ToolLogic => tr("Tool: Game logic", "Инструмент: Служебное"),
            ToolTerrain => tr("Tool: Terrain", "Инструмент: Рельеф"),
            ToolPaint => tr("Tool: Paint", "Инструмент: Покраска"),
            ToolTiles => tr("Tool: Tiles", "Инструмент: Тайлы"),
            Rotate => tr("Turn the object (Shift: the other way)", "Повернуть объект (Shift: в другую сторону)"),
            Ruler => tr("Ruler", "Линейка"),
            Knife => tr("Knife", "Нож"),
            XformMove => tr("Move handles", "Ручки перемещения"),
            XformRotate => tr("Rotate handles", "Ручки поворота"),
            XformScale => tr("Scale handles", "Ручки масштаба"),
            EditMesh => tr("Edit the mesh (vertices, edges, faces)", "Редактировать меш (вершины, рёбра, грани)"),
            JoinObjects => tr("Join the selected objects into one", "Объединить выделенные объекты в один"),
            BuildRoad => tr("Build the planned road", "Построить намеченную дорогу"),
            TileGrid => tr("Tile grid on / off", "Сетка тайлов"),
            Group => tr("Group the selection", "Сгруппировать выбранное"),
            Ungroup => tr("Ungroup", "Разгруппировать"),
            LookDown => tr("Look straight down, north up", "Смотреть сверху, север вверху"),
            View2d => tr("2D plan view on / off (3D: tilt and turn again)", "2D-вид сверху вкл / выкл (3D: снова наклон и поворот)"),
            Assets => tr("Catalogue on / off", "Каталог"),
            Overview => tr("Overview map on / off", "Обзорная карта"),
            Lines => tr("Lines window", "Окно линий"),
            Traffic => tr("Traffic window", "Окно трафика"),
            Imagery => tr("Satellite and terrain window", "Окно спутника и рельефа"),
            BrushSmaller => tr("Smaller brush", "Кисть меньше"),
            BrushLarger => tr("Larger brush", "Кисть больше"),
            Delete => tr("Delete the selection", "Удалить выбранное"),
            Focus => tr("Go to the selection", "Перейти к выбранному"),
            TestDrive => tr("Test drive in openOMSI (Shift: from the camera)", "Тест-драйв в openOMSI (Shift: от камеры)"),
            Picture => tr("Save a picture of the view", "Сохранить картинку вида"),
        }
    }

    pub fn default(self) -> Chord {
        match self {
            Forward => Chord::key(KeyCode::KeyW),
            Back => Chord::key(KeyCode::KeyS),
            Left => Chord::key(KeyCode::KeyA),
            Right => Chord::key(KeyCode::KeyD),
            TurnLeft => Chord::key(KeyCode::KeyQ),
            TurnRight => Chord::key(KeyCode::KeyE),
            Undo => Chord::ctrl(KeyCode::KeyZ),
            Redo => Chord { code: KeyCode::KeyZ, ctrl: true, shift: true },
            Save => Chord::ctrl(KeyCode::KeyS),
            Copy => Chord::ctrl(KeyCode::KeyC),
            Cut => Chord::ctrl(KeyCode::KeyX),
            Paste => Chord::ctrl(KeyCode::KeyV),
            Duplicate => Chord::ctrl(KeyCode::KeyD),
            SelectAll => Chord::ctrl(KeyCode::KeyA),
            Find => Chord::ctrl(KeyCode::KeyF),
            Commands => Chord::ctrl(KeyCode::KeyK),
            ToolSelect => Chord::key(KeyCode::Digit1),
            ToolRoads => Chord::key(KeyCode::Digit2),
            ToolObjects => Chord::key(KeyCode::Digit3),
            ToolLogic => Chord::key(KeyCode::Digit4),
            ToolTerrain => Chord::key(KeyCode::Digit5),
            ToolPaint => Chord::key(KeyCode::Digit6),
            ToolTiles => Chord::key(KeyCode::Digit7),
            Rotate => Chord::key(KeyCode::KeyR),
            XformMove => Chord::key(KeyCode::KeyV),
            XformRotate => Chord::key(KeyCode::KeyC),
            XformScale => Chord::key(KeyCode::KeyX),
            EditMesh => Chord::key(KeyCode::Tab),
            JoinObjects => Chord::ctrl(KeyCode::KeyJ),
            Ruler => Chord::key(KeyCode::KeyU),
            Knife => Chord::key(KeyCode::KeyK),
            BuildRoad => Chord::key(KeyCode::Enter),
            TileGrid => Chord::key(KeyCode::KeyG),
            Group => Chord::ctrl(KeyCode::KeyG),
            Ungroup => Chord { code: KeyCode::KeyG, ctrl: true, shift: true },
            LookDown => Chord::key(KeyCode::Home),
            View2d => Chord::key(KeyCode::Digit0),
            Assets => Chord::key(KeyCode::KeyB),
            Overview => Chord::key(KeyCode::KeyM),
            Lines => Chord::key(KeyCode::KeyL),
            Traffic => Chord::key(KeyCode::KeyT),
            Imagery => Chord::key(KeyCode::KeyI),
            BrushSmaller => Chord::key(KeyCode::BracketLeft),
            BrushLarger => Chord::key(KeyCode::BracketRight),
            Delete => Chord::key(KeyCode::Delete),
            Focus => Chord::key(KeyCode::KeyF),
            TestDrive => Chord::key(KeyCode::F5),
            Picture => Chord::key(KeyCode::F12),
        }
    }

    /// Whether Shift only changes what the command does (it is not part of the key).
    fn shift_free(self) -> bool {
        matches!(self, Rotate | TestDrive)
    }

    /// The keys that also do it and are not changed.
    pub fn also(self) -> &'static str {
        match self {
            Forward => "↑",
            Back => "↓",
            Left => "←",
            Right => "→",
            Redo => "Ctrl+Y",
            Commands => "Ctrl+Shift+P",
            BuildRoad => "Num Enter",
            Delete => "Backspace",
            _ => "",
        }
    }
}

/// The keys as chosen: the defaults with the changes from the settings.
pub struct Keymap(Vec<(Cmd, Option<Chord>)>);

impl Keymap {
    pub fn of(changed: &BTreeMap<String, String>) -> Keymap {
        Keymap(
            ALL.iter()
                .map(|c| {
                    let chord = match changed.get(c.id()) {
                        Some(t) if t.is_empty() => None,
                        Some(t) => Chord::parse(t).or(Some(c.default())),
                        None => Some(c.default()),
                    };
                    (*c, chord)
                })
                .collect(),
        )
    }

    pub fn chord(&self, cmd: Cmd) -> Option<Chord> {
        self.0.iter().find(|(c, _)| *c == cmd).and_then(|(_, k)| *k)
    }

    /// The command on a key pressed with these modifiers.
    pub fn find(&self, code: KeyCode, ctrl: bool, shift: bool) -> Option<Cmd> {
        let exact = self.0.iter().find(|(_, k)| k.is_some_and(|k| k.code == code && k.ctrl == ctrl && k.shift == shift));
        let loose = || self.0.iter().find(|(c, k)| c.shift_free() && k.is_some_and(|k| k.code == code && k.ctrl == ctrl));
        exact.or_else(loose).map(|(c, _)| *c)
    }

    /// Whether `code` is the key of a command (whatever is held with it): for the keys
    /// that are held down, as the camera's.
    pub fn is_key(&self, cmd: Cmd, code: KeyCode) -> bool {
        self.chord(cmd).is_some_and(|k| k.code == code)
    }
}

/// Puts a command on a key (None: on none) in the settings' list; whatever had the key
/// loses it.
pub fn bind(changed: &mut BTreeMap<String, String>, cmd: Cmd, chord: Option<Chord>) {
    if let Some(k) = chord {
        let map = Keymap::of(changed);
        for (other, had) in &map.0 {
            if *other != cmd && *had == Some(k) {
                changed.insert(other.id().into(), String::new());
            }
        }
    }
    let text = chord.map(|k| k.text()).unwrap_or_default();
    if chord == Some(cmd.default()) {
        changed.remove(cmd.id());
    } else {
        changed.insert(cmd.id().into(), text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_all_different_and_survive_the_settings() {
        let map = Keymap::of(&BTreeMap::new());
        for (c, k) in &map.0 {
            let k = k.unwrap();
            assert_eq!(Chord::parse(&k.text()), Some(k), "{}", c.id());
            assert_eq!(map.0.iter().filter(|(_, o)| *o == Some(k)).count(), 1, "{} shares its key", c.id());
        }
    }

    #[test]
    fn a_taken_key_is_taken_from_its_old_command() {
        let mut changed = BTreeMap::new();
        bind(&mut changed, Cmd::Ruler, Some(Chord::key(KeyCode::KeyK)));
        let map = Keymap::of(&changed);
        assert_eq!(map.find(KeyCode::KeyK, false, false), Some(Cmd::Ruler));
        assert_eq!(map.chord(Cmd::Knife), None);
        bind(&mut changed, Cmd::Ruler, Some(Cmd::Ruler.default()));
        assert_eq!(Keymap::of(&changed).find(KeyCode::KeyU, false, false), Some(Cmd::Ruler));
    }

    #[test]
    fn shift_turns_the_other_way_without_changing_the_key() {
        let map = Keymap::of(&BTreeMap::new());
        assert_eq!(map.find(KeyCode::KeyR, false, true), Some(Cmd::Rotate));
        assert_eq!(map.find(KeyCode::KeyG, true, true), Some(Cmd::Ungroup));
        assert_eq!(map.find(KeyCode::KeyG, true, false), Some(Cmd::Group));
    }
}

# Changelog

Releases are tagged `v<version>`; the downloads are on the
[Releases](https://github.com/openOMSI-org/openMapper/releases) page, each with the notes of its
version from this file.

## 0.1.2 - 2026-10-23

### Added
- **Hot reload**: new, changed and removed objects, splines and textures in the content folder (and, a little slower, in the OMSI 2 folder) show up in the catalogue and on the map by themselves, a moment after the files stop changing. Nothing to press; what openMapper writes itself does not trigger it.
- **Test drive in the original OMSI 2** (Map > Test drive in OMSI 2): the saved map and the objects, roads and textures made for it are copied into the OMSI 2 folder (asked once) and OMSI 2 is started (Windows).
- **Pictures and patterns**: Objects > Shapes and lamps > Picture / pattern: a picture or texture becomes a panel object (on the ground like a decal, or standing like a sign) that is placed, moved, turned and scaled like any object; repeats make a pattern; transparency is taken from the picture.
- **Light sources** (a lamp that glows and lights the ground round it, at night or always) and **sound sources** (a .wav that plays where the marker stands, with a reach and a volume; the marker shows in the editor only).
- **A log file**: `~/.openomsi/logs/openmapper.log` (the last run's as `openmapper.previous.log`), with the version and the graphics adapter, every warning, and the message, place and backtrace of a crash. A crash of the program shows where the log is.

### Fixed
- **Maps made or saved in openMapper did not open in OMSI 2's own editor**: global.cfg lacked the sixth line of `[backgroundimage]` OMSI 2 reads (it took the next keyword for it), and tiles lacked the empty line after `[terrain]` and `[water]`, the line for the mirror flag of a spline and the `Object Nr.` lines OMSI 2's editor writes. Tiles and global.cfg of the stock maps are now written back the way OMSI 2 wrote them, and the ground painting masks have the header of OMSI 2's own.
- Satellite imagery and terrain: a download that fails (dropped connection, a busy server) is tried again twice, then the tile again; a tile that fails no longer ends the job at three in a row by chance; an error inside the job ends it with a message instead of leaving the window waiting; the picture of a tile is built in place instead of held twice in memory (the usual end of long runs); locks poisoned by a crash elsewhere no longer cascade.
- Start on older graphics drivers (Windows 10): the editor tries every graphics backend, then OpenGL alone, then the software adapter, before giving up, and uses the most edge smoothing the adapter can really do.
- "This app can't run on your PC" on Windows 10: it is the ARM64 download on an Intel / AMD PC. Each Windows zip now has WHICH-FILE.txt saying which to use and how to tell, and the program has a manifest for Windows 10 and 11.
- A crash with a wgpu validation error ("Texture … has been destroyed"): the interface's textures are now freed after the frame that may still draw them is submitted.
- The mouse wheel over the world map (satellite choice) zooms the map only and no longer scrolls the panel beside it; the zoom steps are finer.

### Changed
- Rust 2024 edition (rust-version 1.85).

## 0.1.1 - 2026-10-23

### Added
- **Updates from GitHub**: at start openMapper looks for a newer release and offers it (Install and restart, Not now, or never offer again). Help > Updates chooses what happens: offer, install and restart by itself, or never look. The download is checked against the release's SHA256SUMS, the program's file is replaced and openMapper starts again once nothing is unsaved.
- The Objects tool shows the points where roads and junction pieces can be joined (free road ends and the helper points of pieces on the map); a junction piece lines up with another piece's point as well as with a road end.

### Fixed
- Google imagery: when Google refuses with "not available for your account and region" (accounts billed in the European Economic Area get no Google satellite tiles) the message now says so and what to do ([#7](https://github.com/openOMSI-org/openMapper/issues/7)).
- An object put on a road stays on it, turned along the road when Auto-Align is on, at the road's own height, instead of being pushed to the roadside or sunk in the ground under a raised road.

## 0.1.0 - 2026-10-23

The first release: a map editor for [openOMSI](https://openomsi.org/) that opens, edits and saves
OMSI 2 maps. Builds for Windows, macOS and Linux, each for x64 and ARM64. It needs an installed
OMSI 2; the original folder is never written to, maps and imported assets go into openOMSI's
content folder.

### Drawing and shaping
- **Roads**: two clicks make an arc that continues smoothly from a road end, joins another end with an S-curve or bend, is cut into OMSI splines and linked with its neighbours; drawing by numbers (length, radius, heading), "keep the height" for bridges and embankments, roundabouts, parallel copies (a second track, the other carriageway), tracks with overhead wire.
- **Road shape**: drag end and middle dots, heights, gradients and cant at both ends, length and radius by numbers, skew and ground cut away for tunnels, lay on the ground, reverse direction, join, the height profile coloured by gradient, and the knife (`K`) that cuts a road where you click.
- **Junctions** made automatically where roads meet or cross (asphalt plate, `[path]` lanes, footpaths round the corners), with **traffic lights** (OMSI light programs and signal heads on poles) and **zebra crossings**; junctions moved or turned carry the roads along.
- **Switches**: a track drawn off another's middle lays OMSI 2's own switch; Auto-Switch, Auto-Junction and Auto-Align switches in the status bar.
- **Road types**: a road maker (lanes each way, widths, middle strip, footways, kerb, textures) writing an ordinary OMSI spline, and per type how its junctions are made.
- **Objects**: a catalogue with the installation's folder tree and thumbnails, an assets panel with search and filters, lining up with the nearest road, objects along a road as spline attachment rows, copies in a row or a grid, hanging and linked objects, texts shown with OMSI's fonts, groups on the map and groups kept for other maps.
- **Select, Move, Rotate and Scale** tools with handles on the ground; location, rotation and scale by numbers, scaled copies of an object's type.
- **Plants**: a plant brush with a plant mix, clearing, and filling an outline (a wood, a park).
- **Terrain**: raise, lower, smooth, flatten and roughen brushes, ramps, level ground under a building, height maps exported and laid back as 16-bit PNG or TIFF.
- **Paint**: the ground's texture layers with detail textures and repeats, painting along a road, painting by slope and height.
- **Tiles**: add 300 × 300 m tiles or a ring of them, remove them, water and street-lamp light maps per tile, tiles taken from another map.
- **Own pictures** on tiles (a scan, a plan), turned, mirrored and scaled by eye.

### Real places
- **Satellite and terrain**: a world map to place the map on, imagery cut exactly to each tile from Esri, official orthophotos of several countries or your own Google, Mapbox, MapTiler or any XYZ, WMTS or WMS source, and terrain heights.
- **From OpenStreetMap**: roads, one-way streets, tram and railway tracks with switches, bus stops, bus lines with their trips, trees and woods, lakes and rivers, and buildings matched to the catalogue's models, in one undo step.
- **GPS tracks** (GPX, KML, GeoJSON) to trace or to build roads from.

### Game logic, timetable and traffic
- Start points, bus stops (in pairs on both sides), car parks, AI paths, sound sources and path rules (speed limits, density, priority, vehicle kinds).
- **Lines and timetable**: lines, trips and tours written as OMSI 2's `TTData`, routes found along the roads, departures from a first and a last time, copies of lines, holidays.
- **Traffic and AI**: road and passenger density over the day, car groups and depots, parked cars and people.
- **Depot files** (`.hof`) made from the map's lines with destinations, stops and IBIS routes, and edited in a window of their own.
- **Signal routes**, **chrono scenarios** (the map's changes over the years, with their own timetable and AI lists), **map settings**, **calendar and place**, number plates, descriptions in several languages.

### Your own models
- **Import** of mods (folders and zips), `.sco` folders and 3D models (OBJ, glTF / GLB, FBX, X, O3D) with their materials, textures and PBR maps; imported models keep their pieces as meshes.
- **Meshes**: shapes and lamps, a mesh editor (vertices, edges and faces; extrude, inset, bevel, loop cut, bridge, spin, mirror and more), booleans, a modifier stack, materials with transparency, glow, normal and ORM maps, UV maps with a UV editor, baking, lamps, street lamps and signal heads, and export to OBJ, glTF, GLB, FBX, Collada, USD, STL, PLY, 3MF, OFF, `.x` and `.o3d`.
- **PBR maker**: normal, roughness, metal and occlusion maps made from a texture, written as openOMSI reads them.

### Checking, testing and sharing
- **Test drive** in openOMSI with one key (`F5`), from the nearest start point or with a free camera.
- **Check in openOMSI** runs the game's own checks unseen and shows what they found; **Map check** names missing files, roads not joined, stops without a lane, ID clashes and more, many fixed with one click; `openmapper --check` does it without a window.
- **Safe saving**: every file written under a temporary name first, the last ten saves kept, autosave every three minutes, earlier versions brought back from the File menu.
- **Export for sharing** as a zip laid out like OMSI 2, **a plan of the map** as an SVG, and import of both.

### Workspace
- A viewport like Unity's scene view with fly, orbit and trackpad controls, a map-editor camera, an overview map, saved views, layers, names in the view, time of day with sun and shadows, seasons, graphics quality and PBR.
- **Controls** window: every key can be changed; a search for any command (`Ctrl+K`); find in map; the right-click menu; an outliner of everything on the map; undo and redo for every action with a history list.
- **New map** from OMSI 2's template, a getting-started checklist, a written tutorial, and the interface in English and Russian at 80–150 % size.

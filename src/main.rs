//! openMapper: a simple map editor for openOMSI.
//!
//! Needs an installed OMSI 2. Maps are read with openOMSI's own readers and saved in OMSI's
//! format into openOMSI's content folder; the original installation is never written to.

// (no console window behind the editor on Windows; one it is started from is used)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assets;
mod autopaint;
mod camera;
mod check;
mod controls;
mod chrono_edit;
mod chrono_tt;
mod clipboard;
mod collab;
mod descriptions;
mod doc;
mod draw;
mod export;
mod gamecheck;
mod icon;
mod geo;
mod gizmo;
mod editable;
mod editor;
mod gpu;
mod heightmap;
mod history;
mod hof;
mod i18n;
mod icons;
mod import;
mod import_image;
mod install;
mod junction;
mod lanes;
mod lanes_edit;
mod mapgroups;
mod merge;
mod meshedit;
mod meshops;
mod library;
mod models;
mod pbr_maker;
mod onground;
mod osm;
mod osm_map;
mod osm_buildings;
mod osm_green;
mod osm_lines;
mod osm_water;
mod parallel;
mod place;
mod plan;
mod plants;
mod render;
mod reshape;
mod road_maker;
mod roadpaint;
mod roads;
mod roundabout;
mod rows;
mod scaled;
mod selection;
mod signals;
mod switches;
mod texts;
mod timetable;
mod traffic;
mod ui;
mod updater;
mod ui_assets;
mod ui_edit;
mod ui_find;
mod ui_groups;
mod ui_hof;
mod ui_geo;
mod sample;
mod signal_maker;
mod ui_guide;
mod ui_calendar;
mod ui_collab;
mod ui_chrono;
mod ui_mapsettings;
mod ui_signals;
mod ui_menu;
mod ui_mesh;
mod ui_minimap;
mod ui_merge;
mod ui_osm;
mod ui_outliner;
mod ui_palette;
mod ui_profile;
mod ui_uv;
mod ui_views;
mod ui_xform;
mod ui_pbr;
mod ui_roads;
mod ui_newmap;
mod trace;
mod ui_timetable;
mod ui_trace;
mod ui_traffic;

/// A program without a console of its own (Windows' GUI subsystem) writes its log and the
/// command line modes' output into the console it was started from, if any.
#[cfg(windows)]
fn attach_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process: u32) -> i32;
    }
    // (ATTACH_PARENT_PROCESS)
    unsafe {
        AttachConsole(u32::MAX);
    }
}

fn main() {
    #[cfg(windows)]
    attach_console();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn")).init();
    // `openmapper --check path/to/global.cfg` checks a map and prints what it found
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).map(|a| a == "--check").unwrap_or(false) {
        let Some(cfg) = args.get(2) else {
            eprintln!("openmapper --check path/to/global.cfg");
            std::process::exit(2);
        };
        std::process::exit(check::cli(std::path::Path::new(cfg)));
    }
    // `openmapper --export path/to/global.cfg out.zip` / `--import file` (no window)
    match (args.get(1).and_then(|a| a.to_str()), args.get(2), args.get(3)) {
        (Some("--export"), Some(cfg), Some(out)) => std::process::exit(export::cli_export(std::path::Path::new(cfg), std::path::Path::new(out))),
        (Some("--import"), Some(f), _) => std::process::exit(export::cli_import(std::path::Path::new(f))),
        // `openmapper --plan path/to/global.cfg out.svg`: a plan of the map
        (Some("--plan"), Some(cfg), Some(out)) => std::process::exit(plan::cli(std::path::Path::new(cfg), std::path::Path::new(out))),
        // `openmapper --icon out.png [size]`: the program's icon (for the packages)
        (Some("--icon"), Some(out), size) => std::process::exit(icon::cli(std::path::Path::new(out), size.and_then(|s| s.to_str()?.parse().ok()).unwrap_or(512))),
        _ => {}
    }
    // `openmapper path/to/global.cfg` opens that map
    updater::clean_up();
    let open = std::env::args_os().nth(1).map(std::path::PathBuf::from).filter(|p| p.is_file());
    let event_loop = match winit::event_loop::EventLoop::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("openMapper: {e}");
            std::process::exit(1);
        }
    };
    let mut app = app::App::new(open);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("openMapper: {e}");
    }
}

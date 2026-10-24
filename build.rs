// Windows: the program's icon in the .exe (the one the file manager and the taskbar show)
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let _ = embed_resource::compile("assets/windows/openmapper.rc", embed_resource::NONE);
    }
    println!("cargo:rerun-if-changed=assets/windows/openmapper.rc");
    println!("cargo:rerun-if-changed=assets/windows/openmapper.manifest");
    println!("cargo:rerun-if-changed=assets/icons/app/openmapper.ico");
}

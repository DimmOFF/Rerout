// Embeds rerout.rc (icon + version info) into rerout.exe.
// Needs rc.exe from the Windows SDK, which comes with the MSVC Build Tools.
// If it cannot be found the build still succeeds, just without the icon.
fn main() {
    println!("cargo:rerun-if-changed=rerout.rc");
    println!("cargo:rerun-if-changed=rerout.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let _ = embed_resource::compile("rerout.rc", embed_resource::NONE);
    }
}

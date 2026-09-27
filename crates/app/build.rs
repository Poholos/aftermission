//! Embeds the icon into the Windows executable, so Explorer, the taskbar
//! and the Start menu show it. The running window gets its icon from
//! `main.rs` instead; both come from the drawing in `examples/icon.rs`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon/aftermission.ico");
    embed_icon();
}

/// Only a Windows host has the resource compiler, and only a Windows
/// target takes the resource. Without the `.ico` (before the example has
/// painted it) the build goes on and says so.
#[cfg(windows)]
fn embed_icon() {
    const ICO: &str = "../../assets/icon/aftermission.ico";

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if !std::path::Path::new(ICO).exists() {
        println!("cargo:warning=no {ICO}: the executable gets no icon");
        return;
    }
    winresource::WindowsResource::new()
        .set_icon(ICO)
        .compile()
        .expect("the resource compiler of the Windows SDK embeds the icon");
}

#[cfg(not(windows))]
fn embed_icon() {}

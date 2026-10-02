// Windows identity (2026-10-02): embed the UnoOne brand icon into the
// starter executable's resource section so "Start UnoOne.exe" shows the
// brand mark in Explorer (and in any shortcut pinned to it) instead of the
// generic white fallback. The starter owns no window — the taskbar icon is
// UnoOnePower.exe's, which Tauri embeds from src-tauri/icons/icon.ico.
fn main() {
    // Only link resources when building FOR Windows (CI cross-checks cfg).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("icon.ico");
        if let Err(error) = resource.compile() {
            panic!("failed to embed the UnoOne icon resource: {error}");
        }
    }
    println!("cargo:rerun-if-changed=icon.ico");
}

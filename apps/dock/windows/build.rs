// Windows identity (2026-10-02): embed the UnoOne brand icon into the dock
// executable so "UnoOneDock.exe" carries the brand mark in Explorer and in
// the Start Menu / Startup-folder shortcuts the dock's --install creates.
// The tray icon itself is loaded at runtime; this fixes the exe/shortcut face.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("icon.ico");
        if let Err(error) = resource.compile() {
            panic!("failed to embed the UnoOne icon resource: {error}");
        }
    }
    println!("cargo:rerun-if-changed=icon.ico");
}
//! Developer CLI for the package validator: run either validation scope
//! against a package root and print the JSON report.
//!
//! Usage:
//!   cargo run -p unoone-usb-manifest --example validate_package -- <root> [identity|launch]
//!
//! `identity` is the fast PackageIdentity scope (Starter/Dock); `launch` is
//! the full DesktopLaunch sweep (the desktop app's background validation),
//! which also hashes required speech models, configs, and attestations.

use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: validate_package <root> [identity|launch]");
        std::process::exit(2);
    };
    let scope = match args.next().as_deref() {
        None | Some("launch") => unoone_usb_manifest::ValidationScope::DesktopLaunch,
        Some("identity") => unoone_usb_manifest::ValidationScope::PackageIdentity,
        Some(other) => {
            eprintln!("unknown scope '{other}': expected 'identity' or 'launch'");
            std::process::exit(2);
        }
    };
    let report = unoone_usb_manifest::validate_package(Path::new(&root), scope);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("report serializes")
    );
    if !report.valid {
        std::process::exit(1);
    }
}

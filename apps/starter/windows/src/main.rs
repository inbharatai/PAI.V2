#![cfg_attr(windows, windows_subsystem = "windows")]

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use unoone_usb_manifest::{validate_package, AssetKind, ValidationScope};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = run(&args);
    if let Err(error) = result {
        show_message("Start UnoOne", &error, MessageKind::Error);
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let root = own_usb_root()?;
    let report = validate_package(&root, ValidationScope::DesktopLaunch);
    if args.iter().any(|arg| arg == "--verify-only") {
        // Do not serialize `report.package`: it embeds the complete manifest and
        // can contain hundreds of runtime/voice assets. The verification CLI
        // needs the decision and failures, not a second copy of the package.
        let summary = serde_json::json!({
            "valid": report.valid,
            "failure_count": report.failures.len(),
            "failures": &report.failures,
        });
        let json = serde_json::to_string_pretty(&summary)
            .map_err(|error| format!("Failed to serialize validation report: {error}"))?;
        println!("{json}");
        return if report.valid {
            Ok(())
        } else {
            Err("Pocket AI validation failed".to_string())
        };
    }

    let package = report.package.ok_or_else(|| {
        let details = report
            .failures
            .iter()
            .map(|failure| {
                format!(
                    "{:?}: {}{}",
                    failure.code,
                    failure.message,
                    failure
                        .path
                        .as_deref()
                        .map(|path| format!(" ({path})"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("This Pocket AI package is missing or has been modified.\n\n{details}")
    })?;

    Command::new(&package.desktop_executable)
        .arg("--vault-root")
        .arg(&package.root)
        .arg("--launched-by-starter")
        .spawn()
        .map_err(|error| format!("UnoOne Power could not be launched: {error}"))?;

    if args.iter().any(|arg| arg == "--no-install-offer") {
        return Ok(());
    }

    if ask_yes_no(
        "UnoOne launched",
        "Enable automatic launch on this computer?\n\nUnoOne Dock will be installed for the current Windows user. Administrator rights are not required.",
    ) {
        install_dock(&package.root, &package.manifest)?;
    }
    Ok(())
}

fn own_usb_root() -> Result<PathBuf, String> {
    if let Some(value) = env::args_os().skip(1).find_map(|value| {
        value
            .to_string_lossy()
            .strip_prefix("--vault-root=")
            .map(PathBuf::from)
    }) {
        return Ok(value);
    }
    let executable =
        env::current_exe().map_err(|error| format!("Cannot locate Start UnoOne: {error}"))?;
    executable
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Cannot determine the Pocket AI root".to_string())
}

fn install_dock(root: &Path, manifest: &unoone_usb_manifest::PocketManifest) -> Result<(), String> {
    let dock = manifest
        .platforms
        .windows
        .dock
        .as_ref()
        .filter(|asset| asset.kind == AssetKind::DockExecutable)
        .ok_or_else(|| "This package does not declare UnoOne Dock".to_string())?;
    let path = root.join(dock.path.replace('/', "\\"));
    Command::new(path)
        .arg("--install")
        .status()
        .map_err(|error| format!("UnoOne Dock installer could not start: {error}"))
        .and_then(|status| {
            if status.success() {
                show_message(
                    "UnoOne Dock",
                    "Automatic launch is enabled for this Windows user.",
                    MessageKind::Info,
                );
                Ok(())
            } else {
                Err(format!("UnoOne Dock installation failed with {status}"))
            }
        })
}

#[derive(Clone, Copy)]
enum MessageKind {
    Info,
    Error,
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn show_message(title: &str, body: &str, kind: MessageKind) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
    };
    let title = wide(title);
    let body = wide(body);
    let icon = match kind {
        MessageKind::Info => MB_ICONINFORMATION,
        MessageKind::Error => MB_ICONERROR,
    };
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | icon,
        );
    }
}

#[cfg(not(windows))]
fn show_message(title: &str, body: &str, _kind: MessageKind) {
    eprintln!("{title}: {body}");
}

#[cfg(windows)]
fn ask_yes_no(title: &str, body: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONQUESTION, MB_YESNO,
    };
    let title = wide(title);
    let body = wide(body);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}

#[cfg(not(windows))]
fn ask_yes_no(_title: &str, _body: &str) -> bool {
    false
}

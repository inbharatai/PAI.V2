#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("UnoOne Dock is available only on Windows.");
}

#[cfg(windows)]
mod windows_app {
    use std::collections::{HashMap, HashSet};
    use std::env;
    use std::ffi::OsStr;
    use std::fs;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::ptr::{null, null_mut};
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::Duration;
    use unoone_usb_manifest::{validate_package, ValidationScope};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
    };
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOVABLE;
    use windows_sys::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO,
        NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, LoadIconW, PostQuitMessage,
        RegisterClassExW, TranslateMessage, CS_HREDRAW, CS_VREDRAW, DBT_DEVICEARRIVAL,
        DBT_DEVICEREMOVECOMPLETE, IDI_APPLICATION, MSG, WM_DESTROY, WM_DEVICECHANGE,
        WM_LBUTTONDBLCLK, WM_RBUTTONUP, WNDCLASSEXW, WS_OVERLAPPED,
    };
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    const RUN_VALUE: &str = "UnoOneDock";
    const APP_DIR: &str = "UnoOne\\Dock";
    const TRAY_ID: u32 = 1;
    const WM_TRAYICON: u32 = 0x8000 + 42;

    static STATE: OnceLock<Mutex<DockState>> = OnceLock::new();

    #[derive(Default)]
    struct DockState {
        hwnd: HWND,
        connected: HashMap<PathBuf, ConnectedPackage>,
        invalid_notified: HashSet<PathBuf>,
    }

    // SAFETY: HWND is only used as an opaque value passed back to Win32.
    unsafe impl Send for DockState {}

    #[derive(Clone)]
    struct ConnectedPackage {
        vault_id: String,
        desktop_executable: PathBuf,
    }

    pub fn main() {
        let args: Vec<String> = env::args().skip(1).collect();
        let result = if args.iter().any(|arg| arg == "--install") {
            install()
        } else if args.iter().any(|arg| arg == "--uninstall") {
            uninstall()
        } else if args.iter().any(|arg| arg == "--run-once") {
            run_once()
        } else {
            run_background()
        };
        if let Err(error) = result {
            native_message("UnoOne Dock", &error, true);
            std::process::exit(1);
        }
    }

    fn install() -> Result<(), String> {
        let source =
            env::current_exe().map_err(|error| format!("Cannot locate UnoOne Dock: {error}"))?;
        let local = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| "LOCALAPPDATA is unavailable".to_string())?
            .join(APP_DIR);
        fs::create_dir_all(&local)
            .map_err(|error| format!("Cannot create {}: {error}", local.display()))?;
        let destination = local.join("UnoOneDock.exe");
        if source != destination {
            fs::copy(&source, &destination).map_err(|error| {
                format!(
                    "Cannot install {} to {}: {error}",
                    source.display(),
                    destination.display()
                )
            })?;
        }

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run, _) = hkcu
            .create_subkey_with_flags(
                "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                KEY_READ | KEY_WRITE,
            )
            .map_err(|error| format!("Cannot open the current-user Run key: {error}"))?;
        run.set_value(
            RUN_VALUE,
            &format!("\"{}\" --background", destination.display()),
        )
        .map_err(|error| format!("Cannot enable current-user startup: {error}"))?;

        Command::new(&destination)
            .arg("--background")
            .spawn()
            .map_err(|error| format!("Dock was installed but could not be started: {error}"))?;
        Ok(())
    }

    fn uninstall() -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(run) = hkcu.open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            KEY_READ | KEY_WRITE,
        ) {
            let _ = run.delete_value(RUN_VALUE);
        }
        let installed = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| "LOCALAPPDATA is unavailable".to_string())?
            .join(APP_DIR)
            .join("UnoOneDock.exe");
        let current = env::current_exe().unwrap_or_default();
        if installed.exists() && installed != current {
            fs::remove_file(&installed).map_err(|error| {
                format!(
                    "Startup was disabled, but the installed file could not be removed: {error}"
                )
            })?;
        }
        Ok(())
    }

    fn run_once() -> Result<(), String> {
        let mut state = DockState::default();
        scan_removable_drives(&mut state, true)?;
        if state.connected.is_empty() {
            return Err("No valid UnoOne Pocket AI is connected.".to_string());
        }
        Ok(())
    }

    fn run_background() -> Result<(), String> {
        let mutex_name = wide("Local\\UnoOneDock.CurrentUser.v1");
        let mutex = unsafe { CreateMutexW(null(), 1, mutex_name.as_ptr()) };
        if mutex.is_null() {
            return Err("Cannot create the UnoOne Dock single-instance mutex.".to_string());
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(mutex) };
            return Ok(());
        }

        unsafe {
            let instance = GetModuleHandleW(null());
            let class_name = wide("UnoOneDockMessageWindow");
            let title = wide("UnoOne Dock");
            let window_class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance as HINSTANCE,
                hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
                hCursor: null_mut(),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: class_name.as_ptr(),
                hIconSm: LoadIconW(null_mut(), IDI_APPLICATION),
            };
            if RegisterClassExW(&window_class) == 0 {
                CloseHandle(mutex);
                return Err("Cannot register the UnoOne Dock notification window.".to_string());
            }
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance as HINSTANCE,
                null(),
            );
            if hwnd.is_null() {
                CloseHandle(mutex);
                return Err("Cannot create the UnoOne Dock notification window.".to_string());
            }
            STATE
                .set(Mutex::new(DockState {
                    hwnd,
                    ..DockState::default()
                }))
                .map_err(|_| "Dock state was already initialized".to_string())?;
            add_tray_icon(hwnd);
            if let Some(state) = STATE.get() {
                let mut state = state.lock().map_err(|_| "Dock state lock failed")?;
                let _ = scan_removable_drives(&mut state, true);
            }

            let mut message: MSG = zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            remove_tray_icon(hwnd);
            CloseHandle(mutex);
        }
        Ok(())
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_DEVICECHANGE
                if wparam as u32 == DBT_DEVICEARRIVAL
                    || wparam as u32 == DBT_DEVICEREMOVECOMPLETE =>
            {
                thread::spawn(|| {
                    thread::sleep(Duration::from_millis(700));
                    if let Some(state) = STATE.get() {
                        if let Ok(mut state) = state.lock() {
                            let _ = scan_removable_drives(&mut state, true);
                        }
                    }
                });
                1
            }
            WM_TRAYICON if lparam as u32 == WM_LBUTTONDBLCLK => {
                thread::spawn(|| {
                    if let Some(state) = STATE.get() {
                        if let Ok(mut state) = state.lock() {
                            let _ = scan_removable_drives(&mut state, true);
                        }
                    }
                });
                0
            }
            WM_TRAYICON if lparam as u32 == WM_RBUTTONUP => {
                native_message(
                    "UnoOne Dock",
                    "UnoOne Dock is watching for a validated Pocket AI.\n\nDouble-click the tray icon to rescan.",
                    false,
                );
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }

    fn scan_removable_drives(state: &mut DockState, launch: bool) -> Result<(), String> {
        let present = removable_pocket_roots();
        let removed: Vec<PathBuf> = state
            .connected
            .keys()
            .filter(|root| !present.contains(*root))
            .cloned()
            .collect();
        for root in removed {
            if let Some(package) = state.connected.remove(&root) {
                notify(
                    state.hwnd,
                    "Pocket AI disconnected",
                    &format!(
                        "{} was removed. UnoOne Power will stop inference, discard active capture buffers, and lock the vault.",
                        package.vault_id
                    ),
                    NotifyKind::Warning,
                );
            }
            state.invalid_notified.remove(&root);
        }

        for root in present {
            let report = validate_package(&root, ValidationScope::DesktopLaunch);
            if let Some(package) = report.package {
                state.invalid_notified.remove(&root);
                if state.connected.contains_key(&root) {
                    continue;
                }
                let connected = ConnectedPackage {
                    vault_id: package.vault_id.clone(),
                    desktop_executable: package.desktop_executable.clone(),
                };
                state.connected.insert(root.clone(), connected.clone());
                notify(
                    state.hwnd,
                    "Valid Pocket AI connected",
                    &format!("Validated {} at {}", connected.vault_id, root.display()),
                    NotifyKind::Info,
                );
                if launch {
                    if let Err(error) = launch_power(&root, &connected.desktop_executable) {
                        notify(
                            state.hwnd,
                            "UnoOne Power launch failed",
                            &error,
                            NotifyKind::Error,
                        );
                    }
                }
            } else if state.invalid_notified.insert(root.clone()) {
                let summary = report
                    .failures
                    .iter()
                    .take(3)
                    .map(|failure| format!("{:?}: {}", failure.code, failure.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                let title = if report
                    .failures
                    .iter()
                    .any(|failure| format!("{:?}", failure.code).contains("MISSING"))
                {
                    "Pocket AI is incomplete"
                } else {
                    "Pocket AI rejected"
                };
                notify(state.hwnd, title, &summary, NotifyKind::Error);
            }
        }
        Ok(())
    }

    fn removable_pocket_roots() -> HashSet<PathBuf> {
        let mut roots = HashSet::new();
        let mask = unsafe { GetLogicalDrives() };
        for index in 0..26 {
            if mask & (1 << index) == 0 {
                continue;
            }
            let letter = (b'A' + index as u8) as char;
            let drive = format!("{letter}:\\");
            let drive_wide = wide(&drive);
            if unsafe { GetDriveTypeW(drive_wide.as_ptr()) } != DRIVE_REMOVABLE {
                continue;
            }
            let drive_root = PathBuf::from(&drive);
            let nested = drive_root.join("UNOONE");
            if nested.join("manifest.json").is_file() {
                roots.insert(nested);
            } else if drive_root.join("manifest.json").is_file() {
                roots.insert(drive_root);
            }
        }
        roots
    }

    fn launch_power(root: &Path, executable: &Path) -> Result<(), String> {
        Command::new(executable)
            .arg("--vault-root")
            .arg(root)
            .arg("--launched-by-dock")
            .spawn()
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "Validated {} but could not launch {}: {error}",
                    root.display(),
                    executable.display()
                )
            })
    }

    unsafe fn add_tray_icon(hwnd: HWND) {
        let mut data = tray_data(hwnd);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_TRAYICON;
        data.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
        copy_wide(&mut data.szTip, "UnoOne Dock");
        Shell_NotifyIconW(NIM_ADD, &data);
    }

    unsafe fn remove_tray_icon(hwnd: HWND) {
        let data = tray_data(hwnd);
        Shell_NotifyIconW(NIM_DELETE, &data);
    }

    #[derive(Clone, Copy)]
    enum NotifyKind {
        Info,
        Warning,
        Error,
    }

    fn notify(hwnd: HWND, title: &str, body: &str, kind: NotifyKind) {
        if hwnd.is_null() {
            println!("{title}: {body}");
            return;
        }
        unsafe {
            let mut data = tray_data(hwnd);
            data.uFlags = NIF_INFO;
            copy_wide(&mut data.szInfoTitle, title);
            copy_wide(&mut data.szInfo, body);
            data.dwInfoFlags = match kind {
                NotifyKind::Info => NIIF_INFO,
                NotifyKind::Warning => NIIF_WARNING,
                NotifyKind::Error => NIIF_ERROR,
            };
            Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = TRAY_ID;
        data
    }

    fn copy_wide<const N: usize>(target: &mut [u16; N], value: &str) {
        for (slot, value) in target
            .iter_mut()
            .zip(OsStr::new(value).encode_wide().chain(std::iter::once(0)))
        {
            *slot = value;
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn native_message(title: &str, body: &str, error: bool) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
        };
        let title = wide(title);
        let body = wide(body);
        unsafe {
            MessageBoxW(
                null_mut(),
                body.as_ptr(),
                title.as_ptr(),
                MB_OK
                    | if error {
                        MB_ICONERROR
                    } else {
                        MB_ICONINFORMATION
                    },
            );
        }
    }
}

#[cfg(windows)]
fn main() {
    windows_app::main();
}

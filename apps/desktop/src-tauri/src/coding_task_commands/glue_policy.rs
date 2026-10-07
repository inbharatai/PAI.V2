//! Pure, std-only helpers for the coding-task Tauri glue (design §8.2).
//!
//! This file has NO tauri and NO adapter imports. That lets it compile and run
//! its tests standalone (`rustc --edition 2021 --test glue_policy.rs`) on hosts
//! that cannot build the desktop crate, and also as a normal child module of
//! `coding_task_commands`. Its tests also scan the glue, `main.rs` and
//! `harness_bridge.rs` source to pin the frozen command surface and wiring.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The only webview label whose UI events may change coding-task state.
pub const MAIN_WINDOW_LABEL: &str = "main";

pub fn is_main_window(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL
}

const NOT_MAIN_WINDOW: &str = "coding task actions are accepted only from the main UnoOne window";

pub fn require_main_window(label: &str) -> Result<(), String> {
    if is_main_window(label) {
        Ok(())
    } else {
        Err(NOT_MAIN_WINDOW.to_owned())
    }
}

/// `<temp>/unoone-coding` (design §8.2). Path only: creating it (0700,
/// owner-checked, Linux only, at first use) is the production ports' job.
pub fn scratch_dir(temp: &Path) -> PathBuf {
    temp.join("unoone-coding")
}

/// `<LOCALAPPDATA or HOME>/UnoOne/coding-tasks`. This is the same host-local
/// base `harness_bridge` uses for the folder-grant store, and design §4.2's
/// `…/UnoOne/coding-tasks/`. Empty or relative values count as unset. `None`
/// means no usable base; the worktree policy then denies apply (fail closed).
pub fn worktree_base(local_app_data: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    [local_app_data, home]
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|base| base.is_absolute())
        .map(|base| base.join("UnoOne").join("coding-tasks"))
}

/// The `deny_within` entries known at startup: the install directory (the
/// parent of the running executable, design §4.2). The vault root is known
/// only after unlock, so the production ports add it at use time.
pub fn static_deny_within(current_exe: Option<PathBuf>) -> Vec<PathBuf> {
    current_exe
        .as_deref()
        .and_then(Path::parent)
        .filter(|dir| dir.is_absolute())
        .map(|dir| vec![dir.to_path_buf()])
        .unwrap_or_default()
}

/// The granted roots as `harness_bridge::get_agent_workspace_info` reports
/// them: the effective workspace root plus every additional granted folder.
/// Blank entries are dropped and entries are trimmed (the grant store's rule).
pub fn granted_roots<'a>(
    effective_root: &'a str,
    folders: impl IntoIterator<Item = &'a str>,
) -> Vec<PathBuf> {
    std::iter::once(effective_root)
        .chain(folders)
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .collect()
}

const ROOT_NOT_A_FOLDER: &str = "coding task root must be the absolute path of an existing folder";
const ROOT_NOT_GRANTED: &str = "coding task root is not inside a folder you granted in Settings; grant the folder first (no grant is created here)";

/// HA31 root gate. `requested` must be an absolute, existing directory that
/// equals or sits inside one of `granted`.
///
/// - Both sides are canonicalised, so `..` and symlinks are resolved first.
/// - The comparison is component-wise (`Path::starts_with`), so `…/proj-evil`
///   never matches `…/proj`.
/// - Granted entries that are relative, missing, or a filesystem root are
///   ignored.
///
/// Returns the canonical root, so the service captures exactly what was
/// checked. Never creates a grant. Errors carry no path text.
pub fn admit_granted_root(requested: &Path, granted: &[PathBuf]) -> Result<PathBuf, String> {
    if requested.as_os_str().is_empty() || !requested.is_absolute() {
        return Err(ROOT_NOT_A_FOLDER.to_owned());
    }
    let canonical = std::fs::canonicalize(requested).map_err(|_| ROOT_NOT_A_FOLDER.to_owned())?;
    if !canonical.is_dir() {
        return Err(ROOT_NOT_A_FOLDER.to_owned());
    }
    let inside = granted
        .iter()
        .filter(|root| root.is_absolute())
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .filter(|root| root.parent().is_some())
        .any(|root| canonical.starts_with(&root));
    if inside {
        Ok(canonical)
    } else {
        Err(ROOT_NOT_GRANTED.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::LazyLock;

    // Sources are scanned as LF text: a Windows checkout (git core.autocrlf)
    // yields CRLF files, and these scans match multi-line patterns.
    fn lf(src: &str) -> String {
        src.replace("\r\n", "\n")
    }
    static GLUE: LazyLock<String> =
        LazyLock::new(|| lf(include_str!("../coding_task_commands.rs")));
    static MAIN: LazyLock<String> = LazyLock::new(|| lf(include_str!("../main.rs")));
    static HARNESS_BRIDGE: LazyLock<String> =
        LazyLock::new(|| lf(include_str!("../harness_bridge.rs")));
    static SELF_SRC: LazyLock<String> = LazyLock::new(|| lf(include_str!("glue_policy.rs")));

    /// Design §8.2 frozen command surface: (command, main-window only, service method).
    const SURFACE: [(&str, bool, &str); 19] = [
        ("coding_task_capability", false, "capability"),
        ("coding_task_list", false, "list_tasks"),
        ("coding_task_open", true, "open_task"),
        ("coding_task_view", false, "task_view"),
        ("coding_task_file_diff", false, "file_diff"),
        ("coding_task_confirm_plan", true, "confirm_plan"),
        ("coding_task_run_gate", true, "run_gate"),
        ("coding_task_review_file", true, "review_file"),
        ("coding_task_revert_file", true, "revert_file"),
        ("coding_task_apply", true, "apply"),
        ("coding_task_revert_applied", true, "revert_applied"),
        ("coding_task_start_preview", true, "start_preview"),
        ("coding_task_stop_preview", true, "stop_preview"),
        ("coding_task_preview_logs", false, "preview_logs"),
        ("coding_task_http_checks", true, "run_http_checks"),
        (
            "coding_task_resolve_interrupted",
            true,
            "resolve_interrupted",
        ),
        ("coding_task_resume", true, "resume"),
        ("coding_task_cancel", true, "cancel"),
        ("coding_task_export_patch", true, "export_patch"),
    ];

    struct Command {
        name: String,
        signature: String,
        body: String,
    }

    /// Every command fn in `src`: the text between the attribute and `{` is
    /// the signature; the body runs to the next column-0 `}`.
    fn commands(src: &str) -> Vec<Command> {
        const ATTR: &str = "#[tauri::command]";
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(at) = rest.find(ATTR) {
            rest = &rest[at + ATTR.len()..];
            let open = rest.find('{').expect("command body");
            let signature = rest[..open].to_owned();
            let after_fn = signature.find("fn ").expect("fn keyword") + 3;
            let name = signature[after_fn..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let close = rest[open..].find("\n}\n").expect("column-0 closing brace") + open;
            out.push(Command {
                name,
                signature,
                body: rest[open + 1..close].to_owned(),
            });
            rest = &rest[close..];
        }
        out
    }

    fn unique_temp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "unoone-glue-policy-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_the_main_label_passes() {
        assert!(require_main_window("main").is_ok());
        assert!(is_main_window(MAIN_WINDOW_LABEL));
        for label in [
            "",
            "Main",
            "MAIN",
            "main ",
            " main",
            "main\0",
            "task-preview",
            "agent-preview",
            "browser-workspace",
            "main-2",
        ] {
            assert!(!is_main_window(label), "{label:?}");
            // The error is a constant: it never echoes the caller's label.
            assert_eq!(require_main_window(label).unwrap_err(), NOT_MAIN_WINDOW);
        }
    }

    #[test]
    fn service_paths_follow_the_design() {
        let temp = std::env::temp_dir();
        assert_eq!(scratch_dir(&temp), temp.join("unoone-coding"));

        let abs_a = temp.join("local-app-data");
        let abs_b = temp.join("home");
        let expect = |base: &Path| base.join("UnoOne").join("coding-tasks");
        assert_eq!(
            worktree_base(Some(abs_a.clone().into()), Some(abs_b.clone().into())),
            Some(expect(&abs_a))
        );
        assert_eq!(
            worktree_base(None, Some(abs_b.clone().into())),
            Some(expect(&abs_b))
        );
        // Empty or relative values are unset, never resolved against the CWD.
        assert_eq!(
            worktree_base(Some(OsString::new()), Some(abs_b.clone().into())),
            Some(expect(&abs_b))
        );
        assert_eq!(
            worktree_base(Some("relative/dir".into()), Some("also-relative".into())),
            None
        );
        assert_eq!(worktree_base(None, None), None);

        let exe = temp.join("install").join("unoone-power.exe");
        assert_eq!(static_deny_within(Some(exe)), vec![temp.join("install")]);
        assert!(static_deny_within(None).is_empty());
        assert!(static_deny_within(Some(PathBuf::from("unoone-power.exe"))).is_empty());
    }

    #[test]
    fn granted_roots_trims_and_drops_blanks() {
        let roots = granted_roots(" /w ", ["", "  ", "/a", " /b"]);
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/w"),
                PathBuf::from("/a"),
                PathBuf::from("/b")
            ]
        );
        assert!(granted_roots("", std::iter::empty()).is_empty());
    }

    #[test]
    fn root_gate_admits_only_inside_a_granted_folder() {
        let base = unique_temp("root-gate");
        let granted = base.join("granted");
        let project = granted.join("proj");
        let sibling = base.join("granted-evil");
        let outside = base.join("outside");
        for dir in [&project, &sibling, &outside] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(project.join("file.txt"), b"SECRET-MARKER").unwrap();
        let grants = vec![granted.clone()];
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap();

        // Inside and equal are admitted and come back canonical.
        assert_eq!(
            admit_granted_root(&project, &grants).unwrap(),
            canon(&project)
        );
        assert_eq!(
            admit_granted_root(&granted, &grants).unwrap(),
            canon(&granted)
        );
        // A non-canonical spelling that resolves inside is admitted as canonical.
        let dotted = granted.join("proj").join("..").join("proj");
        assert_eq!(
            admit_granted_root(&dotted, &grants).unwrap(),
            canon(&project)
        );

        let denied = |p: &Path, g: &[PathBuf]| {
            let error = admit_granted_root(p, g).unwrap_err();
            assert!(!error.contains("SECRET-MARKER"));
            assert!(
                !error.contains(&*base.to_string_lossy()),
                "error leaks a path: {error}"
            );
            error
        };
        // Sibling prefix, `..` escape, outside, missing, file, relative, empty.
        assert_eq!(denied(&sibling, &grants), ROOT_NOT_GRANTED);
        assert_eq!(
            denied(&granted.join("..").join("outside"), &grants),
            ROOT_NOT_GRANTED
        );
        assert_eq!(denied(&outside, &grants), ROOT_NOT_GRANTED);
        assert_eq!(denied(&granted.join("missing"), &grants), ROOT_NOT_A_FOLDER);
        assert_eq!(
            denied(&project.join("file.txt"), &grants),
            ROOT_NOT_A_FOLDER
        );
        assert_eq!(
            denied(Path::new("granted/proj"), &grants),
            ROOT_NOT_A_FOLDER
        );
        assert_eq!(denied(Path::new(""), &grants), ROOT_NOT_A_FOLDER);
        // No grants, or only unusable grants (relative, missing, filesystem root).
        assert_eq!(denied(&project, &[]), ROOT_NOT_GRANTED);
        let root_of_fs = project.ancestors().last().unwrap().to_path_buf();
        let unusable = vec![
            PathBuf::from("granted"),
            base.join("never-created"),
            root_of_fs,
        ];
        assert_eq!(denied(&project, &unusable), ROOT_NOT_GRANTED);
        // The gate never creates anything.
        assert!(!base.join("never-created").exists());

        #[cfg(unix)]
        {
            // A symlink inside the granted folder that points outside is denied.
            let link = granted.join("link-out");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert_eq!(denied(&link, &grants), ROOT_NOT_GRANTED);
            // A granted folder reached through a symlink still matches its target.
            let granted_link = base.join("granted-link");
            std::os::unix::fs::symlink(&granted, &granted_link).unwrap();
            assert_eq!(
                admit_granted_root(&project, &[granted_link]).unwrap(),
                canon(&project)
            );
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn pure_module_has_no_tauri_or_adapter_imports() {
        let production = SELF_SRC.split("#[cfg(test)]").next().unwrap();
        for needle in [
            "use tauri",
            "tauri::",
            "pai_harness_adapter",
            "crate::",
            "super::",
            "unsafe",
        ] {
            assert!(
                !production.contains(needle),
                "pure module mentions {needle}"
            );
        }
    }

    #[test]
    fn glue_exposes_exactly_the_frozen_surface() {
        let found = commands(&GLUE);
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        let frozen: Vec<&str> = SURFACE.iter().map(|(n, _, _)| *n).collect();
        assert_eq!(names, frozen);
        for (command, (name, main_only, method)) in found.iter().zip(SURFACE) {
            assert!(
                command
                    .signature
                    .contains(&format!("pub(crate) async fn {name}(")),
                "{name}: must be a pub(crate) async command"
            );
            assert!(
                command
                    .signature
                    .contains("state: tauri::State<'_, CodingTaskState>"),
                "{name}: managed state param"
            );
            assert!(
                command.signature.contains("-> Result<"),
                "{name}: async commands with State must return Result"
            );
            // The window guard is the first statement exactly where the table says "main".
            let first = command.body.lines().map(str::trim).find(|l| !l.is_empty());
            if main_only {
                assert_eq!(
                    first,
                    Some("main_only(&window)?;"),
                    "{name}: main-window guard first"
                );
                assert!(command.signature.contains("window: tauri::WebviewWindow"));
            } else {
                assert!(
                    !command.body.contains("main_only("),
                    "{name}: table says no window check"
                );
            }
            // One delegation, off the executor, to the expected service method.
            assert_eq!(
                command.body.matches("service.").count(),
                1,
                "{name}: one service call"
            );
            assert!(
                command.body.contains(&format!("service.{method}(")),
                "{name}: delegates to {method}"
            );
            assert_eq!(
                command.body.matches("blocking(move ||").count(),
                1,
                "{name}: spawn_blocking"
            );
        }
    }

    #[test]
    fn capability_url_reaches_only_the_main_window() {
        let view = commands(&GLUE)
            .into_iter()
            .find(|c| c.name == "coding_task_view")
            .unwrap();
        assert!(view
            .body
            .contains("if !glue_policy::is_main_window(window.label())"));
        assert!(view.body.contains("view.preview.capability_url = None;"));
        // No other command returns a raw TaskView to a non-main caller.
        for command in commands(&GLUE) {
            let returns_view = command.signature.contains("Result<TaskView, String>")
                || command.signature.contains("Result<PreviewView, String>");
            if returns_view && command.name != "coding_task_view" {
                assert!(
                    command.body.contains("main_only(&window)?;"),
                    "{}",
                    command.name
                );
            }
        }
    }

    #[test]
    fn glue_has_no_spawn_host_lane_or_model_role_surface() {
        for needle in [
            "Command::new(",
            "std::process",
            "DesktopProcess",
            "process_lease",
            "full_access",
            "set_enabled",
            ".record_plan(",
            ".propose_edit(",
            ".record_narrative(",
            ".run_repair_loop(",
            "unsafe {",
            "unsafe fn",
        ] {
            assert!(!GLUE.contains(needle), "glue contains {needle}");
        }
        // Errors go through ui_error; the only Tauri APIs used are the ones
        // existing desktop commands already use.
        assert!(GLUE.contains("fn ui_error(error: TaskError) -> String"));
        for api in [
            "tauri::State<'_,",
            "tauri::WebviewWindow",
            "tauri::AppHandle",
            "tauri::async_runtime::spawn_blocking",
            "use tauri::Emitter;",
        ] {
            assert!(GLUE.contains(api), "{api}");
        }
    }

    #[test]
    fn coding_tasks_are_not_harness_or_model_tools() {
        for needle in [
            "coding_task",
            "CodingTask",
            "UiApplyEvent",
            "UiReconcileEvent",
        ] {
            assert!(
                !HARNESS_BRIDGE.contains(needle),
                "harness_bridge.rs mentions {needle}"
            );
        }
    }

    #[test]
    fn main_rs_wires_module_state_handlers_and_lock_hook() {
        assert!(MAIN.contains("\nmod coding_task_commands;\n"));
        assert!(MAIN.contains(
            "coding_task_commands::CodingTaskState::new(Arc::clone(&vault_state.vault))"
        ));
        assert_eq!(MAIN.matches(".manage(coding_task_state)").count(), 1);

        let handler = MAIN.split("tauri::generate_handler![").nth(1).unwrap();
        let handler = &handler[..handler.find("])").unwrap()];
        for (name, _, _) in SURFACE {
            assert_eq!(
                handler
                    .matches(&format!("coding_task_commands::{name},"))
                    .count(),
                1,
                "{name} registered once"
            );
        }
        assert_eq!(
            handler.matches("coding_task_commands::").count(),
            SURFACE.len()
        );

        // on_lock runs after emergency_lock() (admission closed) and before preview::emergency_stop.
        let stop = MAIN
            .split("fn stop_desktop_work(app: &tauri::AppHandle) {")
            .nth(1)
            .unwrap();
        let stop = &stop[..stop.find("\n}\n").unwrap()];
        let lock = stop
            .find("app.state::<DesktopVaultState>().emergency_lock();")
            .unwrap();
        let hook = stop
            .find("app.state::<coding_task_commands::CodingTaskState>()\n        .on_lock();")
            .unwrap();
        let preview = stop.find("preview::emergency_stop(app);").unwrap();
        assert!(lock < hook && hook < preview);
        assert_eq!(
            MAIN.matches("CodingTaskState>()\n        .on_lock()")
                .count(),
            1
        );
    }
}

//! Pure, std-only helpers for the Stage 6 knowledge Tauri glue (design §3.1).
//!
//! Like `coding_task_commands/glue_policy.rs`, this file has NO tauri and NO
//! adapter imports, so it compiles and runs its tests standalone
//! (`rustc --edition 2021 --test glue_policy.rs`) on hosts that cannot build
//! the desktop crate, and also as a normal child module of
//! `knowledge_commands`. Its tests also scan the glue and `main.rs` source to
//! pin the frozen §3.1 command surface and its wiring.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// The only webview label whose UI events may change knowledge state.
pub const MAIN_WINDOW_LABEL: &str = "main";

pub fn is_main_window(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL
}

const NOT_MAIN_WINDOW: &str = "knowledge actions are accepted only from the main UnoOne window";

pub fn require_main_window(label: &str) -> Result<(), String> {
    if is_main_window(label) {
        Ok(())
    } else {
        Err(NOT_MAIN_WINDOW.to_owned())
    }
}

/// `<temp>/unoone-knowledge` (design §3.1). Path only: creating it at first
/// use is the knowledge service's job.
pub fn scratch_dir(temp: &Path) -> PathBuf {
    temp.join("unoone-knowledge")
}

/// `<temp>/unoone-knowledge-learning`: the task-learning loop gets its own
/// sibling scratch base so the two services never share one private
/// directory (the spec names only the service scratch; recorded in the
/// Stage 6 U handoff). Path only.
pub fn learning_scratch_dir(temp: &Path) -> PathBuf {
    temp.join("unoone-knowledge-learning")
}

/// Held-out / evaluation path-name markers (case-insensitive substrings of a
/// relative path, design §3.1).
pub const EXCLUDED_NAME_MARKERS: [&str; 3] = ["heldout", "held-out", "acceptance-suite"];

pub fn excluded_name_markers() -> Vec<String> {
    EXCLUDED_NAME_MARKERS
        .iter()
        .map(|m| (*m).to_owned())
        .collect()
}

/// `<LOCALAPPDATA or HOME>/UnoOne/knowledge-exclusions.json`. Empty or
/// relative values count as unset (never resolved against the CWD).
pub fn exclusions_file(
    local_app_data: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    [local_app_data, home]
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|base| base.is_absolute())
        .map(|base| base.join("UnoOne").join("knowledge-exclusions.json"))
}

/// Upper bounds for the best-effort exclusions file.
pub const MAX_EXCLUSIONS_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_EXCLUSIONS: usize = 16_384;

/// Parse a JSON array of SHA-256 hex strings (`["<64 hex>", ...]`).
///
/// Entries are lowercased; entries that are not exactly 64 hex digits are
/// skipped. A document that is not a flat array of plain strings (escapes,
/// nesting, numbers, trailing garbage) yields `None`: the caller then uses an
/// empty set, exactly as for a missing file (design: "best-effort").
pub fn parse_exclusions(text: &str) -> Option<Vec<String>> {
    let body = text.trim_start_matches('\u{feff}').trim();
    let inner = body.strip_prefix('[')?.strip_suffix(']')?;
    let mut out = Vec::new();
    let mut rest = inner.trim();
    if rest.is_empty() {
        return Some(out);
    }
    loop {
        let after_quote = rest.strip_prefix('"')?;
        let end = after_quote.find('"')?;
        let value = &after_quote[..end];
        if value.contains('\\') {
            return None;
        }
        if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
            if out.len() >= MAX_EXCLUSIONS {
                return None;
            }
            out.push(value.to_ascii_lowercase());
        }
        rest = after_quote[end + 1..].trim_start();
        if rest.is_empty() {
            return Some(out);
        }
        rest = rest.strip_prefix(',')?.trim_start();
    }
}

/// Best-effort load: a missing, oversized, unreadable or malformed file is
/// an empty set. Never creates anything.
pub fn load_exclusions(path: Option<&Path>) -> Vec<String> {
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if !meta.is_file() || meta.len() > MAX_EXCLUSIONS_FILE_BYTES {
        return Vec::new();
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| parse_exclusions(&text))
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

const ROOT_NOT_A_FOLDER: &str =
    "knowledge source folder must be the absolute path of an existing folder";
const ROOT_NOT_GRANTED: &str = "knowledge source folder is not inside a folder you granted in Settings; grant the folder first (no grant is created here)";
const PATH_INVALID: &str =
    "knowledge source file must be a relative path inside the granted folder";

/// The coding_task_open (HA31) root gate, reused for
/// `DistillSource::LocalFile.root`: `requested` must be an absolute,
/// existing directory equal to or inside one of `granted`. Both sides are
/// canonicalised and compared component-wise; relative, missing or
/// filesystem-root grants are ignored. Returns the canonical root. Never
/// creates a grant; errors carry no path text.
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

/// Lexical pre-check of `DistillSource::LocalFile.path` (defence in depth;
/// the service still captures fd-safely under a snapshot policy for the
/// root): non-empty, at most 4096 bytes, no NUL, not absolute, no drive or
/// UNC prefix, no `..` component under either separator.
pub fn admit_relative_path(path: &str) -> Result<(), String> {
    let invalid = || PATH_INVALID.to_owned();
    if path.is_empty() || path.len() > 4096 || path.contains('\0') {
        return Err(invalid());
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(invalid());
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return Err(invalid());
    }
    if path.split(['/', '\\']).any(|part| part == "..") {
        return Err(invalid());
    }
    if Path::new(path)
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(invalid());
    }
    Ok(())
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
    static GLUE: LazyLock<String> = LazyLock::new(|| lf(include_str!("../knowledge_commands.rs")));
    static MAIN: LazyLock<String> = LazyLock::new(|| lf(include_str!("../main.rs")));
    static HARNESS_BRIDGE: LazyLock<String> =
        LazyLock::new(|| lf(include_str!("../harness_bridge.rs")));
    static SELF_SRC: LazyLock<String> = LazyLock::new(|| lf(include_str!("glue_policy.rs")));

    /// Design §3.1 frozen command surface:
    /// (command, main-window only, receiver, delegated call, emits update event).
    const SURFACE: [(&str, bool, &str, &str, bool); 19] = [
        ("knowledge_status", false, "service", "status", false),
        ("knowledge_initialize", true, "service", "initialize", true),
        (
            "knowledge_rebuild_index",
            true,
            "service",
            "rebuild_index",
            true,
        ),
        ("knowledge_search", false, "service", "search", false),
        ("knowledge_list", false, "service", "list", false),
        ("knowledge_detail", false, "service", "detail", false),
        ("knowledge_reject", true, "service", "reject", true),
        (
            "knowledge_revoke_approval",
            true,
            "service",
            "revoke_approval",
            true,
        ),
        (
            "knowledge_export_preview",
            false,
            "service",
            "export_preview",
            false,
        ),
        ("knowledge_export", true, "service", "export", false),
        (
            "knowledge_distill_preview",
            false,
            "",
            "distill_request_sha256",
            false,
        ),
        ("knowledge_distill", true, "service", "distill", true),
        (
            "knowledge_distill_runs",
            false,
            "service",
            "distill_runs",
            false,
        ),
        (
            "task_relevant_patterns",
            false,
            "learning",
            "relevant_patterns",
            false,
        ),
        (
            "task_propose_candidate",
            true,
            "learning",
            "propose_candidate",
            true,
        ),
        (
            "task_verification_preview",
            false,
            "learning",
            "verification_preview",
            false,
        ),
        (
            "task_verify_candidate",
            true,
            "learning",
            "verify_candidate",
            true,
        ),
        (
            "task_approve_pattern",
            true,
            "learning",
            "approve_pattern",
            true,
        ),
        (
            "task_revoke_pattern",
            true,
            "learning",
            "revoke_pattern",
            true,
        ),
    ];

    /// The JS argument keys after Tauri's camelCase conversion, i.e. the Rust
    /// parameter names other than window/app/state (design §3.1 table).
    const PARAMS: [(&str, &[&str]); 19] = [
        ("knowledge_status", &[]),
        ("knowledge_initialize", &["event: UiInitEvent"]),
        ("knowledge_rebuild_index", &[]),
        ("knowledge_search", &["query: KnowledgeQuery"]),
        ("knowledge_list", &["filter: KnowledgeListFilter"]),
        (
            "knowledge_detail",
            &["logical_id: String", "revision: Option<u32>"],
        ),
        ("knowledge_reject", &["event: UiRejectEvent"]),
        ("knowledge_revoke_approval", &["event: UiRevokeEvent"]),
        ("knowledge_export_preview", &["request: ExportRequest"]),
        ("knowledge_export", &["event: UiExportConsentEvent"]),
        ("knowledge_distill_preview", &["request: DistillRequest"]),
        (
            "knowledge_distill",
            &["request: DistillRequest", "event: UiDistillEvent"],
        ),
        ("knowledge_distill_runs", &[]),
        (
            "task_relevant_patterns",
            &["task_id: String", "limit: usize"],
        ),
        (
            "task_propose_candidate",
            &["task_id: String", "event: UiProposeCandidateEvent"],
        ),
        (
            "task_verification_preview",
            &["task_id: String", "candidate: RecordRef"],
        ),
        (
            "task_verify_candidate",
            &["task_id: String", "event: UiVerifyCandidateEvent"],
        ),
        (
            "task_approve_pattern",
            &["task_id: String", "event: UiApprovePatternEvent"],
        ),
        (
            "task_revoke_pattern",
            &["task_id: String", "event: UiRevokePatternEvent"],
        ),
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

    /// Parameter declarations of a command signature, split on top-level
    /// commas (generic arguments such as `State<'_, T>` stay whole).
    fn params(signature: &str) -> Vec<String> {
        let after_fn = signature.find(" fn ").expect("fn keyword");
        let open = after_fn + signature[after_fn..].find('(').expect("param list") + 1;
        let close = signature.find(") ->").expect("param list end");
        let mut out = Vec::new();
        let mut depth = 0i32;
        let mut current = String::new();
        for c in signature[open..close].chars() {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                _ => {}
            }
            if c == ',' && depth == 0 {
                out.push(current.trim().to_owned());
                current.clear();
            } else {
                current.push(c);
            }
        }
        out.push(current.trim().to_owned());
        out.retain(|p| !p.is_empty());
        out
    }

    fn unique_temp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "unoone-knowledge-glue-{tag}-{}-{nanos}",
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
            assert_eq!(require_main_window(label).unwrap_err(), NOT_MAIN_WINDOW);
        }
    }

    #[test]
    fn service_paths_and_markers_follow_the_design() {
        let temp = std::env::temp_dir();
        assert_eq!(scratch_dir(&temp), temp.join("unoone-knowledge"));
        assert_eq!(
            learning_scratch_dir(&temp),
            temp.join("unoone-knowledge-learning")
        );
        assert_ne!(scratch_dir(&temp), learning_scratch_dir(&temp));
        assert_eq!(
            excluded_name_markers(),
            vec![
                "heldout".to_owned(),
                "held-out".to_owned(),
                "acceptance-suite".to_owned()
            ]
        );

        let abs_a = temp.join("local-app-data");
        let abs_b = temp.join("home");
        let expect = |base: &Path| base.join("UnoOne").join("knowledge-exclusions.json");
        assert_eq!(
            exclusions_file(Some(abs_a.clone().into()), Some(abs_b.clone().into())),
            Some(expect(&abs_a))
        );
        assert_eq!(
            exclusions_file(None, Some(abs_b.clone().into())),
            Some(expect(&abs_b))
        );
        assert_eq!(
            exclusions_file(Some(OsString::new()), Some(abs_b.clone().into())),
            Some(expect(&abs_b))
        );
        assert_eq!(
            exclusions_file(Some("rel".into()), Some("also-rel".into())),
            None
        );
        assert_eq!(exclusions_file(None, None), None);
    }

    #[test]
    fn exclusions_parse_best_effort() {
        let a = "a".repeat(64);
        let upper = "ABCDEF0123456789".repeat(4);
        assert_eq!(parse_exclusions("[]"), Some(vec![]));
        assert_eq!(parse_exclusions(" \n[ ] \n"), Some(vec![]));
        assert_eq!(
            parse_exclusions(&format!("\u{feff}[\"{a}\" ,\n \"{upper}\"]")),
            Some(vec![a.clone(), upper.to_ascii_lowercase()])
        );
        // Entries that are not 64 hex digits are skipped, not fatal.
        assert_eq!(
            parse_exclusions(&format!("[\"{a}\", \"short\", \"{}\"]", "g".repeat(64))),
            Some(vec![a.clone()])
        );
        // Not a flat array of plain strings: the whole file is ignored.
        for bad in [
            "",
            "{}",
            "[1]",
            "[\"a\",]",
            "[\"a\" \"b\"]",
            "[[\"a\"]]",
            "[\"a\\\"b\"]",
            "[\"unterminated]",
            "null",
        ] {
            assert_eq!(parse_exclusions(bad), None, "{bad:?}");
        }
        let many = format!(
            "[{}]",
            vec![format!("\"{a}\""); MAX_EXCLUSIONS + 1].join(",")
        );
        assert_eq!(parse_exclusions(&many), None, "entry cap");
    }

    #[test]
    fn exclusions_load_never_fails_and_never_creates() {
        let base = unique_temp("exclusions");
        let missing = base.join("UnoOne").join("knowledge-exclusions.json");
        assert!(load_exclusions(Some(&missing)).is_empty());
        assert!(
            !base.join("UnoOne").exists(),
            "load never creates the folder"
        );
        assert!(load_exclusions(None).is_empty());
        let file = base.join("x.json");
        let h = "0f".repeat(32);
        std::fs::write(&file, format!("[\"{h}\"]")).unwrap();
        assert_eq!(load_exclusions(Some(&file)), vec![h]);
        std::fs::write(&file, "not json").unwrap();
        assert!(load_exclusions(Some(&file)).is_empty());
        assert!(
            load_exclusions(Some(&base)).is_empty(),
            "a directory is not a file"
        );
        std::fs::remove_dir_all(&base).unwrap();
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
        let project = granted.join("notes");
        let sibling = base.join("granted-evil");
        let outside = base.join("outside");
        for dir in [&project, &sibling, &outside] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(project.join("guide.md"), b"SECRET-MARKER").unwrap();
        let grants = vec![granted.clone()];
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap();
        assert_eq!(
            admit_granted_root(&project, &grants).unwrap(),
            canon(&project)
        );
        assert_eq!(
            admit_granted_root(&granted, &grants).unwrap(),
            canon(&granted)
        );
        let dotted = granted.join("notes").join("..").join("notes");
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
        assert_eq!(denied(&sibling, &grants), ROOT_NOT_GRANTED);
        assert_eq!(
            denied(&granted.join("..").join("outside"), &grants),
            ROOT_NOT_GRANTED
        );
        assert_eq!(denied(&outside, &grants), ROOT_NOT_GRANTED);
        assert_eq!(denied(&granted.join("missing"), &grants), ROOT_NOT_A_FOLDER);
        assert_eq!(
            denied(&project.join("guide.md"), &grants),
            ROOT_NOT_A_FOLDER
        );
        assert_eq!(
            denied(Path::new("granted/notes"), &grants),
            ROOT_NOT_A_FOLDER
        );
        assert_eq!(denied(Path::new(""), &grants), ROOT_NOT_A_FOLDER);
        assert_eq!(denied(&project, &[]), ROOT_NOT_GRANTED);
        let root_of_fs = project.ancestors().last().unwrap().to_path_buf();
        let unusable = vec![
            PathBuf::from("granted"),
            base.join("never-created"),
            root_of_fs,
        ];
        assert_eq!(denied(&project, &unusable), ROOT_NOT_GRANTED);
        assert!(!base.join("never-created").exists());
        #[cfg(unix)]
        {
            let link = granted.join("link-out");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert_eq!(denied(&link, &grants), ROOT_NOT_GRANTED);
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
    fn relative_path_precheck() {
        for ok in [
            "guide.md",
            "docs/guide.md",
            "./docs/guide.md",
            "a/b/c.py",
            "docs\\guide.md",
            "..notes.md",
            "x..y/z",
        ] {
            assert!(admit_relative_path(ok).is_ok(), "{ok:?}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "\\\\server\\share\\x",
            "\\x",
            "C:\\x",
            "c:x",
            "../x",
            "docs/../../x",
            "docs\\..\\x",
            "a\0b",
            "..",
        ] {
            assert_eq!(
                admit_relative_path(bad).unwrap_err(),
                PATH_INVALID,
                "{bad:?}"
            );
        }
        assert!(admit_relative_path(&"a".repeat(4097)).is_err());
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
        let frozen: Vec<&str> = SURFACE.iter().map(|s| s.0).collect();
        assert_eq!(names, frozen);
        for (command, (name, main_only, receiver, method, emits)) in found.iter().zip(SURFACE) {
            assert!(
                command
                    .signature
                    .contains(&format!("pub(crate) async fn {name}(")),
                "{name}: must be a pub(crate) async command"
            );
            // The pure hash helper needs no managed state; every service command takes it.
            assert_eq!(
                command
                    .signature
                    .contains("state: tauri::State<'_, KnowledgeState>"),
                !receiver.is_empty(),
                "{name}: managed state param"
            );
            assert!(
                command.signature.contains("-> Result<"),
                "{name}: Result return"
            );
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
            // One delegation, off the executor, to the expected method.
            let calls = command.body.matches("service.").count()
                + command.body.matches("learning.").count();
            if receiver.is_empty() {
                assert_eq!(calls, 0, "{name}: pure helper, no service call");
                assert_eq!(
                    command.body.matches(&format!("{method}(")).count(),
                    1,
                    "{name}: {method}"
                );
            } else {
                assert_eq!(calls, 1, "{name}: one service call");
                assert!(
                    command.body.contains(&format!("{receiver}.{method}(")),
                    "{name}: delegates to {receiver}.{method}"
                );
            }
            assert_eq!(
                command.body.matches("blocking(move ||").count(),
                1,
                "{name}: spawn_blocking"
            );
            // The content-free update event follows exactly the state-changing commands.
            assert_eq!(
                command.body.contains("notify(&app,"),
                emits,
                "{name}: update event"
            );
            assert_eq!(
                command.signature.contains("app: tauri::AppHandle"),
                emits,
                "{name}: app handle only when emitting"
            );
        }
    }

    #[test]
    fn glue_params_match_the_js_argument_table() {
        let found = commands(&GLUE);
        for (command, (name, expected)) in found.iter().zip(PARAMS) {
            assert_eq!(command.name, name);
            let got: Vec<String> = params(&command.signature)
                .into_iter()
                .filter(|p| {
                    !p.starts_with("window:") && !p.starts_with("app:") && !p.starts_with("state:")
                })
                .collect();
            let want: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
            assert_eq!(
                got, want,
                "{name}: parameter names/types (JS keys are their camelCase)"
            );
        }
    }

    #[test]
    fn local_file_roots_pass_the_grant_gate_before_hashing_and_distilling() {
        let found = commands(&GLUE);
        for name in ["knowledge_distill_preview", "knowledge_distill"] {
            let command = found.iter().find(|c| c.name == name).unwrap();
            let admit = command
                .body
                .find("admit_local_sources(request).await?")
                .expect(name);
            let call = command.body.find("blocking(move ||").unwrap();
            assert!(
                admit < call,
                "{name}: grant gate runs before the service call"
            );
        }
        assert!(GLUE.contains("glue_policy::admit_granted_root(root, &granted)?"));
        assert!(GLUE.contains("glue_policy::admit_relative_path(path)?"));
        assert!(GLUE.contains("crate::harness_bridge::get_agent_workspace_info().await?"));
    }

    #[test]
    fn task_ids_are_parsed_and_errors_are_fixed_classifications() {
        let found = commands(&GLUE);
        for command in found.iter().filter(|c| c.name.starts_with("task_")) {
            assert!(
                command.body.contains("let task = task_id_of(&task_id)?;"),
                "{}",
                command.name
            );
        }
        assert!(GLUE.contains("fn task_id_of(raw: &str) -> Result<TaskId, String>"));
        assert!(GLUE.contains("TaskId::parse(raw).map_err(|_| ui_error(LearningError::Invalid))"));
        assert!(GLUE.contains("fn ui_error<E: std::fmt::Display>(error: E) -> String"));
    }

    #[test]
    fn glue_has_no_spawn_host_lane_or_model_surface() {
        for needle in [
            "Command::new(",
            "std::process",
            "DesktopProcess",
            "process_lease",
            "full_access",
            "set_enabled",
            "unsafe {",
            "unsafe fn",
            "std::fs::write",
            "File::create",
        ] {
            assert!(!GLUE.contains(needle), "glue contains {needle}");
        }
        for api in [
            "tauri::State<'_,",
            "tauri::WebviewWindow",
            "tauri::AppHandle",
            "tauri::async_runtime::spawn_blocking",
            "use tauri::Emitter;",
        ] {
            assert!(GLUE.contains(api), "{api}");
        }
        assert!(
            GLUE.contains("pub(crate) const UPDATED_EVENT: &str = \"unoone:knowledge-updated\";")
        );
    }

    #[test]
    fn knowledge_is_not_a_harness_or_model_tool() {
        for needle in [
            "knowledge_commands",
            "KnowledgeState",
            "TaskLearning",
            "approve_pattern",
            "UiDistillEvent",
        ] {
            assert!(
                !HARNESS_BRIDGE.contains(needle),
                "harness_bridge.rs mentions {needle}"
            );
        }
    }

    #[test]
    fn main_rs_wires_module_state_and_handlers() {
        assert!(MAIN.contains("\nmod knowledge_commands;\n"));
        // Same canonical vault Arc and the SAME CodingTaskService Arc.
        assert!(MAIN.contains(
            "knowledge_commands::KnowledgeState::new(\n        Arc::clone(&vault_state.vault),\n        Arc::clone(&coding_task_state.0),\n    );"
        ));
        let state_at = MAIN.find("let knowledge_state =").unwrap();
        let coding_at = MAIN.find("let coding_task_state =").unwrap();
        assert!(coding_at < state_at);
        assert_eq!(MAIN.matches(".manage(knowledge_state)").count(), 1);
        let manage_coding = MAIN.find(".manage(coding_task_state)").unwrap();
        let manage_knowledge = MAIN.find(".manage(knowledge_state)").unwrap();
        assert!(
            manage_coding < manage_knowledge,
            "knowledge_state is built from coding_task_state before it moves"
        );

        let handler = MAIN.split("tauri::generate_handler![").nth(1).unwrap();
        let handler = &handler[..handler.find("])").unwrap()];
        for (name, _, _, _, _) in SURFACE {
            assert_eq!(
                handler
                    .matches(&format!("knowledge_commands::{name},"))
                    .count(),
                1,
                "{name} registered once"
            );
        }
        assert_eq!(
            handler.matches("knowledge_commands::").count(),
            SURFACE.len()
        );
        // Stage 5 wiring is unchanged by Stage 6.
        assert_eq!(handler.matches("coding_task_commands::").count(), 19);
        assert_eq!(
            MAIN.matches("CodingTaskState>()\n        .on_lock()")
                .count(),
            1
        );

        // K1/K2 expose on_lock, so (design §3.1) it runs after emergency_lock()
        // and the coding-task hook, before the preview stop.
        let stop = MAIN
            .split("fn stop_desktop_work(app: &tauri::AppHandle) {")
            .nth(1)
            .unwrap();
        let stop = &stop[..stop.find("\n}\n").unwrap()];
        let lock = stop
            .find("app.state::<DesktopVaultState>().emergency_lock();")
            .unwrap();
        let coding = stop
            .find("app.state::<coding_task_commands::CodingTaskState>()\n        .on_lock();")
            .unwrap();
        let knowledge = stop
            .find("app.state::<knowledge_commands::KnowledgeState>().on_lock();")
            .unwrap();
        let preview = stop.find("preview::emergency_stop(app);").unwrap();
        assert!(lock < coding && coding < knowledge && knowledge < preview);
        assert_eq!(MAIN.matches("KnowledgeState>().on_lock()").count(), 1);
        assert!(GLUE.contains(
            "pub(crate) fn on_lock(&self) {\n        self.service.on_lock();\n        self.learning.on_lock();\n    }"
        ));
    }
}

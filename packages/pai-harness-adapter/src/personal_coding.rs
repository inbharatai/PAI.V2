//! Personal reviewed coding-check template, always the existing isolated coding backend.
//! No shell/host-command fallback, file application, model code or automatic repairs.
use crate::{
    coding_task::*,
    task_ledger::{AcceptanceCriterion, CriterionCheck, OracleVisibility, TaskId},
};
use ports::*;
use std::{path::PathBuf, sync::Arc};

/// Fixed bounded read-only Python check. User chooses exactly one file in an existing grant;
/// the native caller checks folder authority. Existing service owns isolation/path/oracle gates.
pub fn check_selected_python(
    service: Arc<CodingTaskService>,
    root: PathBuf,
    relative: String,
    objective: String,
    active: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> Result<TaskView, String> {
    active()?;
    if !matches!(
        service.capability(),
        IsolationCapability::RuntimeVerified { .. }
    ) {
        return Err("Isolation unavailable/unverified on this host. No unfenced fallback.".into());
    }
    if !relative.ends_with(".py") || !crate::task_ledger::valid_rel_path(&relative) {
        return Err("Select one relative Python file".into());
    }
    let plan = GatePlan {
        schema: GATE_PLAN_SCHEMA.into(),
        commands: vec![GateCommand::PythonCompile {
            id: "personal-python-check".into(),
            role: GateRole::Build,
            files: vec![relative.clone()],
            timeout_ms: 15_000,
            output_bytes: 2048,
        }],
        stop_on_failure: true,
        copy_out: CopyOutRequest {
            mode: CopyOutMode::Off,
            ignore_dir_names: CopyOutRequest::default_ignores(),
            max_files: 1,
            max_total_bytes: 4096,
        },
        limits: WorkspaceLimits {
            cpu_seconds: 15,
            memory_bytes: 256 * 1024 * 1024,
            processes: 8,
            work_tmpfs_bytes: 8 * 1024 * 1024,
            file_size_bytes: 1024 * 1024,
            open_files: 64,
            total_timeout_ms: 20_000,
            rss_watchdog_bytes: 512 * 1024 * 1024,
        },
    };
    let opened = service
        .open_task(OpenTaskRequest {
            root,
            files: vec![relative.clone()],
            primary: relative,
            oracle_files: vec![],
            oracle_visibility: OracleVisibility::Visible,
            objective,
            acceptance: vec![AcceptanceCriterion {
                id: "python-check".into(),
                text:
                    "Selected Python file compiles in isolation; not proof of wider task completion"
                        .into(),
                check: CriterionCheck::GateCommand {
                    command_id: "personal-python-check".into(),
                    expected_exit: 0,
                },
                confirmed_by_user: true,
            }],
            gate_plan: plan,
            preview: None,
            repair: RepairBudget {
                max_attempts: 0,
                max_total_gate_ms: 20_000,
            },
            allowed_new_prefixes: vec![],
        })
        .map_err(|e| e.to_string())?;
    let id = TaskId::parse(&opened.task_id).map_err(|e| e.to_string())?;
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watchdog = {
        let done = done.clone();
        let service = service.clone();
        let id = id.clone();
        let active = active.clone();
        std::thread::spawn(move || {
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                if active().is_err() {
                    let _ = service.cancel(&id);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        })
    };
    let result = active().and_then(|_| {
        service
            .run_gate(&id, GateTarget::Current)
            .map_err(|e| e.to_string())
    });
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = watchdog.join();
    active()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[test]
    fn personal_coding_actual_backend_or_explicit_host_block_no_fallback() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let work = root.path().join("work");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(project.join("selected.py"), "answer = 42\n").unwrap();
        let vaultroot = root.path().join("vault");
        std::fs::create_dir(&vaultroot).unwrap();
        unoone_vault_core::Vault::create(&vaultroot, b"coding-scope-fixture").unwrap();
        let mut vault = unoone_vault_core::Vault::open(&vaultroot).unwrap();
        vault.unlock(b"coding-scope-fixture").unwrap();
        let vault = Arc::new(Mutex::new(Some(vault)));
        for platform in [PlatformClass::NonLinux, PlatformClass::current()] {
            let service = Arc::new(CodingTaskService::new(
                vault.clone(),
                ServiceConfig {
                    scratch: root.path().join("scratch"),
                    worktree_base: work.clone(),
                    worktree_deny_within: vec![vaultroot.clone()],
                    platform,
                    host_commands_enabled: true,
                },
            ));
            let capability = service.capability();
            let result = check_selected_python(
                service.clone(),
                project.clone(),
                "selected.py".into(),
                "Check this selected file".into(),
                Arc::new(|| Ok(())),
            );
            if matches!(capability, IsolationCapability::RuntimeVerified { .. }) {
                let view = result.unwrap();
                assert!(!view.task_id.is_empty());
                assert_eq!(view.gates.len(), 1);
                assert_eq!(view.gates[0].commands[0].status, Some(0));
                println!("REAL_ISOLATED_GATE_EXECUTED");
            } else {
                assert!(result.is_err());
                assert!(service.list_tasks().unwrap().is_empty());
                println!("HOST_BLOCKED_NO_FALLBACK: {capability:?}");
            }
            assert_eq!(
                std::fs::read_to_string(project.join("selected.py")).unwrap(),
                "answer = 42\n"
            );
        }
    }
}

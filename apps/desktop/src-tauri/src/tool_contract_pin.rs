//! Desktop registry pin: every tool the Power harness lanes actually register must be
//! declared in `packages/tool-contracts/tools.v1.json` with matching harness facts.
//!
//! This is the Rust half of the tool-contract drift gate. The Kotlin half is
//! `scripts/check_tool_contract_sync.py` (android tables), run in both CI workflows.
//! `browser.act` and `agent.spawn` construct their manifests inline behind an
//! `AppHandle` (not constructible in a unit test), so this test pins their ids in the
//! contract and fully pins manifest facts for the other 11 desktop tools.

#[cfg(test)]
mod tests {
    use crate::harness_bridge::{
        desktop_read_tools, DesktopDocCreateTool, DesktopPatchTool, DesktopSearchTool,
    };
    use crate::safety::DesktopSafetyGuard;
    use inbharat_harness_core::tools::{
        ListFilesTool, MakeDirTool, ReadFileTool, RunProcessTool, WriteFileTool,
    };
    use inbharat_harness_core::{RootedFs, Tool};
    use std::sync::{Arc, Mutex};
    use unoone_tool_contracts::{HarnessFacts, ToolContract};
    use unoone_vault_core::Vault;

    fn contract() -> ToolContract {
        ToolContract::parse().expect("tools.v1.json must parse")
    }

    /// (id, side_effect, confirmation_mode, timeout_ms) for a constructed tool.
    fn facts(tool: &dyn Tool) -> (String, String, String, u64) {
        let m = tool.manifest();
        (
            m.id.clone(),
            format!("{:?}", m.side_effect),
            format!("{:?}", m.confirmation),
            m.default_timeout.as_millis() as u64,
        )
    }

    fn assert_matches_contract(tool: &dyn Tool, contract: &ToolContract) {
        let (id, side, conf, timeout_ms) = facts(tool);
        let entry = contract.tool(&id).unwrap_or_else(|| {
            panic!("tool '{id}' is registered in production but missing from tools.v1.json")
        });
        assert_eq!(
            entry.platforms,
            vec!["desktop".to_owned()],
            "{id}: registered desktop tool must be declared platform=desktop"
        );
        assert_eq!(
            entry.status, "active",
            "{id}: registered production tool must be status=active"
        );
        let h: &HarnessFacts = entry
            .harness
            .as_ref()
            .unwrap_or_else(|| panic!("{id}: contract entry must carry harness facts"));
        assert_eq!(h.side_effect, side, "{id}: side_effect drift");
        assert_eq!(h.confirmation_mode, conf, "{id}: confirmation drift");
        assert_eq!(
            entry.timeout_ms,
            Some(timeout_ms),
            "{id}: timeout drift (contract={:?}, manifest={timeout_ms}ms)",
            entry.timeout_ms
        );
        // Risk class follows the documented confirmation-mode mapping.
        let expected_risk = match conf.as_str() {
            "Always" => "STRONG_CONFIRM",
            "OnSideEffect" => "CONFIRM",
            _ => "DIRECT",
        };
        assert_eq!(entry.risk_class, expected_risk, "{id}: risk mapping drift");
    }

    #[test]
    fn production_registry_matches_the_tool_contract() {
        let contract = contract();

        // --- Read-only lane (always registered) ---
        let safety = Arc::new(Mutex::new(DesktopSafetyGuard::new_with_vault_root(
            "C:/nonexistent-vault",
        )));
        let vault: Arc<Mutex<Option<Vault>>> = Arc::new(Mutex::new(None));
        let read_tools = desktop_read_tools("C:/nonexistent-vault", vault, safety);
        assert_eq!(read_tools.len(), 4, "read lane registers 4 tools");
        for t in &read_tools {
            assert_matches_contract(&**t, &contract);
        }

        // --- Full-access lane: harness core built-ins + bridge tools ---
        // browser.act / agent.spawn need a live AppHandle — their ids are pinned
        // by the completeness assertion below.
        let dir = std::env::temp_dir().join(format!(
            "unoone-tool-contract-pin-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let fs = RootedFs::new(&dir).expect("fence");
        // The search/patch tools are fenced to the granted-folder set —
        // here a single-root set standing in for the run-time union.
        let folders = crate::granted_fs::GrantedFolders::new(vec![fs]).expect("granted-folder set");

        let constructible: Vec<Box<dyn Tool>> = vec![
            Box::new(ReadFileTool::default()),
            Box::new(ListFilesTool::default()),
            Box::new(WriteFileTool::default()),
            Box::new(MakeDirTool::default()),
            Box::new(RunProcessTool::default()),
            Box::new(DesktopSearchTool::new(folders.clone())),
            Box::new(DesktopPatchTool::new(folders.clone())),
            Box::new(DesktopDocCreateTool::new(folders.clone())),
        ];
        for t in &constructible {
            assert_matches_contract(t.as_ref(), &contract);
        }

        // --- Completeness: the contract's desktop id set == the real registry ---
        let mut registered: Vec<String> = read_tools
            .iter()
            .map(|t: &Arc<dyn Tool>| t.manifest().id.clone())
            .collect();
        registered.extend(constructible.iter().map(|t| t.manifest().id.clone()));
        registered.push("browser.act".to_owned());
        registered.push("agent.spawn".to_owned());
        registered.sort();
        let mut declared = contract.desktop_active_ids();
        declared.sort();
        assert_eq!(
            registered, declared,
            "the desktop registry and tools.v1.json disagree — update both in the same commit"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_lane_confirmation_modes_never_bypass_confirmation_stage() {
        // Safety cross-check: no desktop tool may declare a side effect while
        // claiming ConfirmationMode::Never (the harness manifest validator
        // forbids it; this pins the contract to the same rule).
        let contract = contract();
        for t in &contract.tools {
            if let Some(h) = &t.harness {
                let has_side_effect = !matches!(h.side_effect.as_str(), "None" | "Read");
                if has_side_effect {
                    assert_ne!(
                        h.confirmation_mode, "Never",
                        "{}: side-effect tool must require confirmation",
                        t.id
                    );
                }
            }
        }
    }
}

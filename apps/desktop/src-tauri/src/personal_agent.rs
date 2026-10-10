//! Main-window-only local consent. Deliberately disconnected from model/tool IPC.
use crate::DesktopVaultState;
use std::sync::atomic::Ordering;
use tauri::Manager;
use unoone_personal_agent_runtime::{load, save, Request, View};

#[tauri::command]
pub async fn personal_agent_view(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopVaultState>,
) -> Result<View, String> {
    if window.label() != "main" {
        return Err("Personal agent is available only in the main window".into());
    }
    let epoch = state.lock_epoch.load(Ordering::SeqCst);
    let vault = state.vault.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = vault.lock().map_err(|_| "Vault unavailable")?;
        if app
            .state::<DesktopVaultState>()
            .lock_epoch
            .load(Ordering::SeqCst)
            != epoch
        {
            return Err("Session changed; reopen the personal agent".into());
        }
        load(guard.as_mut().ok_or("Unlock the local vault first")?)?.view()
    })
    .await
    .map_err(|_| "Personal agent worker failed")?
}

#[tauri::command]
pub async fn personal_agent_mutate(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopVaultState>,
    request: Request,
) -> Result<View, String> {
    if window.label() != "main" {
        return Err("Local user review required in main window".into());
    }
    let epoch = state.lock_epoch.load(Ordering::SeqCst);
    let vault = state.vault.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = vault.lock().map_err(|_| "Vault unavailable")?;
        if app
            .state::<DesktopVaultState>()
            .lock_epoch
            .load(Ordering::SeqCst)
            != epoch
        {
            return Err("Session changed; reopen the personal agent".into());
        }
        let vault = guard.as_mut().ok_or("Unlock the local vault first")?;
        let mut ledger = load(vault)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "Clock unavailable")?
            .as_millis() as u64;
        ledger.apply(request, now)?;
        if app
            .state::<DesktopVaultState>()
            .lock_epoch
            .load(Ordering::SeqCst)
            != epoch
        {
            return Err("Session changed; local edit cancelled".into());
        }
        save(vault, &ledger)?;
        ledger.view()
    })
    .await
    .map_err(|_| "Personal agent worker failed")?
}

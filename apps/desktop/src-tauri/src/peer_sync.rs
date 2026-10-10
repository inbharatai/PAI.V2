//! Explicit main-window local peer review. No hidden service, dispatcher or master-key export.
use crate::DesktopVaultState;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tauri::Manager;
use unoone_local_peer_sync::{self as sync, IdentityChoice, Offer, Selection, Status};
use unoone_personal_agent_runtime as runtime;
static SESSION: AtomicU64 = AtomicU64::new(0);

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub action: String,
    pub offer: Option<Offer>,
    pub selection: Option<Selection>,
    pub choice: Option<IdentityChoice>,
    pub compared_fingerprint: bool,
    pub address: String,
}
#[tauri::command]
pub async fn peer_sync_cancel(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Main window only".into());
    }
    SESSION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}
#[tauri::command]
pub async fn peer_sync_command(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopVaultState>,
    command: Command,
) -> Result<Status, String> {
    if window.label() != "main" {
        return Err("Pairing requires local user in main window".into());
    }
    if command.address.len() > 80 {
        return Err("Address too long".into());
    }
    let epoch = state.lock_epoch.load(Ordering::SeqCst);
    let vault = state.vault.clone();
    let generation = SESSION.fetch_add(1, Ordering::SeqCst) + 1;
    let guard_app = app.clone();
    let session: sync::tls::SessionGuard = Arc::new(move || {
        SESSION.load(Ordering::SeqCst) == generation
            && guard_app
                .state::<DesktopVaultState>()
                .lock_epoch
                .load(Ordering::SeqCst)
                == epoch
    });
    tauri::async_runtime::spawn_blocking(move || {
        let (snapshot, ledger) = {
            let mut lock = vault.lock().map_err(|_| "Vault unavailable")?;
            sync::require(session(), "Session changed; reopen peer panel")?;
            let v = lock.as_mut().ok_or("Unlock vault first")?;
            let ledger = runtime::load(v)?;
            let mut s = sync::load(v, &ledger)?;
            match command.action.as_str() {
                "status" => return s.status(&ledger),
                "approve" => {
                    s.approve(
                        command.offer.ok_or("Missing offer")?,
                        command.selection.ok_or("Choose records")?,
                        command.choice.ok_or("Choose identity policy")?,
                        command.compared_fingerprint,
                        &ledger,
                    )?;
                    sync::save(v, &s)?;
                    return s.status(&ledger);
                }
                "revoke" => {
                    s.peer.as_mut().ok_or("No peer")?.revoked = true;
                    sync::save(v, &s)?;
                    return s.status(&ledger);
                }
                "listen" | "sync" => {
                    s.active()?;
                    (s, ledger)
                }
                _ => return Err("Unknown peer action".into()),
            }
        };
        // No vault mutex held during socket waits. Lock epoch/session guard checked on every socket read/write.
        if command.action == "listen" {
            sync::tls::listen_once(&command.address, &snapshot, session.clone(), |request| {
                let mut lock = vault.lock().map_err(|_| "Vault unavailable")?;
                sync::require(session(), "Session ended; no ACK")?;
                let v = lock.as_mut().ok_or("Unlock vault first")?;
                let mut local = runtime::load(v)?;
                let current = sync::load(v, &local)?;
                let mut next = current.receive(&request.page)?;
                next.merge_into(&mut local)?;
                let page = next.page(&local, request.want_after)?;
                next.sent_ack = next.sent_ack.max(request.want_after);
                sync::require(session(), "Session ended; no ACK")?;
                runtime::save(v, &local)?;
                sync::save(v, &next)?;
                Ok(sync::Reply {
                    page,
                    acknowledged: next.received.len() as u64,
                })
            })?;
        } else {
            let page = snapshot.page(&ledger, snapshot.sent_ack)?;
            let last_sent = page.changes.last().map_or(page.after, |c| c.sequence);
            let reply = sync::tls::connect_guarded(
                &command.address,
                &snapshot,
                &sync::Exchange {
                    page,
                    want_after: snapshot.received.len() as u64,
                },
                session.clone(),
            )?;
            let mut lock = vault.lock().map_err(|_| "Vault unavailable")?;
            sync::require(session(), "Session ended; received data not applied")?;
            let v = lock.as_mut().ok_or("Unlock vault first")?;
            let mut local = runtime::load(v)?;
            let current = sync::load(v, &local)?;
            sync::require(
                reply.acknowledged >= current.sent_ack && reply.acknowledged <= last_sent,
                "Invalid peer ACK",
            )?;
            let mut next = current.receive(&reply.page)?;
            next.sent_ack = reply.acknowledged;
            next.merge_into(&mut local)?;
            runtime::save(v, &local)?;
            sync::save(v, &next)?;
        }
        let mut lock = vault.lock().map_err(|_| "Vault unavailable")?;
        sync::require(session(), "Session ended")?;
        let v = lock.as_mut().ok_or("Unlock vault first")?;
        let local = runtime::load(v)?;
        sync::load(v, &local)?.status(&local)
    })
    .await
    .map_err(|_| "Peer worker failed")?
}

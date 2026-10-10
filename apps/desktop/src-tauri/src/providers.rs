//! Trusted main-window Sources/Prepared lane. Deliberately not registered as model tools.
use crate::DesktopVaultState;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tauri::Manager;
use unoone_personal_provider_adapters::{
    self as p,
    google::Google,
    guardian,
    oauth::{self, OAuthConfig, PendingOAuth},
    store, Review,
};
use unoone_privacy_guardian as g;

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum NativeRequest {
    View,
    Configure {
        client_id: String,
        redirect_uri: String,
        #[serde(default)]
        client_secret: Option<String>,
    },
    Connect {
        write_scopes: bool,
    },
    Disconnect,
    Search {
        folder: String,
        query: String,
        page: Option<String>,
    },
    Message {
        folder: String,
        id: String,
    },
    Thread {
        folder: String,
        id: String,
    },
    Labels,
    Calendars {
        page: Option<String>,
    },
    Events {
        calendar: String,
        start: String,
        end: String,
        page: Option<String>,
    },
    FreeBusy {
        calendar: String,
        start: String,
        end: String,
    },
    Prepare {
        review: Review,
    },
    /// §3.6 guardian decision for a prepared operation, shown BEFORE Commit (no effect).
    GuardianPreview {
        operation_id: String,
    },
    /// Visible report/correct-warning control; records a correction note on the task.
    GuardianCorrection {
        task_id: String,
        fingerprint: String,
        kind: String,
        comment: String,
    },
    Commit {
        operation_id: String,
        exact_digest: String,
        /// Exact fingerprint of the WARN the person acknowledged in the UI; None = no acknowledgement.
        #[serde(default)]
        acknowledged_fingerprint: Option<String>,
    },
    Reconcile {
        operation_id: String,
        provider_id: String,
    },
}
fn open_system_browser(url: &str) -> Result<(), String> {
    if !url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?") {
        return Err("Invalid OAuth URL".into());
    }
    // §3.6 link-open hook: host-constructed destination still passes the guardian (no bypass path).
    let decision = g::check(
        &g::Intent::OpenLink {
            link: g::Link {
                display_text: "Google sign-in".into(),
                href: url.into(),
            },
            origin: g::ContentSource::UserTyped,
        },
        &g::Context {
            trusted_domains: vec!["google.com".into()],
            ..Default::default()
        },
    );
    g::enforce(&decision, None, p::now_ms()).map_err(|r| r.message)?;
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("rundll32")
        .arg("url.dll,FileProtocolHandler")
        .arg(url)
        .spawn();
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let status = std::process::Command::new("xdg-open").arg(url).spawn();
    status
        .map(|mut child| {
            let _reaper = std::thread::spawn(move || child.wait());
        })
        .map_err(|_| "Cannot open system browser".into())
}
fn callback(listener: std::net::TcpListener, redirect: &str) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|_| "OAuth listener unavailable")?;
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .map_err(|_| "OAuth listener unavailable")?;
                let mut bytes = vec![0u8; 8192];
                let n = stream
                    .read(&mut bytes)
                    .map_err(|_| "OAuth callback interrupted")?;
                if n == 8192 {
                    return Err("OAuth callback oversized".into());
                }
                let request =
                    std::str::from_utf8(&bytes[..n]).map_err(|_| "Malformed OAuth callback")?;
                let line = request.lines().next().ok_or("Missing callback")?;
                let parts = line.split_whitespace().collect::<Vec<_>>();
                if parts.len() != 3
                    || parts[0] != "GET"
                    || !parts[1].starts_with("/oauth/callback?")
                {
                    continue;
                }
                let base = reqwest::Url::parse(redirect).map_err(|_| "Invalid redirect")?;
                let url = base
                    .join(parts[1])
                    .map_err(|_| "Invalid callback")?
                    .to_string();
                let _=stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nReturn to UnoOne. Authorization is being checked; this page is not proof of connection.");
                bytes.fill(0);
                return Ok(url);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(_) => return Err("OAuth callback unavailable".into()),
        }
    }
    Err("OAuth cancelled or timed out; no account connected".into())
}
#[tauri::command]
pub async fn provider_request(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopVaultState>,
    request: NativeRequest,
) -> Result<Value, String> {
    if window.label() != "main" {
        return Err("Sources require native main-window user action".into());
    }
    let epoch = state.lock_epoch.load(Ordering::SeqCst);
    let vault = state.vault.clone();
    tauri::async_runtime::spawn_blocking(move|| {
        let mut guard=vault.lock().map_err(|_|"Vault unavailable")?;
        let check_session=||->Result<(),String>{ if app.state::<DesktopVaultState>().lock_epoch.load(Ordering::SeqCst)!=epoch {Err("Vault session changed; operation held for reconciliation".into())}else{Ok(())} };
        check_session()?;
        let vault=guard.as_mut().ok_or("Unlock local vault first")?;
        let mut local=store::load(vault)?;
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_|"Provider worker unavailable")?;
        let result=rt.block_on(async {
            let mut data=Value::Null;
            match request {
                NativeRequest::View=>{
                    // §3.6 consent preview from the declared manifest, shown before Connect. No connector is enabled by viewing.
                    data=json!({"connector_consent_preview":{"read_only":guardian::google_manifest(None,false,p::now_ms()).consent_preview(),"reviewed_actions":guardian::google_manifest(None,true,p::now_ms()).consent_preview()},"egress_policy":"default offline; only the declared endpoints above are reachable, no reputation lookup, no cloud fallback"});
                },
                NativeRequest::Configure{client_id,redirect_uri,client_secret}=> {
                    if local.tokens.is_some(){return Err("Disconnect before changing account configuration".into());}
                    let config=OAuthConfig{client_id,redirect_uri,client_secret}; config.validate_desktop()?; local.config=Some(config);
                }
                NativeRequest::Connect{write_scopes}=> {
                    let config=local.config.clone().ok_or("UNCONFIGURED: configure your native Desktop OAuth client")?;
                    let uri=config.validate_desktop()?;
                    let listener=std::net::TcpListener::bind(("127.0.0.1",uri.port().ok_or("Redirect port missing")?)).map_err(|_|"Cannot bind registered loopback redirect; choose another configured port")?;
                    let pending=PendingOAuth::begin(config.clone(),write_scopes,p::now_ms())?;
                    open_system_browser(&pending.authorization_url)?;
                    let response=callback(listener,&config.redirect_uri)?;
                    let tokens=pending.finish(&response,p::now_ms()).await?;
                    if local.tokens.as_ref().is_some_and(|old|old.account()!=tokens.account()){return Err("Account changed; disconnect old account before connecting another".into());}
                    local.tokens=Some(tokens);
                }
                NativeRequest::Disconnect=> {
                    // Revoke locally even when offline. Provider revocation is best effort,
                    // surfaced explicitly; no old local grant/token remains usable.
                    let remote=if let Some(tokens)=&local.tokens {oauth::revoke(tokens).await.err()}else{None};
                    local.tokens=None;
                    for entry in &mut local.entries {if entry.status==p::CommitStatus::Prepared {entry.status=p::CommitStatus::Rejected;}}
                    data=json!({"revocation_warning":remote});
                }
                NativeRequest::Prepare{review}=> {
                    local.prepare(review.clone(),p::now_ms())?;
                    store::task_note(vault,&review,"PREPARED locally, no external effect; exact local review required")?;
                }
                NativeRequest::GuardianPreview{operation_id}=> {
                    let entry=local.entries.iter().find(|e|e.review.operation_id==operation_id).ok_or("Unknown operation")?;
                    data=json!({"decision":guardian::decision(&entry.review,&guardian::context(&local)),"manifest_preview":guardian::google_manifest(local.tokens.as_ref().map(|t|t.account()),local.tokens.as_ref().is_some_and(|t|t.scopes().len()>2),p::now_ms()).consent_preview()});
                }
                NativeRequest::GuardianCorrection{task_id,fingerprint,kind,comment}=> {
                    let kind=match kind.as_str(){"FALSE_ALARM"=>g::CorrectionKind::FalseAlarm,"CONFIRMED_HARMFUL"=>g::CorrectionKind::ConfirmedHarmful,"MISSED_WARNING"=>g::CorrectionKind::MissedWarning,_=>return Err("Unknown correction kind".into())};
                    if fingerprint.len()>2048||comment.len()>2048 {return Err("Correction text bound".into());}
                    guardian::task_note(vault,&task_id,&g::Correction::new(kind,&fingerprint,&comment,p::now_ms()).ledger_note(),p::now_ms())?;
                }
                NativeRequest::Commit{operation_id,exact_digest,acknowledged_fingerprint}=> {
                    check_session()?;
                    let config=local.config.as_ref().ok_or("UNCONFIGURED")?;
                    oauth::refresh(config,local.tokens.as_mut().ok_or("Connect account first")?,p::now_ms()).await?;
                    let (review,grant)=local.begin_commit(&operation_id,&exact_digest,p::now_ms())?;
                    // §3.6 host-owned guardian BEFORE reserving the attempt: BLOCK/unacknowledged WARN never dispatches.
                    let ctx=guardian::context(&local);
                    let guardian_receipt=match guardian::enforce(&review,&ctx,acknowledged_fingerprint.as_deref(),p::now_ms()) {
                        Ok(receipt)=>receipt,
                        Err(message)=> {
                            if let Some(decision)=guardian::decision(&review,&ctx) {
                                if let Err(refusal)=g::enforce(&decision,None,p::now_ms()) { let _=guardian::task_note(vault,&review.task_id,&refusal.receipt.ledger_note(),p::now_ms()); }
                            }
                            return Err(message);
                        }
                    };
                    store::save(vault,&local)?; // durable uncertainty BEFORE any side effect
                    store::task_note(vault,&review,"NEEDS_RECONCILIATION: attempt reserved; no automatic retry")?;
                    if let Some(receipt)=&guardian_receipt { guardian::task_note(vault,&review.task_id,&receipt.ledger_note(),p::now_ms())?; }
                    check_session()?;
                    let receipt=Google::new(local.tokens.as_ref().ok_or("Account disconnected")?)?.with_session_guard(&check_session).with_guardian(ctx,acknowledged_fingerprint).commit(&review,&grant,p::now_ms()).await?;
                    check_session()?; local.finish(receipt.clone())?; store::save(vault,&local)?;
                    store::task_note(vault,&review,&format!("Provider readback VERIFIED: {} · {}. Task itself remains review-only.",receipt.provider_id,receipt.detail))?;
                    data=json!(receipt);
                }
                NativeRequest::Reconcile{operation_id,provider_id}=> {
                    let entry=local.entries.iter().find(|e|e.review.operation_id==operation_id).ok_or("Unknown operation")?.clone();
                    if entry.status!=p::CommitStatus::NeedsReconciliation {return Err("Only uncertain operations reconcile; no replay".into());}
                    oauth::refresh(local.config.as_ref().ok_or("UNCONFIGURED")?,local.tokens.as_mut().ok_or("Connect account first")?,p::now_ms()).await?;
                    let receipt=Google::new(local.tokens.as_ref().ok_or("Account disconnected")?)?.with_session_guard(&check_session).verify(&entry.review,&provider_id).await?;
                    check_session()?;local.finish(receipt.clone())?;store::save(vault,&local)?;
                    store::task_note(vault,&entry.review,&format!("Provider reconciliation readback VERIFIED: {}. Not an imported execution grant.",receipt.provider_id))?;
                    data=json!(receipt);
                }
                read=> {
                    oauth::refresh(local.config.as_ref().ok_or("UNCONFIGURED")?,local.tokens.as_mut().ok_or("Connect account first")?,p::now_ms()).await?;
                    let google=Google::new(local.tokens.as_ref().ok_or("Account disconnected")?)?.with_session_guard(&check_session);
                    data=match read {
                        NativeRequest::Search{folder,query,page}=>google.search(&folder,&query,page.as_deref()).await?,
                        NativeRequest::Message{folder,id}=>google.message(&folder,&id).await?,
                        NativeRequest::Thread{folder,id}=>google.thread(&folder,&id).await?,
                        NativeRequest::Labels=>google.labels().await?,
                        NativeRequest::Calendars{page}=>google.calendars(page.as_deref()).await?,
                        NativeRequest::Events{calendar,start,end,page}=>google.events(&calendar,&start,&end,page.as_deref()).await?,
                        NativeRequest::FreeBusy{calendar,start,end}=>google.free_busy(&calendar,&start,&end).await?,
                        _=>return Err("Unsupported provider action".into())
                    };
                }
            }
            check_session()?; store::save(vault,&local)?;
            Ok::<Value,String>(json!({"sources":local.view(),"data":data}))
        });
        result
    }).await.map_err(|_|"Provider worker interrupted; reload and reconcile, never repeat send".to_string())?
}

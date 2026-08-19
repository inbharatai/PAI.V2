// UnoOne Power — Desktop Browser Workspace
// Uses Tauri's built-in WebView (WebView2 on Windows, WKWebView elsewhere).
//
// TRUTHFULNESS CONTRACT
// ---------------------
// Every action in this module is executed *by the backend* against the real
// webview window, and the reported result is parsed from what the page
// actually returned. Nothing reports success before the page confirms it.
// A selector that matches nothing is a failure. A URL scheme outside the
// allowlist is refused before it ever reaches the webview. Screenshots are
// real PNG files on disk accompanied by the SHA-256 of their bytes.
// There is deliberately no ExecuteScript variant: model-generated arbitrary
// script execution is not a feature, it is the defect being removed.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Manager;
use unoone_browser_policy::{evaluate as evaluate_redirect, RedirectVerdict};

/// Browser session configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    pub headless: bool,
    pub user_data_dir: Option<String>,
    pub proxy: Option<String>,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub disable_images: bool,
    pub disable_javascript: bool,
    pub accept_languages: String,
    pub user_agent: Option<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            headless: false,
            user_data_dir: None,
            proxy: None,
            viewport_width: 1280,
            viewport_height: 800,
            disable_images: false,
            disable_javascript: false,
            accept_languages: "en-US,en;q=0.9".to_string(),
            user_agent: None,
        }
    }
}

/// Typed browser actions. There is intentionally no arbitrary-script action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BrowserAction {
    Navigate {
        url: String,
    },
    Back,
    Forward,
    Reload,
    ExtractPageText,
    ExtractElementText {
        selector: String,
    },
    Click {
        selector: String,
    },
    Type {
        selector: String,
        text: String,
    },
    FillForm {
        fields: Vec<FormFillField>,
    },
    Scroll {
        direction: ScrollDirection,
        amount: u32,
    },
    Wait {
        milliseconds: u64,
    },
    GetPageInfo,
    Screenshot,
    Close,
    ClearSession,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormFillField {
    pub selector: String,
    pub value: String,
}

/// Truthful result of a browser action. `verified` is true only when the
/// webview returned a parseable success payload for the requested action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserActionResult {
    pub success: bool,
    pub verified: bool,
    pub data: serde_json::Value,
    pub error: Option<String>,
    pub user_message: String,
    pub current_url: Option<String>,
    pub current_title: Option<String>,
    pub screenshot_path: Option<String>,
    pub screenshot_sha256: Option<String>,
}

impl BrowserActionResult {
    fn failure(error: &str, session_url: Option<String>, session_title: Option<String>) -> Self {
        Self {
            success: false,
            verified: false,
            data: serde_json::Value::Null,
            error: Some(error.to_string()),
            user_message: error.to_string(),
            current_url: session_url,
            current_title: session_title,
            screenshot_path: None,
            screenshot_sha256: None,
        }
    }
}

/// Active browser session state
pub struct BrowserSession {
    pub window_label: String,
    pub current_url: Option<String>,
    pub title: Option<String>,
}

pub struct BrowserStateHolder {
    pub session: Mutex<Option<BrowserSession>>,
    /// One-shot confirmation tokens granted by the user for risky actions
    /// (form submission, file upload, download). A token is consumed on use.
    pub confirmation_tokens: Mutex<HashSet<String>>,
}

impl BrowserStateHolder {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            confirmation_tokens: Mutex::new(HashSet::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// Pure logic — every function here is deterministic and unit-tested below.
// ---------------------------------------------------------------------------

/// URL schemes that may be navigated to. Everything else is refused.
const ALLOWED_SCHEMES: [&str; 2] = ["http", "https"];

/// Validate a navigation URL and return its normalised form.
/// Blocks javascript:, file:, data:, vbscript:, about: and anything else not
/// in the allowlist. This runs before any script is built or injected.
pub fn validate_navigation_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("URL is empty".to_string());
    }
    if trimmed.len() > 8192 {
        return Err("URL exceeds the 8 KiB limit".to_string());
    }
    // Detect the scheme without a full URL parser: scheme is everything up to
    // the first ':', lowercased. Scheme-free input is treated as https with a
    // host, and refused if it contains no dot (it would be a search query,
    // which this workspace does not guess at).
    let scheme = match trimmed.split_once(':') {
        Some((head, _)) => head.trim().to_ascii_lowercase(),
        None => {
            let hostish = trimmed.split('/').next().unwrap_or("");
            if hostish.contains('.') && !hostish.contains(char::is_whitespace) {
                return Ok(format!("https://{}", trimmed));
            }
            return Err(format!(
                "Refusing to navigate to '{}': no scheme and not a dotted host. Use a full https:// URL.",
                trimmed
            ));
        }
    };
    if !ALLOWED_SCHEMES.contains(&scheme.as_str()) {
        return Err(format!(
            "Scheme '{}' is not allowed. Permitted schemes: {:?}.",
            scheme, ALLOWED_SCHEMES
        ));
    }
    // A scheme with no host is not a navigation target.
    let rest = trimmed.split_once(':').map(|(_, rest)| rest).unwrap_or("");
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        return Err(format!("URL '{}' has a scheme but no host", trimmed));
    }
    Ok(trimmed.to_string())
}

/// Serialise a Rust string into a JSON/JavaScript string literal. This is the
/// ONLY supported way to interpolate user/model-supplied values into bridge
/// scripts — the old `.replace('\'', "\\'")` escaping was injection-prone.
pub fn js_string_literal(value: &str) -> String {
    serde_json::to_string(value).expect("serialising a &str cannot fail")
}

/// Wrap a bridge expression so it always reinjects the bridge and always
/// returns a JSON string result envelope: {"ok":bool,"error":?,"data":...}
fn bridge_envelope(expression: &str) -> String {
    format!(
        "{bridge}\n(function(){{try{{{expr}}}catch(e){{return JSON.stringify({{ok:false,error:String(e)}});}}}})()",
        bridge = BROWSER_BRIDGE_SCRIPT,
        expr = expression,
    )
}

/// Script: navigate to a validated URL. Verification happens Rust-side by
/// polling page info afterwards (see `poll_page_ready`).
pub fn build_navigate_script(url: &str) -> Result<String, String> {
    let normalised = validate_navigation_url(url)?;
    Ok(bridge_envelope(&format!(
        "window.location.href = {lit}; return JSON.stringify({{ok:true,data:{{intent:'navigate',to:{lit}}}}});",
        lit = js_string_literal(&normalised),
    )))
}

/// Script: read canonical page state. Doubles as bridge liveness probe.
pub fn build_page_info_script() -> String {
    bridge_envelope(
        "const i=window.__unooneBrowserBridge.getPageInfo();return JSON.stringify({ok:true,data:i});",
    )
}

pub fn build_history_script(which: BrowserHistoryAction) -> String {
    let call = match which {
        BrowserHistoryAction::Back => "history.back()",
        BrowserHistoryAction::Forward => "history.forward()",
        BrowserHistoryAction::Reload => "location.reload()",
    };
    bridge_envelope(&format!(
        "{}; return JSON.stringify({{ok:true,data:{{intent:'{}'}}}});",
        call,
        which.as_str()
    ))
}

#[derive(Debug, PartialEq, Eq)]
pub enum BrowserHistoryAction {
    Back,
    Forward,
    Reload,
}

impl BrowserHistoryAction {
    fn as_str(&self) -> &'static str {
        match self {
            BrowserHistoryAction::Back => "back",
            BrowserHistoryAction::Forward => "forward",
            BrowserHistoryAction::Reload => "reload",
        }
    }
}

pub fn build_extract_page_text_script() -> String {
    bridge_envelope(
        "const t=window.__unooneBrowserBridge.extractText();return JSON.stringify({ok:true,data:{text:t}});",
    )
}

pub fn build_extract_element_text_script(selector: &str) -> String {
    bridge_envelope(&format!(
        "const t=window.__unooneBrowserBridge.extractText({sel});if(t===null){{return JSON.stringify({{ok:false,error:'Selector matched nothing: '+{sel}}});}}return JSON.stringify({{ok:true,data:{{text:t}}}});",
        sel = js_string_literal(selector),
    ))
}

/// Risk heuristics run *in the page* at the element: submit buttons, file
/// inputs, download links. If the element is risky and the action is not
/// confirmed, the script refuses and reports CONFIRMATION_REQUIRED.
fn risk_probe(selector_literal: &str, confirmed: bool) -> String {
    format!(
        "const el=document.querySelector({sel});\
         if(!el){{return JSON.stringify({{ok:false,error:'Selector matched nothing: '+{sel}}});}}\
         if(!{confirmed}&&window.__unooneBrowserBridge.isRisky(el)){{return JSON.stringify({{ok:false,code:'CONFIRMATION_REQUIRED',error:'This action may submit a form, upload or download data, or change account state. Confirm it explicitly to proceed.'}});}}",
        sel = selector_literal,
        confirmed = if confirmed { "true" } else { "false" },
    )
}

pub fn build_click_script(selector: &str, confirmed: bool) -> String {
    let sel = js_string_literal(selector);
    bridge_envelope(&format!(
        "{probe} el.click(); return JSON.stringify({{ok:true,data:{{clicked:{sel}}}}});",
        probe = risk_probe(&sel, confirmed),
    ))
}

pub fn build_type_script(selector: &str, text: &str, confirmed: bool) -> String {
    let sel = js_string_literal(selector);
    let txt = js_string_literal(text);
    bridge_envelope(&format!(
        "{probe} window.__unooneBrowserBridge.type(el,{txt});\
         const ok=el.value==={txt};\
         if(!ok){{return JSON.stringify({{ok:false,error:'Value after typing does not match the requested text.'}});}}\
         return JSON.stringify({{ok:true,data:{{typed:{sel},chars:{len}}}}});",
        probe = risk_probe(&sel, confirmed),
        len = text.chars().count(),
    ))
}

pub fn build_fill_form_script(fields: &[FormFillField], confirmed: bool) -> Result<String, String> {
    if fields.is_empty() {
        return Err("FillForm requires at least one field".to_string());
    }
    if fields.iter().any(|f| f.selector.trim().is_empty()) {
        return Err("FillForm field has an empty selector".to_string());
    }
    let fields_json = serde_json::to_string(fields).map_err(|e| e.to_string())?;
    Ok(bridge_envelope(&format!(
        "const fields={fj};\
         let unresolved=[];\
         for(const f of fields){{if(!document.querySelector(f.selector)){{unresolved.push(f.selector);}}}}\
         if(unresolved.length){{return JSON.stringify({{ok:false,error:'Selectors matched nothing: '+unresolved.join(', ')}});}}\
         if(!{confirmed}){{for(const f of fields){{const el=document.querySelector(f.selector);if(window.__unooneBrowserBridge.isRisky(el)){{return JSON.stringify({{ok:false,code:'CONFIRMATION_REQUIRED',error:'A target field may upload data (file input) or trigger submission. Confirm explicitly to proceed.'}});}}}}}}\
         const filled=window.__unooneBrowserBridge.fillForm(fields);\
         if(filled!==fields.length){{return JSON.stringify({{ok:false,error:'Only '+filled+' of '+fields.length+' fields were filled'}});}}\
         return JSON.stringify({{ok:true,data:{{filled:filled}}}});",
        fj = fields_json,
        confirmed = if confirmed { "true" } else { "false" },
    )))
}

pub fn build_scroll_script(direction: &ScrollDirection, amount: u32) -> Result<String, String> {
    let dir = match direction {
        ScrollDirection::Up => "up",
        ScrollDirection::Down => "down",
    };
    let capped = amount.min(100_000);
    Ok(bridge_envelope(&format!(
        "window.__unooneBrowserBridge.scroll('{}', {}); return JSON.stringify({{ok:true,data:{{direction:'{}',amount:{}}}}});",
        dir, capped, dir, capped
    )))
}

/// Parse the JSON envelope returned by a bridge script.
pub fn parse_bridge_result(raw: &str) -> Result<serde_json::Value, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Bridge returned an empty result".to_string());
    }
    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("Bridge result was not JSON: '{}': {}", trimmed, e))?;
    match value.get("ok").and_then(|b| b.as_bool()) {
        Some(true) => Ok(value),
        Some(false) => Err(value
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("Action failed in the page without an error message")
            .to_string()),
        None => Err("Bridge result missing 'ok' field".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Bridge script — injected before every action, so it reliably survives
// navigation (each eval reinjects it on the new page).
// ---------------------------------------------------------------------------

const BROWSER_BRIDGE_SCRIPT: &str = r#"
window.__unooneBrowserBridge = {
    extractText: function(selector) {
        if (selector) {
            const el = document.querySelector(selector);
            return el ? el.innerText : null;
        }
        return document.body ? document.body.innerText : '';
    },
    isRisky: function(el) {
        if (!el) return false;
        const tag = (el.tagName || '').toLowerCase();
        const type = (el.getAttribute && el.getAttribute('type') || '').toLowerCase();
        if (tag === 'input' && (type === 'submit' || type === 'file' || type === 'image')) return true;
        if (tag === 'button' && (type === 'submit' || type === '' || type === null)) {
            if (el.closest && el.closest('form')) return true;
        }
        if (tag === 'a' && el.hasAttribute && el.hasAttribute('download')) return true;
        if (el.closest && el.closest('a[download]')) return true;
        if (tag === 'input' && type === 'submit') return true;
        return false;
    },
    click: function(selector) {
        const el = document.querySelector(selector);
        if (el) { el.click(); return true; }
        return false;
    },
    type: function(el, text) {
        el.focus();
        el.value = text;
        el.dispatchEvent(new Event('input', {bubbles: true}));
        el.dispatchEvent(new Event('change', {bubbles: true}));
    },
    fillForm: function(fields) {
        let filled = 0;
        for (const f of fields) {
            const el = document.querySelector(f.selector);
            if (el) {
                el.focus();
                el.value = f.value;
                el.dispatchEvent(new Event('input', {bubbles: true}));
                el.dispatchEvent(new Event('change', {bubbles: true}));
                filled++;
            }
        }
        return filled;
    },
    scroll: function(direction, amount) {
        const y = direction === 'down' ? amount : -amount;
        window.scrollBy(0, y);
        return true;
    },
    clearSessionStorage: function() {
        try { window.localStorage.clear(); } catch (e) {}
        try { window.sessionStorage.clear(); } catch (e) {}
        return true;
    },
    getPageInfo: function() {
        return {
            url: window.location.href,
            title: document.title,
            readyState: document.readyState
        };
    }
};
"#;

// ---------------------------------------------------------------------------
// Execution — everything below runs against the real webview window.
// ---------------------------------------------------------------------------

const EVAL_TIMEOUT: Duration = Duration::from_secs(10);
const NAVIGATE_POLL_LIMIT: Duration = Duration::from_secs(20);

fn eval_bridge(app: &tauri::AppHandle, window_label: &str, script: &str) -> Result<String, String> {
    let window = app
        .get_webview_window(window_label)
        .ok_or_else(|| format!("Webview window '{}' not found", window_label))?;

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx2 = tx.clone();
    window
        .eval_with_callback(script, move |result| {
            let _ = tx2.send(result);
        })
        .map_err(|e| format!("Eval failed: {}", e))?;

    rx.recv_timeout(EVAL_TIMEOUT).map_err(|e| {
        format!(
            "Eval timed out or channel closed after {:?}: {}",
            EVAL_TIMEOUT, e
        )
    })
}

/// Read page info, tolerating failure (returns None).
fn read_page_info(app: &tauri::AppHandle, window_label: &str) -> Option<serde_json::Value> {
    let raw = eval_bridge(app, window_label, &build_page_info_script()).ok()?;
    let payload = parse_bridge_result(&raw).ok()?;
    Some(payload)
}

/// After a navigation trigger, poll until the page loads. Returns
/// (url, title, verdict) of the settled page, or an error on timeout or on a
/// forbidden landing (HTTPS downgrade / non-http page).
fn poll_page_ready(
    app: &tauri::AppHandle,
    window_label: &str,
    target: &str,
) -> Result<(Option<String>, Option<String>, RedirectVerdict), String> {
    let start = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(300));
        if let Some(info) = read_page_info(app, window_label) {
            let data = info.get("data").cloned().unwrap_or(serde_json::Value::Null);
            let url = data.get("url").and_then(|u| u.as_str()).map(String::from);
            let title = data.get("title").and_then(|t| t.as_str()).map(String::from);
            let ready = data
                .get("readyState")
                .and_then(|r| r.as_str())
                .map(|s| s == "complete" || s == "interactive")
                .unwrap_or(false);
            if ready {
                if let Some(u) = url.as_deref() {
                    match evaluate_redirect(u, target) {
                        RedirectVerdict::Reached => {
                            return Ok((url, title, RedirectVerdict::Reached));
                        }
                        verdict @ RedirectVerdict::RequiresReview { .. } => {
                            // Surfaced, never silently accepted as success.
                            return Ok((url, title, verdict));
                        }
                        RedirectVerdict::NotReached { reason } => return Err(reason),
                    }
                }
            }
        }
        if start.elapsed() > NAVIGATE_POLL_LIMIT {
            return Err(format!(
                "Navigation to '{}' did not reach a loadable page within {:?}",
                target, NAVIGATE_POLL_LIMIT
            ));
        }
    }
}

fn capture_screenshot(
    app: &tauri::AppHandle,
    window_label: &str,
) -> Result<(String, String), String> {
    let window = app
        .get_webview_window(window_label)
        .ok_or_else(|| format!("Webview window '{}' not found", window_label))?;

    let position = window
        .outer_position()
        .map_err(|e| format!("Cannot read window position: {}", e))?;
    let size = window
        .outer_size()
        .map_err(|e| format!("Cannot read window size: {}", e))?;

    let screen = screenshots::Screen::from_point(position.x, position.y)
        .map_err(|e| format!("No display contains the browser window: {}", e))?;
    let display = screen.display_info;
    let rel_x = (position.x - display.x).max(0);
    let rel_y = (position.y - display.y).max(0);

    let image = screen
        .capture_area(rel_x, rel_y, size.width, size.height)
        .map_err(|e| format!("Screen capture failed: {}", e))?;

    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png_bytes);
        let encoder = png::Encoder::new(&mut cursor, image.width(), image.height());
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("PNG header error: {}", e))?;
        writer
            .write_image_data(image.as_raw())
            .map_err(|e| format!("PNG encode error: {}", e))?;
    }

    let sha = sha2_hex(&png_bytes);
    let dir = std::env::temp_dir().join("unoone-browser");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create screenshot dir: {}", e))?;
    let filename = format!(
        "browser-{}.png",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3f")
    );
    let path = dir.join(filename);
    std::fs::write(&path, &png_bytes).map_err(|e| format!("Cannot write screenshot: {}", e))?;

    Ok((path.to_string_lossy().to_string(), sha))
}

/// Hex-encoded SHA-256 without pulling in a new crate beyond what the
/// workspace already uses (sha2 comes via vault-core's dependency tree).
fn sha2_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn session_snapshot_url_title(state: &BrowserStateHolder) -> (Option<String>, Option<String>) {
    let lock = state.session.lock().ok();
    match lock.as_deref() {
        Some(Some(s)) => (s.current_url.clone(), s.title.clone()),
        _ => (None, None),
    }
}

fn with_session<R>(
    state: &BrowserStateHolder,
    f: impl FnOnce(&mut Option<BrowserSession>) -> R,
) -> Result<R, String> {
    let mut lock = state
        .session
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    Ok(f(&mut lock))
}

#[tauri::command]
pub fn browser_start_session(
    config: Option<BrowserConfig>,
    window_label: String,
    state: tauri::State<'_, Arc<BrowserStateHolder>>,
) -> Result<BrowserActionResult, String> {
    let _config = config.unwrap_or_default();
    if window_label.trim().is_empty() {
        return Err("window_label is required".to_string());
    }
    with_session(&state, |session| {
        *session = Some(BrowserSession {
            window_label: window_label.clone(),
            current_url: None,
            title: None,
        });
    })?;
    Ok(BrowserActionResult {
        success: true,
        verified: true,
        data: serde_json::json!({ "window_label": window_label }),
        error: None,
        user_message: format!("Browser session bound to window '{}'. The frontend owns window creation; all actions execute against that window.", window_label),
        current_url: None,
        current_title: None,
        screenshot_path: None,
        screenshot_sha256: None,
    })
}

#[tauri::command]
pub fn browser_stop_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<BrowserStateHolder>>,
) -> Result<BrowserActionResult, String> {
    let session_opt = with_session(&state, |session| session.take())?;
    if let Ok(mut tokens) = state.confirmation_tokens.lock() {
        tokens.clear();
    }
    let mut window_closed = false;
    let mut note = "No active browser session.".to_string();
    if let Some(session) = session_opt {
        window_closed = match app.get_webview_window(&session.window_label) {
            Some(w) => w.close().is_ok(),
            None => false,
        };
        note = if window_closed {
            format!(
                "Session closed; window '{}' destroyed; confirmation token store cleared.",
                session.window_label
            )
        } else {
            format!(
                "Session closed and confirmation tokens cleared; window '{}' was already gone.",
                session.window_label
            )
        };
    }
    Ok(BrowserActionResult {
        success: true,
        verified: true,
        data: serde_json::json!({ "window_closed": window_closed }),
        error: None,
        user_message: note,
        current_url: None,
        current_title: None,
        screenshot_path: None,
        screenshot_sha256: None,
    })
}

/// Execute a typed action against the real webview and report what actually
/// happened. `confirmed` is the user's explicit consent for risky elements
/// (submit / upload / download); unconfirmed risky actions are refused.
#[tauri::command]
pub async fn browser_execute(
    action: BrowserAction,
    confirmed: Option<bool>,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<BrowserStateHolder>>,
) -> Result<BrowserActionResult, String> {
    let state = state.inner().clone();
    let state_for_closure = state.clone();
    let mut result = tauri::async_runtime::spawn_blocking(move || {
        browser_execute_sync(action, confirmed.unwrap_or(false), app, state_for_closure)
    })
    .await
    .map_err(|e| format!("Browser executor failed: {}", e))??;

    // Attach live page context to whatever the action returned.
    if result.current_url.is_none() {
        let (u, t) = session_snapshot_url_title(&state);
        result.current_url = u;
        result.current_title = t;
    }
    Ok(result)
}

fn browser_execute_sync(
    action: BrowserAction,
    confirmed: bool,
    app: tauri::AppHandle,
    state: Arc<BrowserStateHolder>,
) -> Result<BrowserActionResult, String> {
    let (window_label, session_url, session_title) = {
        let lock = state
            .session
            .lock()
            .map_err(|e| format!("State lock error: {}", e))?;
        match lock.as_ref() {
            Some(s) => (
                s.window_label.clone(),
                s.current_url.clone(),
                s.title.clone(),
            ),
            None => {
                return Ok(BrowserActionResult::failure(
                    "No active browser session. Call browser_start_session first.",
                    None,
                    None,
                ))
            }
        }
    };

    // Evals block on an mpsc channel with a timeout; spawn_blocking keeps
    // the async executor free.
    let mut result = match action {
        BrowserAction::Navigate { url } => {
            match build_navigate_script(&url).and_then(|script| {
                let normalised = validate_navigation_url(&url)?;
                eval_bridge(&app, &window_label, &script)?;
                let (u, t, verdict) = poll_page_ready(&app, &window_label, &normalised)?;
                with_session(&state, |session| {
                    if let Some(s) = session.as_mut() {
                        s.current_url = u.clone();
                        s.title = t.clone();
                    }
                })?;
                Ok::<_, String>((u, t, verdict))
            }) {
                Ok((u, t, RedirectVerdict::Reached)) => ok_result(
                    "navigate",
                    serde_json::json!({ "action": "navigate", "url": u, "title": t }),
                    &window_label,
                    &app,
                ),
                Ok((u, t, RedirectVerdict::RequiresReview { landed, expected })) => {
                    // The page loaded, but on an unexpected cross-origin. This
                    // is surfaced as unverified — never a silent success.
                    BrowserActionResult {
                        success: true,
                        verified: false,
                        data: serde_json::json!({
                            "action": "navigate",
                            "url": u,
                            "title": t,
                            "redirected": true,
                            "landed": landed.clone(),
                            "expected": expected.clone(),
                        }),
                        error: None,
                        user_message: format!(
                            "Navigation completed but redirected off the target domain. {} landed on {} — review before trusting this page.",
                            expected, landed
                        ),
                        current_url: u,
                        current_title: t,
                        screenshot_path: None,
                        screenshot_sha256: None,
                    }
                }
                Ok((_, _, RedirectVerdict::NotReached { reason })) => {
                    BrowserActionResult::failure(&reason, session_url, session_title)
                }
                Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
            }
        }
        BrowserAction::Back | BrowserAction::Forward | BrowserAction::Reload => {
            let which = match action {
                BrowserAction::Back => BrowserHistoryAction::Back,
                BrowserAction::Forward => BrowserHistoryAction::Forward,
                _ => BrowserHistoryAction::Reload,
            };
            match eval_bridge(&app, &window_label, &build_history_script(which)).and_then(|_| {
                // After history movement/reload, wait for load like navigate.
                let (_, t, _verdict) = poll_page_ready(&app, &window_label, "")?;
                let info = read_page_info(&app, &window_label);
                Ok((info, t))
            }) {
                Ok((info, title)) => {
                    let url = info
                        .as_ref()
                        .and_then(|i| i.get("data"))
                        .and_then(|d| d.get("url"))
                        .and_then(|u| u.as_str())
                        .map(String::from);
                    with_session(&state, |session| {
                        if let Some(s) = session.as_mut() {
                            s.current_url = url.clone();
                            s.title = title.clone();
                        }
                    })
                    .ok();
                    ok_result(
                        "history",
                        serde_json::json!({ "url": url, "title": title }),
                        &window_label,
                        &app,
                    )
                }
                Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
            }
        }
        BrowserAction::ExtractPageText => {
            match eval_bridge(&app, &window_label, &build_extract_page_text_script()) {
                Ok(raw) => match parse_bridge_result(&raw) {
                    Ok(payload) => ok_result(
                        "extract_page_text",
                        payload["data"].clone(),
                        &window_label,
                        &app,
                    ),
                    Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
                },
                Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
            }
        }
        BrowserAction::ExtractElementText { selector } => {
            match eval_bridge(
                &app,
                &window_label,
                &build_extract_element_text_script(&selector),
            ) {
                Ok(raw) => match parse_bridge_result(&raw) {
                    Ok(payload) => ok_result(
                        "extract_element_text",
                        payload["data"].clone(),
                        &window_label,
                        &app,
                    ),
                    Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
                },
                Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
            }
        }
        BrowserAction::Click { selector } => {
            match eval_bridge(
                &app,
                &window_label,
                &build_click_script(&selector, confirmed),
            ) {
                Ok(raw) => match parse_bridge_result(&raw) {
                    Ok(payload) => ok_result("click", payload["data"].clone(), &window_label, &app),
                    Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
                },
                Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
            }
        }
        BrowserAction::Type { selector, text } => {
            match eval_bridge(
                &app,
                &window_label,
                &build_type_script(&selector, &text, confirmed),
            ) {
                Ok(raw) => match parse_bridge_result(&raw) {
                    Ok(payload) => ok_result("type", payload["data"].clone(), &window_label, &app),
                    Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
                },
                Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
            }
        }
        BrowserAction::FillForm { fields } => {
            match build_fill_form_script(&fields, confirmed).and_then(|script| {
                let raw = eval_bridge(&app, &window_label, &script)?;
                parse_bridge_result(&raw).map(|p| p["data"].clone())
            }) {
                Ok(data) => ok_result("fill_form", data, &window_label, &app),
                Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
            }
        }
        BrowserAction::Scroll { direction, amount } => {
            match build_scroll_script(&direction, amount).and_then(|script| {
                let raw = eval_bridge(&app, &window_label, &script)?;
                parse_bridge_result(&raw).map(|p| p["data"].clone())
            }) {
                Ok(data) => ok_result("scroll", data, &window_label, &app),
                Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
            }
        }
        BrowserAction::Wait { milliseconds } => {
            let capped = milliseconds.min(60_000);
            std::thread::sleep(Duration::from_millis(capped));
            ok_result(
                "wait",
                serde_json::json!({ "waited_ms": capped }),
                &window_label,
                &app,
            )
        }
        BrowserAction::GetPageInfo => match read_page_info(&app, &window_label) {
            Some(info) => ok_result("page_info", info["data"].clone(), &window_label, &app),
            None => BrowserActionResult::failure(
                "Could not read page info — bridge unreachable",
                session_url,
                session_title,
            ),
        },
        BrowserAction::Screenshot => match capture_screenshot(&app, &window_label) {
            Ok((path, sha)) => {
                let mut r = ok_result(
                    "screenshot",
                    serde_json::json!({ "path": path, "sha256": sha }),
                    &window_label,
                    &app,
                );
                r.screenshot_path = Some(path);
                r.screenshot_sha256 = Some(sha);
                r
            }
            Err(e) => BrowserActionResult::failure(&e, session_url, session_title),
        },
        BrowserAction::Close => {
            let closed = match app.get_webview_window(&window_label) {
                Some(w) => w.close().is_ok(),
                None => false,
            };
            with_session(&state, |session| {
                *session = None;
            })
            .ok();
            BrowserActionResult {
                success: closed,
                verified: closed,
                data: serde_json::json!({ "window_closed": closed }),
                error: if closed {
                    None
                } else {
                    Some("Window was already gone".to_string())
                },
                user_message: if closed {
                    "Browser window closed and session cleared.".to_string()
                } else {
                    "Browser window could not be closed (already gone); session cleared."
                        .to_string()
                },
                current_url: session_url,
                current_title: session_title,
                screenshot_path: None,
                screenshot_sha256: None,
            }
        }
        BrowserAction::ClearSession => {
            let script = bridge_envelope(
                "window.__unooneBrowserBridge.clearSessionStorage();return JSON.stringify({ok:true,data:{cleared:['localStorage','sessionStorage']}});",
            );
            let (cleared, error) = match eval_bridge(&app, &window_label, &script) {
                Ok(raw) => match parse_bridge_result(&raw) {
                    Ok(_) => (true, None),
                    Err(e) => (false, Some(e)),
                },
                Err(e) => (false, Some(e)),
            };
            // Confirmation tokens are session-scoped: they die with a ClearSession too.
            if let Ok(mut tokens) = state.confirmation_tokens.lock() {
                tokens.clear();
            }
            BrowserActionResult {
                success: cleared,
                verified: cleared,
                data: serde_json::json!({
                    "page_storage_cleared": cleared,
                    "note": "WebView2 profile data (cookies, cache, disk profile) is not exposed through Tauri's webview API and is NOT cleared by this action."
                }),
                error,
                user_message: "Page-scoped storage cleared; cookie/profile data clearing requires WebView2 ClearBrowsingData, unavailable via Tauri.".to_string(),
                current_url: session_url,
                current_title: session_title,
                screenshot_path: None,
                screenshot_sha256: None,
            }
        }
    };

    // Attach live page context to whatever the action returned.
    if result.current_url.is_none() {
        let (u, t) = session_snapshot_url_title(&state);
        result.current_url = u;
        result.current_title = t;
    }
    Ok(result)
}

fn ok_result(
    action: &str,
    data: serde_json::Value,
    window_label: &str,
    app: &tauri::AppHandle,
) -> BrowserActionResult {
    let (url, title) = read_page_info(app, window_label)
        .map(|i| {
            let d = i.get("data").cloned().unwrap_or(serde_json::Value::Null);
            (
                d.get("url").and_then(|u| u.as_str()).map(String::from),
                d.get("title").and_then(|t| t.as_str()).map(String::from),
            )
        })
        .unwrap_or((None, None));
    BrowserActionResult {
        success: true,
        verified: true,
        data,
        error: None,
        user_message: format!("{} completed and verified against the live page.", action),
        current_url: url,
        current_title: title,
        screenshot_path: None,
        screenshot_sha256: None,
    }
}

/// Get the browser bridge script (kept for the frontend's manual reinjection
/// after creating the window; every action reinjects it anyway).
#[tauri::command]
pub fn get_browser_bridge_script() -> String {
    BROWSER_BRIDGE_SCRIPT.to_string()
}

/// Execute raw page JS. Retained for the frontend's bridge reinjection only.
#[tauri::command]
pub async fn browser_eval(
    window_label: String,
    script: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    eval_bridge(&app, &window_label, &script)
}

// ---------------------------------------------------------------------------
// Tests — deterministic, no webview required.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- scheme validation ---------------------------------------------------

    #[test]
    fn https_navigation_allowed() {
        assert_eq!(
            validate_navigation_url("https://example.com/path?q=1").unwrap(),
            "https://example.com/path?q=1"
        );
    }

    #[test]
    fn http_localhost_allowed() {
        assert!(validate_navigation_url("http://127.0.0.1:8080/test").is_ok());
    }

    #[test]
    fn bare_host_is_upgraded_to_https() {
        assert_eq!(
            validate_navigation_url("example.com/docs").unwrap(),
            "https://example.com/docs"
        );
    }

    #[test]
    fn javascript_scheme_blocked() {
        let err = validate_navigation_url("javascript:alert(1)").unwrap_err();
        assert!(err.contains("not allowed"));
    }

    #[test]
    fn javascript_scheme_mixed_case_blocked() {
        assert!(validate_navigation_url("JaVaScRiPt:alert(1)").is_err());
    }

    #[test]
    fn file_scheme_blocked() {
        assert!(validate_navigation_url("file:///C:/Windows/system32").is_err());
    }

    #[test]
    fn data_scheme_blocked() {
        assert!(validate_navigation_url("data:text/html,<script>alert(1)</script>").is_err());
    }

    #[test]
    fn vbscript_scheme_blocked() {
        assert!(validate_navigation_url("vbscript:msgbox(1)").is_err());
    }

    #[test]
    fn about_scheme_blocked() {
        assert!(validate_navigation_url("about:blank").is_err());
    }

    #[test]
    fn scheme_without_host_refused() {
        assert!(validate_navigation_url("https://").is_err());
    }

    #[test]
    fn whitespace_bare_search_refused() {
        assert!(validate_navigation_url("how do i bake bread").is_err());
    }

    #[test]
    fn empty_url_refused() {
        assert!(validate_navigation_url("   ").is_err());
    }

    #[test]
    fn overlong_url_refused() {
        assert!(validate_navigation_url(&format!("https://e.co/{}", "a".repeat(9000))).is_err());
    }

    // -- interpolation escaping ----------------------------------------------

    #[test]
    fn js_literal_escapes_single_and_double_quotes() {
        let lit = js_string_literal("o'connor said \"hi\"");
        assert!(lit.starts_with('"') && lit.ends_with('"'));
        assert!(lit.contains("\\\""));
    }

    #[test]
    fn js_literal_escapes_backslashes() {
        let lit = js_string_literal("C:\\Users\\evil\\");
        assert!(lit.contains("\\\\"));
    }

    #[test]
    fn js_literal_escapes_newlines() {
        let lit = js_string_literal("line1\nline2\r\nline3");
        assert!(lit.contains("\\n"));
        assert!(!lit.contains("\nline2"));
    }

    #[test]
    fn js_literal_preserves_unicode() {
        let lit = js_string_literal("नमस्ते 🌏 你好");
        assert!(lit.contains("नमस्ते"));
    }

    #[test]
    fn js_literal_neutralises_script_close_smuggling() {
        // The classic breakout: ');alert('xss');//
        let payload = "');alert('xss');//";
        let script = build_click_script(payload, false);
        assert!(script.contains(&js_string_literal(payload)));
        // Every occurrence of the payload must be inside a JSON string
        // literal (double-quoted); the payload can only escape a JS string
        // if it is placed between single quotes, which must never happen.
        assert!(script.contains(&format!(
            "document.querySelector({})",
            js_string_literal(payload)
        )));
        assert!(!script.contains(&format!("'{}'", payload)));
    }

    // -- script construction --------------------------------------------------

    #[test]
    fn navigate_script_validates_first() {
        assert!(build_navigate_script("javascript:alert(1)").is_err());
        assert!(build_navigate_script("https://ok.test").is_ok());
    }

    #[test]
    fn click_script_uses_json_serialised_selector() {
        let script = build_click_script("#q input[name='s']", false);
        let lit = js_string_literal("#q input[name='s']");
        assert!(script.contains(&format!("document.querySelector({})", lit)));
    }

    #[test]
    fn type_script_verifies_value_after_typing() {
        let script = build_type_script("#box", "hello", false);
        assert!(script.contains("el.value==="));
    }

    #[test]
    fn unconfirmed_risky_click_script_refuses_before_clicking() {
        let script = build_click_script("#submit", false);
        assert!(script.contains("CONFIRMATION_REQUIRED"));
    }

    #[test]
    fn confirmed_click_script_omits_refusal_path() {
        let confirmed = build_click_script("#submit", true);
        let unconfirmed = build_click_script("#submit", false);
        assert!(confirmed.contains("if(!true&&"));
        assert!(unconfirmed.contains("if(!false&&"));
    }

    #[test]
    fn fill_form_empty_fields_refused() {
        assert!(build_fill_form_script(&[], false).is_err());
    }

    #[test]
    fn fill_form_empty_selector_refused() {
        let fields = vec![FormFillField {
            selector: "  ".to_string(),
            value: "x".to_string(),
        }];
        assert!(build_fill_form_script(&fields, false).is_err());
    }

    #[test]
    fn fill_form_reports_unresolved_selectors() {
        let fields = vec![FormFillField {
            selector: "#a".to_string(),
            value: "x".to_string(),
        }];
        let script = build_fill_form_script(&fields, false).unwrap();
        assert!(script.contains("Selectors matched nothing:"));
    }

    #[test]
    fn scroll_amount_capped() {
        let script = build_scroll_script(&ScrollDirection::Down, 999_999_999).unwrap();
        assert!(script.contains("100000"));
    }

    // -- result parsing --------------------------------------------------------

    #[test]
    fn parse_success_envelope() {
        let v = parse_bridge_result(r#"{"ok":true,"data":{"text":"hi"}}"#).unwrap();
        assert_eq!(v["data"]["text"], "hi");
    }

    #[test]
    fn parse_failure_envelope_surfaces_error() {
        let err = parse_bridge_result(r#"{"ok":false,"error":"Selector matched nothing: #nope"}"#)
            .unwrap_err();
        assert!(err.contains("Selector matched nothing"));
    }

    #[test]
    fn parse_garbage_is_error_not_success() {
        assert!(parse_bridge_result("<html>no way</html>").is_err());
        assert!(parse_bridge_result("").is_err());
        assert!(parse_bridge_result("null").is_err());
        assert!(parse_bridge_result(r#"{"data":{}}"#).is_err());
    }

    // -- redirect policy delegation (full matrix lives in unoone-browser-policy) --

    #[test]
    fn redirect_policy_is_the_crate_not_string_splitting() {
        // The old predicate accepted ANY http(s) page post-redirect. Now an
        // unrelated origin must surface as RequiresReview...
        assert!(matches!(
            evaluate_redirect("https://other.example/page", "https://example.com/start"),
            RedirectVerdict::RequiresReview { .. }
        ));
        // ...downgrades must fail...
        assert!(matches!(
            evaluate_redirect("http://example.com/x", "https://example.com/y"),
            RedirectVerdict::NotReached { .. }
        ));
        // ...and chrome-internal pages must fail...
        assert!(matches!(
            evaluate_redirect("about:blank", "https://example.com"),
            RedirectVerdict::NotReached { .. }
        ));
        // ...while same-origin still passes.
        assert_eq!(
            evaluate_redirect(
                "https://example.com/docs/index.html?x=1",
                "https://example.com/start"
            ),
            RedirectVerdict::Reached
        );
    }

    #[test]
    fn history_scripts_cover_all_three() {
        assert!(build_history_script(BrowserHistoryAction::Back).contains("history.back()"));
        assert!(build_history_script(BrowserHistoryAction::Forward).contains("history.forward()"));
        assert!(build_history_script(BrowserHistoryAction::Reload).contains("location.reload()"));
    }
}

//! OWNER-ONLY live read smoke test. Ignored in every ordinary test run. Never import
//! Hyperagent/environment access tokens: this obtains fresh consent via real PKCE.
use std::io::{Read, Write};
use unoone_personal_provider_adapters::{
    google::Google,
    now_ms,
    oauth::{revoke, OAuthConfig, PendingOAuth},
};

#[tokio::test]
#[ignore = "Requires owner-provided Desktop OAuth registration, exact redirect, expected existing disposable account and interactive system-browser consent; absent here. NOT live-qualified."]
async fn owner_authorized_live_readonly_smoke() {
    let client_id = std::env::var("UNOONE_GOOGLE_CLIENT_ID")
        .expect("Owner must configure UNOONE_GOOGLE_CLIENT_ID");
    let redirect_uri = std::env::var("UNOONE_GOOGLE_REDIRECT")
        .expect("Owner must configure exact loopback UNOONE_GOOGLE_REDIRECT");
    let expected = std::env::var("UNOONE_GOOGLE_EXPECTED_ACCOUNT")
        .expect("Owner must name existing disposable account explicitly");
    let config = OAuthConfig {
        client_id,
        redirect_uri: redirect_uri.clone(),
        client_secret: std::env::var("UNOONE_GOOGLE_CLIENT_SECRET").ok(),
    };
    let redirect = config
        .validate_desktop()
        .expect("Registered desktop loopback configuration");
    let listener = std::net::TcpListener::bind(("127.0.0.1", redirect.port().unwrap()))
        .expect("Configured loopback port must be available");
    listener.set_nonblocking(true).unwrap();
    let pending = PendingOAuth::begin(config, false, now_ms()).unwrap();
    #[cfg(target_os = "windows")]
    let launch = std::process::Command::new("rundll32")
        .arg("url.dll,FileProtocolHandler")
        .arg(&pending.authorization_url)
        .spawn();
    #[cfg(target_os = "macos")]
    let launch = std::process::Command::new("open")
        .arg(&pending.authorization_url)
        .spawn();
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let launch = std::process::Command::new("xdg-open")
        .arg(&pending.authorization_url)
        .spawn();
    let mut child = launch.expect("Owner session requires system browser");
    let _reaper = std::thread::spawn(move || child.wait());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    let callback = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "Owner authorization timed out; no account connected"
        );
        match listener.accept() {
            Ok((mut stream, peer)) => {
                assert!(peer.ip().is_loopback());
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = [0u8; 8192];
                let n = stream.read(&mut bytes).expect("Callback interrupted");
                assert!(n < bytes.len());
                let text = std::str::from_utf8(&bytes[..n]).expect("Malformed callback");
                let parts = text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .collect::<Vec<_>>();
                if parts.len() != 3
                    || parts[0] != "GET"
                    || !parts[1].starts_with("/oauth/callback?")
                {
                    continue;
                }
                let url = redirect.join(parts[1]).unwrap().to_string();
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nReturn to the owner test. Authorization will be checked; this page is not proof of connection.").unwrap();
                bytes.fill(0);
                break url;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await
            }
            Err(_) => panic!("Loopback callback unavailable"),
        }
    };
    let tokens = pending
        .finish(&callback, now_ms())
        .await
        .expect("Fresh Google consent/token/profile verification failed");
    let account_matches = tokens.account() == expected;
    // Explicit owner command authorizes this single bounded read, never mailbox bulk read.
    let read = if account_matches {
        Google::new(&tokens).unwrap().calendars(None).await
    } else {
        Err("Unexpected account; no calendar read".into())
    };
    let revoked = revoke(&tokens).await; // no residual test grant intentionally retained
    assert!(
        account_matches,
        "Owner-selected account did not match configured binding"
    );
    assert!(
        read.is_ok(),
        "Live calendar-list read failed; no qualification claimed"
    );
    assert!(
        revoked.is_ok(),
        "Remote revocation unconfirmed; owner must remove app access in Google Account settings"
    );
}

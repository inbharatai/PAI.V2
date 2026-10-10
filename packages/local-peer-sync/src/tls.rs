//! Standard TLS1.3 mutual proof-of-private-key with exact out-of-band certificate pins.
//! No TOFU, DNS, proxy, public address, redirects, TLS1.2, custom cryptography or test trust.
use crate::{hash, require, Exchange, Reply, Result, State, MAX_BODY};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider},
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    ClientConfig, ClientConnection, DigitallySignedStruct, DistinguishedName, ServerConfig,
    ServerConnection, SignatureScheme, StreamOwned,
};
use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    sync::Arc,
    time::{Duration, Instant},
};

pub fn identity() -> Result<(Vec<u8>, Vec<u8>)> {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["unoone.local".into()])
            .map_err(|_| "Local certificate creation failed")?;
    Ok((cert.der().to_vec(), key_pair.serialize_der()))
}
#[derive(Debug)]
struct Pin {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
    hints: Vec<DistinguishedName>,
}
impl Pin {
    fn matches(
        &self,
        cert: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
    ) -> std::result::Result<(), rustls::Error> {
        if !intermediates.is_empty() || cert.len() > 8192 || hash(cert.as_ref()) != self.fingerprint
        {
            Err(rustls::Error::General("Unapproved peer certificate".into()))
        } else {
            Ok(())
        }
    }
}
impl ServerCertVerifier for Pin {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        self.matches(cert, intermediates)?;
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
impl ClientCertVerifier for Pin {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &self.hints
    }
    fn verify_client_cert(
        &self,
        cert: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        self.matches(cert, intermediates)?;
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        ServerCertVerifier::verify_tls12_signature(self, message, cert, dss)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        ServerCertVerifier::verify_tls13_signature(self, message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        ServerCertVerifier::supported_verify_schemes(self)
    }
}
fn pin(state: &State, provider: Arc<CryptoProvider>) -> Result<Arc<Pin>> {
    Ok(Arc::new(Pin {
        fingerprint: state.active()?.offer.fingerprint.clone(),
        provider,
        hints: vec![],
    }))
}
pub fn server_config(state: &State) -> Result<Arc<ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| "TLS configuration")?
        .with_client_cert_verifier(pin(state, provider)?)
        .with_single_cert(
            vec![CertificateDer::from(state.certificate.clone())],
            PrivatePkcs8KeyDer::from(state.private_key.clone()).into(),
        )
        .map_err(|_| "TLS identity")?;
    config.max_early_data_size = 0;
    config.send_tls13_tickets = 0;
    Ok(Arc::new(config))
}
pub fn client_config(state: &State) -> Result<Arc<ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| "TLS configuration")?
        .dangerous()
        .with_custom_certificate_verifier(pin(state, provider)?)
        .with_client_auth_cert(
            vec![CertificateDer::from(state.certificate.clone())],
            PrivatePkcs8KeyDer::from(state.private_key.clone()).into(),
        )
        .map_err(|_| "TLS identity")?;
    config.enable_early_data = false;
    config.resumption = rustls::client::Resumption::disabled();
    Ok(Arc::new(config))
}
pub fn local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
        IpAddr::V6(v) => v.is_loopback() || v.is_unique_local() || v.is_unicast_link_local(),
    }
}
pub fn address(value: &str) -> Result<SocketAddr> {
    require(value.len() <= 80, "Address length")?;
    let a: SocketAddr = value
        .parse()
        .map_err(|_| "Use a numeric LAN IP:port (no hostname)")?;
    require(
        local_ip(a.ip()) && !a.ip().is_unspecified() && a.port() > 0,
        "Only loopback/private LAN addresses are allowed",
    )?;
    Ok(a)
}
pub type SessionGuard = Arc<dyn Fn() -> bool + Send + Sync>;
struct Deadline {
    socket: TcpStream,
    end: Instant,
    guard: SessionGuard,
}
impl Deadline {
    fn new(socket: TcpStream, guard: SessionGuard) -> Result<Self> {
        require(
            local_ip(socket.peer_addr().map_err(|_| "Peer address")?.ip()),
            "Nonlocal peer rejected",
        )?;
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|_| "Socket timeout")?;
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|_| "Socket timeout")?;
        Ok(Self {
            socket,
            end: Instant::now() + Duration::from_secs(15),
            guard,
        })
    }
    fn check(&self) -> std::io::Result<()> {
        if !(self.guard)() || Instant::now() >= self.end {
            Err(std::io::ErrorKind::TimedOut.into())
        } else {
            Ok(())
        }
    }
}
impl Read for Deadline {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.check()?;
        self.socket.read(b)
    }
}
impl Write for Deadline {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.check()?;
        self.socket.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check()?;
        self.socket.flush()
    }
}
// Deliberately small closed HTTP/1.1 subset. No chunked encoding, compression or keepalive.
pub fn read_http(r: &mut impl Read, response: bool) -> Result<Vec<u8>> {
    let mut headers = Vec::new();
    loop {
        require(headers.len() < 2048, "HTTP header bound")?;
        let mut byte = [0];
        r.read_exact(&mut byte)
            .map_err(|_| "Truncated/timed-out TLS stream")?;
        headers.push(byte[0]);
        if headers.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = std::str::from_utf8(&headers).map_err(|_| "Invalid HTTP header")?;
    let mut lines = text.split("\r\n");
    require(
        lines.next()
            == Some(if response {
                "HTTP/1.1 200 OK"
            } else {
                "POST /unoone-peer-v1 HTTP/1.1"
            }),
        "Unsupported HTTP endpoint/status",
    )?;
    let mut length = None;
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, val) = line.split_once(':').ok_or("Invalid HTTP header")?;
        match name.to_ascii_lowercase().as_str() {
            "content-length" => {
                require(length.is_none(), "Duplicate length")?;
                length = Some(val.trim().parse::<usize>().map_err(|_| "Invalid length")?);
            }
            "host" | "content-type" | "connection" => (),
            _ => return Err("Unsupported HTTP header".into()),
        }
    }
    let size = length.ok_or("Missing length")?;
    require(size > 0 && size <= MAX_BODY, "HTTP body bound")?;
    let mut body = vec![0; size];
    r.read_exact(&mut body)
        .map_err(|_| "Truncated/timed-out body; no ACK")?;
    Ok(body)
}
pub fn write_http(w: &mut impl Write, body: &[u8], response: bool) -> Result<()> {
    require(
        !body.is_empty() && body.len() <= MAX_BODY,
        "HTTP body bound",
    )?;
    let start = if response {
        "HTTP/1.1 200 OK"
    } else {
        "POST /unoone-peer-v1 HTTP/1.1"
    };
    write!(w, "{start}\r\nHost: unoone.local\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).map_err(|_| "TLS write failed")?;
    w.write_all(body)
        .and_then(|_| w.flush())
        .map_err(|_| "TLS write failed".into())
}
pub fn serve_socket(
    socket: TcpStream,
    state: &State,
    persist_and_reply: impl FnOnce(Exchange) -> Result<Reply>,
) -> Result<()> {
    serve_guarded(socket, state, Arc::new(|| true), persist_and_reply)
}
pub fn serve_guarded(
    socket: TcpStream,
    state: &State,
    guard: SessionGuard,
    persist_and_reply: impl FnOnce(Exchange) -> Result<Reply>,
) -> Result<()> {
    let mut tls = StreamOwned::new(
        ServerConnection::new(server_config(state)?).map_err(|_| "TLS setup")?,
        Deadline::new(socket, guard)?,
    );
    let bytes = read_http(&mut tls, false)?;
    crate::json_guard::preflight(&bytes, MAX_BODY)?;
    let request: Exchange = serde_json::from_slice(&bytes).map_err(|_| "Invalid sync request")?;
    let reply = persist_and_reply(request)?; // ALL durable receive state before ACK.
    write_http(
        &mut tls,
        &serde_json::to_vec(&reply).map_err(|_| "Reply encoding")?,
        true,
    )?;
    tls.conn.send_close_notify();
    tls.flush().map_err(|_| "TLS close failed".into())
}
pub fn connect(address_text: &str, state: &State, request: &Exchange) -> Result<Reply> {
    connect_guarded(address_text, state, request, Arc::new(|| true))
}
pub fn connect_guarded(
    address_text: &str,
    state: &State,
    request: &Exchange,
    guard: SessionGuard,
) -> Result<Reply> {
    require(guard(), "Session closed")?;
    let socket = TcpStream::connect_timeout(&address(address_text)?, Duration::from_secs(3))
        .map_err(|_| "LAN peer unreachable")?;
    let mut tls = StreamOwned::new(
        ClientConnection::new(
            client_config(state)?,
            ServerName::try_from("unoone.local").map_err(|_| "TLS name")?,
        )
        .map_err(|_| "TLS setup")?,
        Deadline::new(socket, guard)?,
    );
    write_http(
        &mut tls,
        &serde_json::to_vec(request).map_err(|_| "Request encoding")?,
        false,
    )?;
    let bytes = read_http(&mut tls, true)?;
    crate::json_guard::preflight(&bytes, MAX_BODY)?;
    let reply = serde_json::from_slice(&bytes).map_err(|_| "Invalid sync reply")?;
    tls.conn.send_close_notify();
    let _ = tls.flush();
    Ok(reply)
}
pub fn listen_once(
    address_text: &str,
    state: &State,
    still_unlocked: SessionGuard,
    persist_and_reply: impl FnOnce(Exchange) -> Result<Reply>,
) -> Result<()> {
    state.active()?;
    let listener = TcpListener::bind(address(address_text)?)
        .map_err(|_| "Cannot bind that local interface/port")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "Listener configuration")?;
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        require(
            still_unlocked(),
            "Vault locked/session closed; listener cancelled",
        )?;
        require(
            Instant::now() < end,
            "No peer within 30 seconds; listener closed",
        )?;
        match listener.accept() {
            Ok((socket, _)) => {
                return serve_guarded(socket, state, still_unlocked, persist_and_reply)
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(_) => return Err("Listener failed".into()),
        }
    }
}

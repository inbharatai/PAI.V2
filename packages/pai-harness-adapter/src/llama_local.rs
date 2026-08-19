//! Synchronous, localhost-only Harness model provider for Pocket AI's verified
//! llama-server process.
//!
//! The desktop host must call this provider from a blocking worker. Model-file
//! identity, process ownership and package hashes remain the responsibility of
//! the existing Pocket AI model manager before a port is handed to this type.

use inbharat_harness_core::providers::FinishReason;
use inbharat_harness_core::{
    CancellationToken, ErrorCode, Failure, FailureClass, HarnessResult, ModelChunk, ModelProvider,
    ModelRequest, ModelResponse,
};
use serde_json::{json, Value as JsonValue};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug)]
pub struct PaiLlamaLocalProvider {
    provider_id: String,
    model_id: String,
    port: u16,
    timeout: Duration,
}

impl PaiLlamaLocalProvider {
    pub fn new(model_id: impl Into<String>, port: u16) -> HarnessResult<Self> {
        let model_id = model_id.into();
        if model_id.trim().is_empty() || model_id.len() > 256 || port == 0 {
            return Err(Failure::invalid(
                "pai.model.new",
                "model id or localhost port is invalid",
            ));
        }
        Ok(Self {
            provider_id: "pai-llama-local".to_owned(),
            model_id,
            port,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        if !timeout.is_zero() && timeout <= Duration::from_secs(600) {
            self.timeout = timeout;
        }
        self
    }

    fn failure(operation: &'static str, message: impl Into<String>) -> Failure {
        Failure::new(
            ErrorCode::ProviderFailed,
            FailureClass::Provider,
            operation,
            message,
        )
    }

    fn role(role: inbharat_harness_core::providers::ModelRole) -> &'static str {
        match role {
            inbharat_harness_core::providers::ModelRole::System => "system",
            inbharat_harness_core::providers::ModelRole::User => "user",
            inbharat_harness_core::providers::ModelRole::Assistant => "assistant",
            inbharat_harness_core::providers::ModelRole::Tool => "tool",
        }
    }
}

impl ModelProvider for PaiLlamaLocalProvider {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn models(&self) -> Vec<String> {
        vec![self.model_id.clone()]
    }

    fn stream(
        &self,
        request: &ModelRequest,
        cancel: &CancellationToken,
        sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    ) -> HarnessResult<ModelResponse> {
        cancel.check("pai.model.local")?;
        if request.model != self.model_id || request.provider != self.provider_id {
            return Err(Failure::new(
                ErrorCode::CapabilityUnavailable,
                FailureClass::Provider,
                "pai.model.local",
                "request does not match the bound local model provider",
            ));
        }

        let mut messages = Vec::with_capacity(request.messages.len().saturating_add(1));
        if !request.system.trim().is_empty() {
            messages.push(json!({"role": "system", "content": request.system}));
        }
        for message in &request.messages {
            messages.push(json!({
                "role": Self::role(message.role),
                "content": message.content,
            }));
        }

        let tools = request
            .tools
            .iter()
            .map(|tool| {
                let schema: JsonValue =
                    serde_json::from_str(&tool.input_schema).map_err(|error| {
                        Failure::invalid(
                            "pai.model.tool_schema",
                            format!("invalid Harness tool schema for {}: {error}", tool.id),
                        )
                    })?;
                Ok(json!({
                    "type": "function",
                    "function": {
                        "name": tool.id,
                        "description": tool.description,
                        "parameters": schema,
                    }
                }))
            })
            .collect::<HarnessResult<Vec<_>>>()?;

        // llama.cpp accepts token budgets rather than bytes. Keep this bounded
        // conservatively and let the Harness enforce the exact byte ceiling.
        let max_tokens = (request.max_output_bytes / 4).clamp(64, 8192);
        let mut body = json!({
            "model": self.model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": 0.7,
            "stream": false,
            "chat_template_kwargs": { "enable_thinking": false },
            "reasoning_budget": 0,
        });
        if !tools.is_empty() {
            body["tools"] = JsonValue::Array(tools);
            body["tool_choice"] = json!("auto");
        }

        let request_bytes = serde_json::to_vec(&body)
            .map_err(|error| Self::failure("pai.model.encode", error.to_string()))?;
        let http = post_json_localhost(self.port, &request_bytes, self.timeout, cancel)?;
        if !(200..300).contains(&http.status) {
            let detail = String::from_utf8_lossy(&http.body);
            return Err(Self::failure(
                "pai.model.response",
                format!(
                    "local llama-server returned HTTP {}: {}",
                    http.status,
                    bound(&detail, 512)
                ),
            ));
        }

        let payload: JsonValue = serde_json::from_slice(&http.body)
            .map_err(|error| Self::failure("pai.model.decode", error.to_string()))?;
        cancel.check("pai.model.local")?;

        let choice = payload
            .get("choices")
            .and_then(JsonValue::as_array)
            .and_then(|choices| choices.first())
            .ok_or_else(|| Self::failure("pai.model.decode", "llama-server returned no choices"))?;
        let message = choice
            .get("message")
            .ok_or_else(|| Self::failure("pai.model.decode", "llama-server returned no message"))?;
        let text = message
            .get("content")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_owned();
        if text.len() > request.max_output_bytes {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Resource,
                "pai.model.output",
                "model output exceeded the Harness byte budget",
            ));
        }

        sink(ModelChunk::Start { block: 0 })?;
        if !text.is_empty() {
            sink(ModelChunk::TextDelta {
                block: 0,
                text: text.clone(),
            })?;
        }

        let mut emitted_tool_calls = 0usize;
        if let Some(calls) = message.get("tool_calls").and_then(JsonValue::as_array) {
            for (index, call) in calls.iter().enumerate() {
                cancel.check("pai.model.tool_calls")?;
                let function = call.get("function").unwrap_or(&JsonValue::Null);
                let tool_id = function
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                if tool_id.is_empty() {
                    continue;
                }
                let arguments = match function.get("arguments") {
                    Some(JsonValue::String(value)) => value.clone(),
                    Some(value) => serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned()),
                    None => "{}".to_owned(),
                };
                let call_id = call
                    .get("id")
                    .and_then(JsonValue::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("call-{index}"));
                sink(ModelChunk::ToolCall {
                    block: u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX),
                    call_id,
                    tool_id: tool_id.to_owned(),
                    arguments,
                })?;
                emitted_tool_calls = emitted_tool_calls.saturating_add(1);
            }
        }
        sink(ModelChunk::End { block: 0 })?;

        let usage = payload.get("usage").unwrap_or(&JsonValue::Null);
        let input_units = usage
            .get("prompt_tokens")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0);
        let output_units = usage
            .get("completion_tokens")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0);
        sink(ModelChunk::Usage {
            input_units,
            output_units,
        })?;

        let raw_finish = choice
            .get("finish_reason")
            .and_then(JsonValue::as_str)
            .unwrap_or("stop");
        let finish = if emitted_tool_calls > 0 || raw_finish == "tool_calls" {
            FinishReason::ToolCalls
        } else if raw_finish == "length" {
            FinishReason::Length
        } else {
            FinishReason::Stop
        };
        sink(ModelChunk::Finish { reason: finish })?;

        Ok(ModelResponse {
            text,
            finish,
            input_units,
            output_units,
            provider_request_id: payload
                .get("id")
                .and_then(JsonValue::as_str)
                .map(str::to_owned),
        })
    }
}

const HTTP_IO_POLL: Duration = Duration::from_millis(150);
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_HTTP_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

struct LocalHttpResponse {
    status: u16,
    body: Vec<u8>,
}

fn post_json_localhost(
    port: u16,
    body: &[u8],
    timeout: Duration,
    cancel: &CancellationToken,
) -> HarnessResult<LocalHttpResponse> {
    cancel.check("pai.model.connect")?;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream =
        TcpStream::connect_timeout(&address, HTTP_CONNECT_TIMEOUT).map_err(|error| {
            PaiLlamaLocalProvider::failure(
                "pai.model.connect",
                format!("could not connect to verified local llama-server: {error}"),
            )
            .retryable(Some(250))
        })?;
    stream
        .set_read_timeout(Some(HTTP_IO_POLL))
        .map_err(|error| PaiLlamaLocalProvider::failure("pai.model.socket", error.to_string()))?;
    stream
        .set_write_timeout(Some(HTTP_IO_POLL))
        .map_err(|error| PaiLlamaLocalProvider::failure("pai.model.socket", error.to_string()))?;
    stream.set_nodelay(true).ok();

    let header = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let deadline = Instant::now() + timeout;
    write_cancelable(&mut stream, header.as_bytes(), deadline, cancel)?;
    write_cancelable(&mut stream, body, deadline, cancel)?;
    stream
        .flush()
        .map_err(|error| PaiLlamaLocalProvider::failure("pai.model.write", error.to_string()))?;

    let response = read_http_response(&mut stream, deadline, cancel);
    if response.is_err() {
        let _ = stream.shutdown(Shutdown::Both);
    }
    response
}

fn write_cancelable(
    stream: &mut TcpStream,
    bytes: &[u8],
    deadline: Instant,
    cancel: &CancellationToken,
) -> HarnessResult<()> {
    let mut offset = 0usize;
    while offset < bytes.len() {
        if let Err(failure) = cancel.check("pai.model.write") {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(failure);
        }
        if Instant::now() >= deadline {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(Failure::new(
                ErrorCode::Timeout,
                FailureClass::Resource,
                "pai.model.write",
                "local model request exceeded its deadline",
            ));
        }
        match stream.write(&bytes[offset..]) {
            Ok(0) => {
                return Err(PaiLlamaLocalProvider::failure(
                    "pai.model.write",
                    "local llama-server closed while receiving the request",
                ));
            }
            Ok(count) => offset = offset.saturating_add(count),
            Err(error) if is_poll_timeout(&error) => continue,
            Err(error) => {
                return Err(PaiLlamaLocalProvider::failure(
                    "pai.model.write",
                    error.to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn read_http_response(
    stream: &mut TcpStream,
    deadline: Instant,
    cancel: &CancellationToken,
) -> HarnessResult<LocalHttpResponse> {
    let mut raw = Vec::with_capacity(64 * 1024);
    let mut scratch = [0u8; 16 * 1024];
    let mut parsed_headers: Option<(usize, u16, Option<usize>, bool)> = None;

    loop {
        if let Err(failure) = cancel.check("pai.model.read") {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(failure);
        }
        if Instant::now() >= deadline {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(Failure::new(
                ErrorCode::Timeout,
                FailureClass::Resource,
                "pai.model.read",
                "local model generation exceeded its deadline",
            ));
        }

        match stream.read(&mut scratch) {
            Ok(0) => break,
            Ok(count) => {
                if raw.len().saturating_add(count) > MAX_HTTP_RESPONSE_BYTES {
                    let _ = stream.shutdown(Shutdown::Both);
                    return Err(Failure::new(
                        ErrorCode::BudgetExceeded,
                        FailureClass::Resource,
                        "pai.model.read",
                        "local llama-server HTTP response exceeded 16 MiB",
                    ));
                }
                raw.extend_from_slice(&scratch[..count]);
                if parsed_headers.is_none() {
                    if let Some(header_end) = find_subslice(&raw, b"\r\n\r\n") {
                        parsed_headers = Some(
                            parse_headers(&raw[..header_end])?.with_body_start(header_end + 4),
                        );
                    }
                }
                if let Some((body_start, _status, content_length, chunked)) = parsed_headers {
                    let body = &raw[body_start..];
                    if let Some(expected) = content_length {
                        if body.len() >= expected {
                            break;
                        }
                    } else if chunked && chunked_message_complete(body) {
                        break;
                    }
                }
            }
            Err(error) if is_poll_timeout(&error) => continue,
            Err(error) => {
                return Err(PaiLlamaLocalProvider::failure(
                    "pai.model.read",
                    error.to_string(),
                ));
            }
        }
    }

    let (body_start, status, content_length, chunked) = parsed_headers.ok_or_else(|| {
        PaiLlamaLocalProvider::failure(
            "pai.model.http",
            "local llama-server returned no HTTP headers",
        )
    })?;
    let mut body = raw[body_start..].to_vec();
    if let Some(expected) = content_length {
        if body.len() < expected {
            return Err(PaiLlamaLocalProvider::failure(
                "pai.model.http",
                "local llama-server closed before the declared response body was complete",
            ));
        }
        body.truncate(expected);
    } else if chunked {
        body = decode_chunked(&body)?;
    }
    Ok(LocalHttpResponse { status, body })
}

trait ParsedHeaderExt {
    fn with_body_start(self, body_start: usize) -> (usize, u16, Option<usize>, bool);
}

impl ParsedHeaderExt for (u16, Option<usize>, bool) {
    fn with_body_start(self, body_start: usize) -> (usize, u16, Option<usize>, bool) {
        (body_start, self.0, self.1, self.2)
    }
}

fn parse_headers(header_bytes: &[u8]) -> HarnessResult<(u16, Option<usize>, bool)> {
    let text = std::str::from_utf8(header_bytes).map_err(|_| {
        PaiLlamaLocalProvider::failure(
            "pai.model.http",
            "local llama-server returned non-UTF8 HTTP headers",
        )
    })?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut status_parts = status_line.split_whitespace();
    let protocol = status_parts.next().unwrap_or_default();
    let status = status_parts
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| (100..=599).contains(value))
        .ok_or_else(|| {
            PaiLlamaLocalProvider::failure(
                "pai.model.http",
                "invalid HTTP status from local llama-server",
            )
        })?;
    if !protocol.starts_with("HTTP/1.") {
        return Err(PaiLlamaLocalProvider::failure(
            "pai.model.http",
            "unsupported HTTP protocol from local llama-server",
        ));
    }
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "content-length" {
            let parsed = value.parse::<usize>().map_err(|_| {
                PaiLlamaLocalProvider::failure(
                    "pai.model.http",
                    "invalid Content-Length from local llama-server",
                )
            })?;
            if parsed > MAX_HTTP_RESPONSE_BYTES {
                return Err(Failure::new(
                    ErrorCode::BudgetExceeded,
                    FailureClass::Resource,
                    "pai.model.http",
                    "local llama-server declared a response larger than 16 MiB",
                ));
            }
            content_length = Some(parsed);
        } else if name == "transfer-encoding"
            && value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("chunked"))
        {
            chunked = true;
        }
    }
    if content_length.is_some() && chunked {
        // RFC 7230: Transfer-Encoding overrides Content-Length. Reject the
        // ambiguous combination instead of interpreting a smuggled response.
        return Err(PaiLlamaLocalProvider::failure(
            "pai.model.http",
            "ambiguous HTTP framing from local llama-server",
        ));
    }
    Ok((status, content_length, chunked))
}

fn chunked_message_complete(body: &[u8]) -> bool {
    decode_chunked(body).is_ok()
}

fn decode_chunked(body: &[u8]) -> HarnessResult<Vec<u8>> {
    let mut cursor = 0usize;
    let mut decoded = Vec::new();
    loop {
        let Some(line_end_rel) = find_subslice(&body[cursor..], b"\r\n") else {
            return Err(PaiLlamaLocalProvider::failure(
                "pai.model.http",
                "incomplete chunk header",
            ));
        };
        let line_end = cursor + line_end_rel;
        let line = std::str::from_utf8(&body[cursor..line_end]).map_err(|_| {
            PaiLlamaLocalProvider::failure("pai.model.http", "non-UTF8 chunk header")
        })?;
        let size_token = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_token, 16)
            .map_err(|_| PaiLlamaLocalProvider::failure("pai.model.http", "invalid chunk size"))?;
        cursor = line_end + 2;
        if size == 0 {
            // Accept either the normal empty trailer (CRLF) or a trailer block.
            if body.len() < cursor + 2 {
                return Err(PaiLlamaLocalProvider::failure(
                    "pai.model.http",
                    "incomplete final chunk",
                ));
            }
            return Ok(decoded);
        }
        if size > MAX_HTTP_RESPONSE_BYTES
            || decoded.len().saturating_add(size) > MAX_HTTP_RESPONSE_BYTES
        {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Resource,
                "pai.model.http",
                "chunked response exceeded 16 MiB",
            ));
        }
        let data_end = cursor.checked_add(size).ok_or_else(|| {
            PaiLlamaLocalProvider::failure("pai.model.http", "chunk size overflow")
        })?;
        if body.len() < data_end + 2 || &body[data_end..data_end + 2] != b"\r\n" {
            return Err(PaiLlamaLocalProvider::failure(
                "pai.model.http",
                "incomplete chunk data",
            ));
        }
        decoded.extend_from_slice(&body[cursor..data_end]);
        cursor = data_end + 2;
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn is_poll_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

fn bound(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_owned();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

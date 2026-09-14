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
    /// Attachment bytes (base64) keyed by the harness `AttachmentMetadata`
    /// id. The harness core carries metadata only; the desktop bridge hands
    /// the actual image bytes to this provider so vision requests stay
    /// local and the audit trail records digests, not pixels.
    attachments: std::collections::BTreeMap<String, (String, String)>,
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
            attachments: std::collections::BTreeMap::new(),
        })
    }

    /// Attach image bytes (base64) under one harness attachment id. The
    /// media type must be an image type llama.cpp accepts in an
    /// `image_url` part (png/jpeg/webp/gif).
    pub fn with_attachment(
        mut self,
        id: impl Into<String>,
        media_type: impl Into<String>,
        base64_bytes: impl Into<String>,
    ) -> Self {
        self.attachments
            .insert(id.into(), (media_type.into(), base64_bytes.into()));
        self
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
}

/// Parses the Harness tool-result transcript format
/// `tool={tool_id} call={call_id} result={content}`. Tool ids and call ids
/// never contain spaces, so the first ` call=` and ` result=` separators are
/// authoritative even when the result content repeats either marker.
fn parse_tool_transcript(content: &str) -> Option<(String, String, String)> {
    let rest = content.strip_prefix("tool=")?;
    let (tool_id, rest) = rest.split_once(" call=")?;
    let (call_id, result) = rest.split_once(" result=")?;
    if tool_id.is_empty() || call_id.is_empty() {
        return None;
    }
    Some((tool_id.to_owned(), call_id.to_owned(), result.to_owned()))
}

fn role_name(role: inbharat_harness_core::providers::ModelRole) -> &'static str {
    match role {
        inbharat_harness_core::providers::ModelRole::System => "system",
        inbharat_harness_core::providers::ModelRole::User => "user",
        inbharat_harness_core::providers::ModelRole::Assistant => "assistant",
        inbharat_harness_core::providers::ModelRole::Tool => "tool",
    }
}

/// Builds the OpenAI-protocol message array for one llama-server request:
/// the system prompt, the Harness transcript with tool exchanges
/// reconstructed into canonical assistant-tool_calls + tool-result pairs,
/// and (when attachments are present) the last user message rendered as
/// multimodal parts. Attachment bytes live in `local_attachments`, keyed by
/// the harness attachment id; ids without local bytes fail closed rather
/// than silently sending a text-only request the model would answer as if
/// it had seen the image.
fn build_openai_messages(
    system: &str,
    history: &[inbharat_harness_core::providers::ModelMessage],
    attachments: &[inbharat_harness_core::providers::AttachmentMetadata],
    local_attachments: &std::collections::BTreeMap<String, (String, String)>,
) -> HarnessResult<Vec<JsonValue>> {
    let mut messages = Vec::with_capacity(history.len().saturating_add(2));
    if !system.trim().is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    // The Harness transcript records tool exchanges as flat Tool-role
    // messages ("tool={id} call={call} result={…}") — it never carries the
    // assistant's own tool-call turn. OpenAI-protocol servers (and the
    // chat template behind them) require the assistant tool-call turn to
    // precede its results; without it the model never sees that it already
    // issued the call, re-issues it every step and the run dies on the
    // step budget (live-observed: 48/48 steps, budget_exceeded, then a
    // silent legacy fallback). Reconstruct the canonical pair: one
    // assistant message carrying the grouped tool_calls, followed by each
    // tool result with its matching tool_call_id.
    let mut index = 0;
    while index < history.len() {
        let message = &history[index];
        if message.role == inbharat_harness_core::providers::ModelRole::Tool {
            let mut calls = Vec::new();
            let mut results = Vec::new();
            while index < history.len()
                && history[index].role == inbharat_harness_core::providers::ModelRole::Tool
            {
                let transcript = &history[index].content;
                if let Some((tool_id, call_id, result)) = parse_tool_transcript(transcript) {
                    calls.push(json!({
                        "id": call_id,
                        "type": "function",
                        "function": {
                            "name": tool_id,
                            "arguments": "{}",
                        },
                    }));
                    results.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": result,
                    }));
                } else {
                    // Not the harness transcript format (defensive): pass
                    // the message through unchanged rather than guessing a
                    // structure the model may misread.
                    results.push(json!({
                        "role": "tool",
                        "content": transcript,
                    }));
                }
                index += 1;
            }
            if !calls.is_empty() {
                messages.push(json!({
                    "role": "assistant",
                    "content": JsonValue::Null,
                    "tool_calls": calls,
                }));
            }
            messages.extend(results);
        } else {
            messages.push(json!({
                "role": role_name(message.role),
                "content": message.content,
            }));
            index += 1;
        }
    }
    // Vision: render the last user message as multimodal parts (text +
    // image_url data URLs). llama-server resolves each image through the
    // mmproj projector passed at startup.
    if !attachments.is_empty() {
        let last_user = messages
            .iter()
            .rposition(|message| message.get("role").and_then(JsonValue::as_str) == Some("user"))
            .ok_or_else(|| {
                Failure::new(
                    ErrorCode::ProviderFailed,
                    FailureClass::Provider,
                    "pai.model.attachments",
                    "attachments require a user message to attach to",
                )
            })?;
        let mut parts = vec![json!({
            "type": "text",
            "text": messages[last_user].get("content").cloned().unwrap_or(JsonValue::String(String::new())),
        })];
        for attachment in attachments {
            let (media_type, base64_bytes) =
                local_attachments.get(&attachment.id).ok_or_else(|| {
                    Failure::new(
                        ErrorCode::ProviderFailed,
                        FailureClass::Provider,
                        "pai.model.attachments",
                        format!("attachment bytes are missing for id {}", attachment.id),
                    )
                })?;
            parts.push(json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{};base64,{}", media_type, base64_bytes),
                }
            }));
        }
        messages[last_user]["content"] = JsonValue::Array(parts);
    }
    Ok(messages)
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

        let messages = build_openai_messages(
            &request.system,
            &request.messages,
            &request.attachments,
            &self.attachments,
        )?;

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
            // Defect #28 (live-caught 2026-09-13): at 0.7 the local 12B
            // intermittently ignored the tool-call protocol on agentic runs —
            // one 2746-token completion of pasted code, zero tool calls, on a
            // "create these files with fs.write" task, while the same prompt
            // at other samples ran 10 tool steps. Tool-bearing requests need
            // format-faithful, deterministic outputs; plain Q&A (no tools)
            // keeps the livelier 0.7.
            "temperature": if tools.is_empty() { 0.7 } else { 0.2 },
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

#[cfg(test)]
mod transcript_tests {
    use super::*;
    use inbharat_harness_core::providers::{ModelMessage, ModelRole};

    fn message(role: ModelRole, content: &str) -> ModelMessage {
        ModelMessage {
            role,
            content: content.to_owned(),
        }
    }

    #[test]
    fn tool_transcript_round_trips() {
        let (tool, call, result) =
            parse_tool_transcript("tool=fs.write call=r-1-2-1 result=Wrote 12 bytes")
                .expect("canonical harness format parses");
        assert_eq!(tool, "fs.write");
        assert_eq!(call, "r-1-2-1");
        assert_eq!(result, "Wrote 12 bytes");
    }

    #[test]
    fn tool_transcript_tolerates_markers_inside_results() {
        let (_, _, result) = parse_tool_transcript(
            "tool=workspace.search call=r-1-3-1 result=hit.txt:1: tool= not a call= marker",
        )
        .expect("first separators win");
        assert_eq!(result, "hit.txt:1: tool= not a call= marker");
    }

    #[test]
    fn tool_transcript_rejects_foreign_shapes() {
        assert!(parse_tool_transcript("").is_none());
        assert!(parse_tool_transcript("the model wrote something").is_none());
        assert!(parse_tool_transcript("tool=fs.write missing-call-marker result=x").is_none());
    }

    #[test]
    fn tool_results_are_preceded_by_an_assistant_tool_calls_turn() {
        let history = vec![
            message(ModelRole::User, "create the marker file"),
            message(
                ModelRole::Tool,
                "tool=fs.write call=r-1-2-1 result=Wrote 41 bytes to live-test.txt",
            ),
            message(
                ModelRole::Tool,
                "tool=fs.read call=r-1-3-1 result=PAI live acceptance marker XYZ42",
            ),
            message(ModelRole::User, "now read it back"),
        ];
        let messages = build_openai_messages("system prefix", &history, &[], &Default::default())
            .expect("build succeeds");
        // system, user, assistant(tool_calls), tool, tool, user
        assert_eq!(messages.len(), 6, "{messages:?}");
        assert_eq!(
            messages[0],
            json!({"role": "system", "content": "system prefix"})
        );
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
        assert!(messages[2]["content"].is_null());
        let calls = messages[2]["tool_calls"].as_array().expect("grouped calls");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["function"]["name"], "fs.write");
        assert_eq!(calls[0]["id"], "r-1-2-1");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "r-1-2-1");
        assert_eq!(messages[3]["content"], "Wrote 41 bytes to live-test.txt");
        assert_eq!(messages[4]["role"], "tool");
        assert_eq!(messages[4]["tool_call_id"], "r-1-3-1");
        assert_eq!(messages[5]["role"], "user");
    }

    #[test]
    fn non_transcript_tool_messages_pass_through_without_fabricated_calls() {
        let history = vec![message(ModelRole::Tool, "raw legacy tool content")];
        let messages =
            build_openai_messages("", &history, &[], &Default::default()).expect("build succeeds");
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0],
            json!({"role": "tool", "content": "raw legacy tool content"})
        );
    }

    #[test]
    fn plain_history_is_untouched() {
        let history = vec![
            message(ModelRole::User, "hello"),
            message(ModelRole::Assistant, "hi there"),
        ];
        let messages =
            build_openai_messages("", &history, &[], &Default::default()).expect("build succeeds");
        assert_eq!(
            messages,
            vec![
                json!({"role": "user", "content": "hello"}),
                json!({"role": "assistant", "content": "hi there"}),
            ]
        );
    }

    /// A one-shot llama-server stand-in: captures the exact HTTP body the
    /// adapter posts and answers with a minimal valid completion. Used by
    /// the wire-level tests below — what matters there is the request, not
    /// the response.
    fn capture_server() -> (u16, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let port = listener.local_addr().expect("mock addr").port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("mock accept");
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            // Read until the headers arrive, then exactly Content-Length bytes.
            let header_end;
            loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    header_end = buffer
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .unwrap_or(0);
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                    header_end = pos;
                    break;
                }
            }
            let headers = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
            let content_length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.trim().eq_ignore_ascii_case("content-length") {
                        value.trim().parse().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            while buffer.len() < header_end + 4 + content_length {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);
            }
            let body = buffer[header_end + 4..].to_vec();
            let _ = tx.send(String::from_utf8_lossy(&body).into_owned());
            let payload = concat!(
                "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"ok\"},",
                "\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,",
                "\"completion_tokens\":1,\"total_tokens\":2}}"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
        (port, rx)
    }

    fn vision_request(
        attachments: Vec<inbharat_harness_core::providers::AttachmentMetadata>,
    ) -> inbharat_harness_core::providers::ModelRequest {
        inbharat_harness_core::providers::ModelRequest {
            request_id: "r-1-1-1".to_owned(),
            provider: "pai-llama-local".to_owned(),
            model: "test-model".to_owned(),
            system: "system prefix".to_owned(),
            messages: vec![message(ModelRole::User, "what is in this image?")],
            tools: Vec::new(),
            attachments,
            max_output_bytes: 4096,
        }
    }

    /// The attachment must reach the wire as an image_url multimodal part on
    /// the last user message — live-caught regression class: the model
    /// truthfully answers "I cannot see any images" when this is dropped.
    #[test]
    fn attachment_bytes_reach_the_wire_as_image_url_parts() {
        let (port, rx) = capture_server();
        let provider = PaiLlamaLocalProvider::new("test-model", port)
            .expect("build provider")
            .with_attachment("attach-1", "image/png", "aGVsbG8=");
        let request = vision_request(vec![inbharat_harness_core::providers::AttachmentMetadata {
            id: "attach-1".to_owned(),
            media_type: "image/png".to_owned(),
            byte_len: 5,
            digest: "deadbeef".to_owned(),
            display_name: None,
        }]);
        let cancel = CancellationToken::new();
        let response = provider
            .stream(&request, &cancel, &mut |_chunk| Ok(()))
            .expect("stream succeeds");
        assert_eq!(response.text, "ok");
        let body = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("captured");
        let parsed: JsonValue = serde_json::from_str(&body).expect("posted body is JSON");
        let user = &parsed["messages"][1];
        assert_eq!(user["role"], "user", "{body}");
        let parts = user["content"]
            .as_array()
            .expect("user content is multimodal parts");
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "what is in this image?");
        assert_eq!(parts[1]["type"], "image_url", "{body}");
        assert_eq!(
            parts[1]["image_url"]["url"],
            "data:image/png;base64,aGVsbG8="
        );
    }

    /// A metadata attachment without matching local bytes must fail closed —
    /// never silently degrade to a text-only request.
    #[test]
    fn attachments_without_local_bytes_fail_closed() {
        let (port, _rx) = capture_server();
        let provider = PaiLlamaLocalProvider::new("test-model", port).expect("build provider");
        let request = vision_request(vec![inbharat_harness_core::providers::AttachmentMetadata {
            id: "attach-1".to_owned(),
            media_type: "image/png".to_owned(),
            byte_len: 5,
            digest: "deadbeef".to_owned(),
            display_name: None,
        }]);
        let cancel = CancellationToken::new();
        let error = provider
            .stream(&request, &cancel, &mut |_chunk| Ok(()))
            .expect_err("missing local bytes must fail");
        assert!(error.to_string().contains("missing"), "{error}");
    }

    /// Defect #28 pin: tool-bearing agent requests must go out at low
    /// temperature (format-faithful tool calls) with tools + tool_choice on
    /// the wire; toolless Q&A keeps the livelier 0.7. Live-caught 2026-09-13:
    /// at 0.7 the local 12B answered a "create these files" task with one
    /// 2746-token prose dump and zero tool calls.
    #[test]
    fn tool_bearing_requests_pin_low_temperature() {
        let (port, rx) = capture_server();
        let provider = PaiLlamaLocalProvider::new("test-model", port).expect("build provider");
        let mut request = vision_request(vec![]);
        request.tools = vec![inbharat_harness_core::providers::ModelTool {
            id: "fs.write".to_owned(),
            description: "write a file".to_owned(),
            input_schema: r#"{"type":"object","properties":{"path":{"type":"string"}}}"#.to_owned(),
        }];
        let cancel = CancellationToken::new();
        let response = provider
            .stream(&request, &cancel, &mut |_chunk| Ok(()))
            .expect("stream succeeds");
        assert_eq!(response.text, "ok");
        let body = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("captured");
        let parsed: JsonValue = serde_json::from_str(&body).expect("posted body is JSON");
        assert_eq!(parsed["temperature"], json!(0.2), "{body}");
        assert_eq!(parsed["tool_choice"], json!("auto"), "{body}");
        assert_eq!(parsed["tools"][0]["function"]["name"], "fs.write", "{body}");
    }

    #[test]
    fn toolless_requests_keep_default_temperature() {
        let (port, rx) = capture_server();
        let provider = PaiLlamaLocalProvider::new("test-model", port).expect("build provider");
        let request = vision_request(vec![]);
        let cancel = CancellationToken::new();
        provider
            .stream(&request, &cancel, &mut |_chunk| Ok(()))
            .expect("stream succeeds");
        let body = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("captured");
        let parsed: JsonValue = serde_json::from_str(&body).expect("posted body is JSON");
        assert_eq!(parsed["temperature"], json!(0.7), "{body}");
        assert!(parsed.get("tools").is_none(), "{body}");
        assert!(parsed.get("tool_choice").is_none(), "{body}");
    }
}

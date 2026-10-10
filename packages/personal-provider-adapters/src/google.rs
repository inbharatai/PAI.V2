//! Exact Google HTTPS endpoints. Redirects/proxies disabled; bounded body/time/page.
//! No automatic retries, especially not sends. A failed mutation/readback stays uncertain.
use crate::{ensure, oauth::Tokens, types::*, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::{Client, Method, Url};
use serde_json::{json, Value};
use std::time::Duration;

pub fn client() -> Result<Client> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|_| "Provider transport unavailable".into())
}
pub async fn bounded_json(mut response: reqwest::Response, limit: usize) -> Result<Value> {
    let status = response.status();
    // Never expose error bodies/URLs, which can contain message text or credentials.
    ensure(
        status.is_success(),
        match status.as_u16() {
            401 => "Account authorization expired/revoked",
            403 => "Provider permission denied",
            404 => "Provider object not found",
            409 | 412 => "Provider changed; fresh review required",
            429 => "Rate limited; wait and explicitly retry reads only",
            _ => "Provider request failed; mutation requires reconciliation",
        },
    )?;
    ensure(
        response.content_length().is_none_or(|n| n <= limit as u64),
        "Provider response exceeds size bound",
    )?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Provider response interrupted; reconcile mutations")?
    {
        ensure(
            bytes.len() + chunk.len() <= limit,
            "Provider response exceeds size bound",
        )?;
        bytes.extend_from_slice(&chunk);
    }
    let result =
        serde_json::from_slice(&bytes).map_err(|_| "Provider returned malformed JSON".into());
    bytes.fill(0);
    result
}
pub struct Google<'a> {
    tokens: &'a Tokens,
    client: Client,
    session_guard: Option<&'a dyn Fn() -> Result<()>>,
    /// Declared-connector egress policy; every URL this adapter contacts passes through it.
    egress: crate::guardian::Egress,
    /// Guardian context + the exact acknowledged fingerprint (if the human accepted a WARN).
    guardian: (unoone_privacy_guardian::Context, Option<String>),
    #[cfg(test)]
    base: Option<String>,
}
impl<'a> Google<'a> {
    #[cfg(test)]
    pub(crate) fn test_client(tokens: &'a Tokens, base: String) -> Self {
        Self {
            tokens,
            client: client().unwrap(),
            session_guard: None,
            egress: crate::guardian::Egress::for_connected_account(
                &tokens.account,
                true,
                crate::now_ms(),
            )
            .unwrap(),
            guardian: (Default::default(), None),
            base: Some(base),
        }
    }
    pub fn new(tokens: &'a Tokens) -> Result<Self> {
        let write = tokens
            .scopes
            .iter()
            .any(|s| !SCOPES_READ.contains(&s.as_str()));
        Ok(Self {
            tokens,
            client: client()?,
            session_guard: None,
            egress: crate::guardian::Egress::for_connected_account(
                &tokens.account,
                write,
                crate::now_ms(),
            )?,
            guardian: (Default::default(), None),
            #[cfg(test)]
            base: None,
        })
    }
    pub fn with_session_guard(mut self, guard: &'a dyn Fn() -> Result<()>) -> Self {
        self.session_guard = Some(guard);
        self
    }
    /// Local guardian context and the fingerprint the human explicitly acknowledged in the UI.
    pub fn with_guardian(
        mut self,
        ctx: unoone_privacy_guardian::Context,
        acknowledged_fingerprint: Option<String>,
    ) -> Self {
        self.guardian = (ctx, acknowledged_fingerprint);
        self
    }
    fn url(&self, calendar: bool, path: &[&str]) -> Result<Url> {
        let base = if calendar {
            "https://www.googleapis.com/calendar/v3/"
        } else {
            "https://gmail.googleapis.com/gmail/v1/users/me/"
        };
        #[cfg(test)]
        let base = self.base.as_deref().unwrap_or(base);
        let mut url = Url::parse(base).map_err(|_| "Endpoint unavailable")?;
        {
            let mut segments = url.path_segments_mut().map_err(|_| "Invalid endpoint")?;
            segments.pop_if_empty();
            for segment in path {
                identifier(segment)?;
                segments.push(segment);
            }
        }
        Ok(url)
    }
    async fn call(
        &self,
        method: Method,
        url: Url,
        body: Option<Value>,
        etag: Option<&str>,
    ) -> Result<Value> {
        if let Some(check) = self.session_guard {
            check()?;
        }
        let bytes = match &body {
            Some(body) => serde_json::to_vec(body)
                .map_err(|_| "Invalid request")?
                .len() as u64,
            None => 0,
        };
        ensure(bytes <= 64 * 1024, "Provider request size bound")?;
        // Outbound network policy: destination must match the declared connector manifest.
        self.egress.authorize(&url, bytes, crate::now_ms())?;
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(&self.tokens.access_token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(etag) = etag {
            request = request.header("If-Match", etag);
        }
        bounded_json(
            request.send().await.map_err(|_| {
                "Provider unavailable; if dispatched mutation, hold for reconciliation"
            })?,
            MAX_RESPONSE,
        )
        .await
    }
    pub async fn search(&self, folder: &str, query: &str, page: Option<&str>) -> Result<Value> {
        self.tokens.require(SCOPES_READ[0])?;
        identifier(folder)?;
        ensure(
            query.len() <= 1024 && page.is_none_or(|s| s.len() <= 2048),
            "Search/page bound",
        )?;
        let mut url = self.url(false, &["messages"])?;
        url.query_pairs_mut().extend_pairs([
            ("labelIds", folder),
            ("q", query),
            ("maxResults", "50"),
            ("includeSpamTrash", "false"),
        ]);
        if let Some(page) = page {
            url.query_pairs_mut().append_pair("pageToken", page);
        }
        let value = self.call(Method::GET, url, None, None).await?;
        ensure(
            value["messages"]
                .as_array()
                .is_none_or(|a| a.len() <= MAX_PAGE),
            "Provider page too large",
        )?;
        Ok(value)
    }
    pub async fn labels(&self) -> Result<Value> {
        self.tokens.require(SCOPES_READ[0])?;
        self.call(Method::GET, self.url(false, &["labels"])?, None, None)
            .await
    }
    pub async fn message(&self, folder: &str, id: &str) -> Result<Value> {
        self.tokens.require(SCOPES_READ[0])?;
        let mut url = self.url(false, &["messages", id])?;
        url.query_pairs_mut().append_pair("format", "full");
        let value = self.call(Method::GET, url, None, None).await?;
        ensure(
            value["id"].as_str() == Some(id)
                && value["labelIds"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|s| s.as_str() == Some(folder))),
            "Message outside approved folder or ID mismatch",
        )?;
        Ok(value) // MIME/HTML/headers are untrusted data; no attachment fetch/render/approval.
    }
    pub async fn thread(&self, folder: &str, id: &str) -> Result<Value> {
        self.tokens.require(SCOPES_READ[0])?;
        let mut url = self.url(false, &["threads", id])?;
        url.query_pairs_mut().append_pair("format", "full");
        let value = self.call(Method::GET, url, None, None).await?;
        let messages = value["messages"].as_array().ok_or("Malformed thread")?;
        ensure(
            messages.len() <= 20
                && messages.iter().all(|m| {
                    m["labelIds"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|s| s.as_str() == Some(folder)))
                }),
            "Thread exceeds reply bound or contains messages outside approved folder",
        )?;
        Ok(value)
    }
    pub async fn calendars(&self, page: Option<&str>) -> Result<Value> {
        self.tokens.require(SCOPES_READ[1])?;
        ensure(page.is_none_or(|s| s.len() <= 2048), "Page token bound")?;
        let mut url = self.url(true, &["users", "me", "calendarList"])?;
        url.query_pairs_mut().append_pair("maxResults", "50");
        if let Some(page) = page {
            url.query_pairs_mut().append_pair("pageToken", page);
        }
        let value = self.call(Method::GET, url, None, None).await?;
        ensure(
            value["items"]
                .as_array()
                .is_none_or(|a| a.len() <= MAX_PAGE),
            "Calendar page bound",
        )?;
        Ok(value)
    }
    pub async fn events(
        &self,
        calendar: &str,
        start: &str,
        end: &str,
        page: Option<&str>,
    ) -> Result<Value> {
        self.tokens.require(SCOPES_READ[1])?;
        interval(start, end)?;
        ensure(page.is_none_or(|s| s.len() <= 2048), "Page token bound")?;
        let mut url = self.url(true, &["calendars", calendar, "events"])?;
        url.query_pairs_mut().extend_pairs([
            ("timeMin", start),
            ("timeMax", end),
            ("singleEvents", "true"),
            ("maxResults", "50"),
        ]);
        if let Some(page) = page {
            url.query_pairs_mut().append_pair("pageToken", page);
        }
        let value = self.call(Method::GET, url, None, None).await?;
        ensure(
            value["items"]
                .as_array()
                .is_none_or(|a| a.len() <= MAX_PAGE),
            "Event page bound",
        )?;
        Ok(value)
    }
    pub async fn event(&self, calendar: &str, id: &str) -> Result<Value> {
        self.tokens.require(SCOPES_READ[1])?;
        self.call(
            Method::GET,
            self.url(true, &["calendars", calendar, "events", id])?,
            None,
            None,
        )
        .await
    }
    pub async fn free_busy(&self, calendar: &str, start: &str, end: &str) -> Result<Value> {
        self.tokens.require(SCOPES_READ[1])?;
        identifier(calendar)?;
        interval(start, end)?;
        let value=self.call(Method::POST,self.url(true,&["freeBusy"])?,Some(json!({"timeMin":start,"timeMax":end,"calendarExpansionMax":1,"groupExpansionMax":1,"items":[{"id":calendar}]})),None).await?;
        let data = &value["calendars"][calendar];
        ensure(
            data.is_object()
                && data["errors"].as_array().is_none_or(|a| a.is_empty())
                && data["busy"].is_array(),
            "Availability unknown; never interpret provider errors as free",
        )?;
        Ok(value)
    }
    /// Caller MUST durably write NeedsReconciliation before this method; its grant
    /// cannot be constructed by deserializing task state or provider text.
    pub async fn commit(
        &self,
        review: &Review,
        grant: &CapabilityGrant,
        now: u64,
    ) -> Result<ProviderReceipt> {
        grant.check(review, &self.tokens.account, now)?;
        // Host-owned guardian gate: BLOCK never dispatches; WARN needs the exact acknowledged fingerprint.
        crate::guardian::enforce(review, &self.guardian.0, self.guardian.1.as_deref(), now)?;
        let value = match &review.mutation {
            Mutation::Send { draft } | Mutation::SaveDraft { draft } => {
                self.tokens
                    .require("https://www.googleapis.com/auth/gmail.compose")?;
                if let Some(reply) = &draft.reply {
                    let thread = self.thread(&review.container, &reply.thread_id).await?;
                    ensure(
                        thread["messages"].as_array().is_some_and(|messages| {
                            messages.iter().any(|m| {
                                m["payload"]["headers"].as_array().is_some_and(|headers| {
                                    headers.iter().any(|h| {
                                        h["name"]
                                            .as_str()
                                            .is_some_and(|n| n.eq_ignore_ascii_case("Message-ID"))
                                            && h["value"].as_str() == Some(&reply.message_id)
                                    })
                                })
                            })
                        }),
                        "Reply Message-ID is not in the permitted provider thread",
                    )?;
                }
                let mut message = json!({"raw":draft.raw(&review.account,&review.operation_id)?});
                if let Some(reply) = &draft.reply {
                    message["threadId"] = json!(reply.thread_id);
                }
                grant.check(review, &self.tokens.account, crate::now_ms())?;
                if matches!(review.mutation, Mutation::Send { .. }) {
                    self.call(
                        Method::POST,
                        self.url(false, &["messages", "send"])?,
                        Some(message),
                        None,
                    )
                    .await?
                } else {
                    self.call(
                        Method::POST,
                        self.url(false, &["drafts"])?,
                        Some(json!({"message":message})),
                        None,
                    )
                    .await?
                }
            }
            Mutation::Label {
                message_id,
                add,
                remove,
            } => {
                self.tokens
                    .require("https://www.googleapis.com/auth/gmail.modify")?;
                self.message(&review.container, message_id).await?;
                grant.check(review, &self.tokens.account, crate::now_ms())?;
                self.call(
                    Method::POST,
                    self.url(false, &["messages", message_id, "modify"])?,
                    Some(json!({"addLabelIds":add,"removeLabelIds":remove})),
                    None,
                )
                .await?
            }
            Mutation::CreateEvent { event } | Mutation::UpdateEvent { event, .. } => {
                self.tokens
                    .require("https://www.googleapis.com/auth/calendar.events")?;
                let availability = self
                    .free_busy(&review.container, &event.start, &event.end)
                    .await?;
                // Existing event occupies its own slot. Updates need explicit conflict
                // interpretation; reject overlaps with any other listed event.
                if let Mutation::UpdateEvent { event_id, etag, .. } = &review.mutation {
                    let existing = self.event(&review.container, event_id).await?;
                    ensure(existing["etag"]==*etag && existing["attendees"].as_array().is_none_or(|a|a.iter().all(|v|v["email"].as_str().is_some_and(|s|event.attendees.iter().any(|e|e==s)))),"Event changed or recipient removal needs a separately supported review; blocked")?;
                    let events = self
                        .events(&review.container, &event.start, &event.end, None)
                        .await?;
                    ensure(
                        events["nextPageToken"].is_null()
                            && events["items"].as_array().is_some_and(|a| {
                                a.iter().all(|e| {
                                    e["id"].as_str() == Some(event_id)
                                        || e["status"] == "cancelled"
                                        || e["transparency"] == "transparent"
                                })
                            }),
                        "Conflict or incomplete event page; review a different time",
                    )?;
                } else {
                    ensure(
                        availability["calendars"][&review.container]["busy"]
                            .as_array()
                            .is_some_and(|a| a.is_empty()),
                        "Calendar conflict; choose/review another time",
                    )?;
                }
                let mut body = event.json();
                let (method, mut url, etag) =
                    if let Mutation::UpdateEvent { event_id, etag, .. } = &review.mutation {
                        (
                            Method::PATCH,
                            self.url(true, &["calendars", &review.container, "events", event_id])?,
                            Some(etag.as_str()),
                        )
                    } else {
                        body["id"] = json!(review.event_id()?);
                        (
                            Method::POST,
                            self.url(true, &["calendars", &review.container, "events"])?,
                            None,
                        )
                    };
                url.query_pairs_mut().append_pair("sendUpdates", "all"); // Review explicitly lists every invite recipient.
                grant.check(review, &self.tokens.account, crate::now_ms())?;
                self.call(method, url, Some(body), etag).await?
            }
            Mutation::CancelEvent {
                event_id,
                etag,
                event,
            } => {
                self.tokens
                    .require("https://www.googleapis.com/auth/calendar.events")?;
                let existing = self.event(&review.container, event_id).await?;
                ensure(
                    existing["etag"] == *etag
                        && existing["summary"] == event.summary
                        && existing["start"]["dateTime"] == event.start
                        && existing["end"]["dateTime"] == event.end,
                    "Cancellation changed; review exact event again",
                )?;
                let mut actual = existing["attendees"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v["email"].as_str().map(str::to_string))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let mut expected = event.attendees.clone();
                actual.sort();
                expected.sort();
                ensure(
                    actual == expected,
                    "Cancellation recipients changed; review again",
                )?;
                let mut url =
                    self.url(true, &["calendars", &review.container, "events", event_id])?;
                url.query_pairs_mut().append_pair("sendUpdates", "all");
                grant.check(review, &self.tokens.account, crate::now_ms())?;
                self.call(
                    Method::PATCH,
                    url,
                    Some(json!({"status":"cancelled"})),
                    Some(etag),
                )
                .await?
            }
        };
        let id = value["id"]
            .as_str()
            .ok_or("Provider ID missing; reconcile before any retry")?;
        identifier(id)?;
        self.verify(review, id).await
    }
    /// Read-only reconciliation; never retries a send. Unknown IDs require exact
    /// provider search/manual inspection, not a new POST.
    pub async fn verify(&self, review: &Review, id: &str) -> Result<ProviderReceipt> {
        ensure(
            review.account == self.tokens.account,
            "Account binding mismatch",
        )?;
        let observed = match &review.mutation {
            Mutation::SaveDraft { .. } => {
                let mut url = self.url(false, &["drafts", id])?;
                url.query_pairs_mut().append_pair("format", "raw");
                self.call(Method::GET, url, None, None).await?
            }
            Mutation::Send { .. } | Mutation::Label { .. } => {
                let mut url = self.url(false, &["messages", id])?;
                url.query_pairs_mut().append_pair("format", "raw");
                self.call(Method::GET, url, None, None).await?
            }
            _ => self.event(&review.container, id).await?,
        };
        ensure(observed["id"].as_str() == Some(id), "Readback ID mismatch")?;
        match &review.mutation {
            Mutation::Send { draft } | Mutation::SaveDraft { draft } => {
                let message = if matches!(review.mutation, Mutation::SaveDraft { .. }) {
                    &observed["message"]
                } else {
                    &observed
                };
                if matches!(review.mutation, Mutation::Send { .. }) {
                    ensure(
                        message["labelIds"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|s| s == "SENT")),
                        "Readback did not establish SENT",
                    )?;
                }
                let raw = message["raw"].as_str().ok_or("Readback raw MIME missing")?;
                let decoded = URL_SAFE_NO_PAD
                    .decode(raw.trim_end_matches('='))
                    .map_err(|_| "Invalid readback MIME")?;
                let text = String::from_utf8(decoded)
                    .map_err(|_| "Non-UTF8 MIME; manual reconciliation needed")?;
                let expected = URL_SAFE_NO_PAD
                    .decode(draft.raw(&review.account, &review.operation_id)?)
                    .map_err(|_| "Invalid expected MIME")?;
                // Google's transit headers may precede the submitted MIME. Exact submitted
                // MIME must remain as a suffix; any normalization is conservative UNVERIFIED.
                ensure(
                    text.as_bytes().ends_with(&expected),
                    "Readback content/recipients differ; manual reconciliation required",
                )?;
                if let Some(reply) = &draft.reply {
                    ensure(
                        message["threadId"].as_str() == Some(&reply.thread_id),
                        "Reply thread mismatch",
                    )?;
                }
            }
            Mutation::Label {
                message_id,
                add,
                remove,
            } => {
                ensure(id == message_id, "Label message ID mismatch")?;
                let labels = observed["labelIds"]
                    .as_array()
                    .ok_or("Readback labels missing")?;
                ensure(
                    add.iter().all(|s| labels.contains(&json!(s)))
                        && remove.iter().all(|s| !labels.contains(&json!(s))),
                    "Labels not observed",
                )?;
            }
            Mutation::CreateEvent { event } | Mutation::UpdateEvent { event, .. } => {
                let expected_id = match &review.mutation {
                    Mutation::UpdateEvent { event_id, .. } => event_id.clone(),
                    _ => review.event_id()?,
                };
                ensure(
                    id == expected_id
                        && observed["status"] == "confirmed"
                        && observed["summary"] == event.summary,
                    "Event ID/status/summary mismatch",
                )?;
                for key in ["start", "end"] {
                    let expected = if key == "start" {
                        &event.start
                    } else {
                        &event.end
                    };
                    let actual = observed[key]["dateTime"]
                        .as_str()
                        .ok_or("Event time missing")?;
                    ensure(
                        chrono::DateTime::parse_from_rfc3339(actual).ok()
                            == chrono::DateTime::parse_from_rfc3339(expected).ok()
                            && observed[key]["timeZone"] == event.time_zone,
                        "Event time/zone mismatch",
                    )?;
                }
                let mut attendees = observed["attendees"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v["email"].as_str().map(str::to_string))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let mut expected = event.attendees.clone();
                attendees.sort();
                expected.sort();
                ensure(attendees == expected, "Event attendees mismatch")?;
            }
            Mutation::CancelEvent { event_id, .. } => ensure(
                id == event_id && observed["status"] == "cancelled",
                "Cancellation not observed",
            )?,
        }
        Ok(ProviderReceipt {
            operation_id: review.operation_id.clone(),
            task_id: review.task_id.clone(),
            account: review.account.clone(),
            provider_id: id.into(),
            container: review.container.clone(),
            status: CommitStatus::Verified,
            observed_ms: crate::now_ms(),
            detail: "Provider ID and exact postcondition read back; not merely HTTP 200".into(),
        })
    }
}
fn interval(start: &str, end: &str) -> Result<()> {
    let start = chrono::DateTime::parse_from_rfc3339(start)
        .map_err(|_| "Explicit RFC3339 start required")?;
    let end =
        chrono::DateTime::parse_from_rfc3339(end).map_err(|_| "Explicit RFC3339 end required")?;
    ensure(
        end > start && (end - start).num_days() <= 31,
        "Availability range must be positive, at most 31 days",
    )
}
pub(crate) async fn profile(tokens: &Tokens) -> Result<String> {
    let google = Google::new(tokens)?;
    let value = google
        .call(Method::GET, google.url(false, &["profile"])?, None, None)
        .await?;
    value["emailAddress"]
        .as_str()
        .map(str::to_string)
        .ok_or("Google account identity missing".into())
}

//! The Discord publisher: a short message per meeting, posted through a webhook.
//!
//! The pieces, in the order they run:
//!
//! 1. [`render_text`] renders the per-target template (the built-in short one, or the cycle
//!    template named in `targets.discord.template`) with the `Chat` variant. An assignee whose
//!    participant has a Discord user or role id is written as a real mention; mention syntax in
//!    text from the model or the transcript is defused, and `@everyone` / `@here` are
//!    neutralised as in every report.
//! 2. [`build_plan`] splits the text into messages of at most 2000 characters (at a line, then
//!    at a word, then anywhere), chunks embeds by 10 and attaches screenshots in batches of at
//!    most 10 files and [`MAX_BATCH_BYTES`], each message with its own `allowed_mentions`
//!    that lists only user and role ids from `cycle.yaml` that the message actually contains.
//! 3. [`send_messages`] posts them one by one (`?wait=true`), as JSON or as a hand-built
//!    `multipart/form-data` body when there are files. A message that is rate limited with a
//!    short wait is retried in place; a longer wait, or any other failure, is returned with
//!    the number of messages already sent, so the outbox can resume from there.
//!
//! # Limits
//!
//! Content is counted in UTF-16 units, which is never less than the number of characters, so
//! a message is never over the limit however Discord counts. The documented limits are 2000
//! characters of content, 10 embeds and 10 attachments per message. Discord's developer
//! documentation gives 20 MiB per uploaded file for all users and no limit for a whole
//! message; reports about webhook uploads disagree, so the publisher stays conservative and
//! sends at most [`MAX_BATCH_BYTES`] (8 MiB) of files per message and skips a single file
//! over that size, with a note in the message.
//!
//! A request that times out after Discord stored the message cannot be told from one that
//! never arrived, and a webhook has no idempotency key, so a retry after such a failure can
//! post a message twice. This is rare and harmless, and cheaper than losing a message.

use std::time::Duration;

use cycle_store::{
    is_discord_id, render, Draft, Participant, RenderOptions, ValidationIssue, Variant,
    SHORT_TEMPLATE_TEXT,
};
use serde_json::{json, Value};
use store::Failure;

use crate::http::{ErrorKind, HttpClient, HttpError, Method, Request, Response};
use crate::webhook::Webhook;

/// Most UTF-16 units of content in one message.
pub const MAX_CONTENT_UNITS: usize = 2000;
/// Most embeds in one message.
pub const MAX_EMBEDS: usize = 10;
/// Most attachments in one message.
pub const MAX_FILES: usize = 10;
/// Most bytes of attachments the publisher puts in one message (conservative, see the module
/// documentation).
pub const MAX_BATCH_BYTES: usize = 8 * 1024 * 1024;
/// Most messages that carry attachments; the rest is left to the full report.
pub const MAX_ATTACHMENT_MESSAGES: usize = 3;
/// Most user ids and most role ids in one `allowed_mentions`.
pub const MAX_ALLOWED_IDS: usize = 100;
/// `SUPPRESS_EMBEDS`: links in the message do not unfurl.
const FLAG_SUPPRESS_EMBEDS: u64 = 1 << 2;

// ----- text ------------------------------------------------------------------------------

/// What to render for one meeting.
pub struct TextSpec<'a> {
    pub draft: &'a Draft,
    pub participants: &'a [Participant],
    /// Body of the cycle template named in `targets.discord.template`; `None` for the
    /// built-in short template.
    pub template_body: Option<&'a str>,
    pub title: &'a str,
    /// Address of the published report, for `{report_link}`.
    pub report_link: &'a str,
}

/// The text of the message, before it is split.
pub fn render_text(spec: &TextSpec<'_>) -> Result<String, Vec<ValidationIssue>> {
    let opts = RenderOptions {
        title: spec.title.to_string(),
        variant: Variant::Chat,
        report_link: spec.report_link.to_string(),
        discord_mentions: true,
        ..RenderOptions::default()
    };
    render(
        spec.draft,
        spec.participants,
        spec.template_body.unwrap_or(SHORT_TEMPLATE_TEXT),
        &opts,
    )
}

/// Length of `text` as Discord could count it: UTF-16 units.
pub fn content_units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Splits `text` into pieces of at most `limit` UTF-16 units. A piece ends at the last line
/// break that fits, else at the last space, else at the limit (never inside a mention such as
/// `<@123>`). The break that was cut at is dropped; pieces that are only whitespace are left
/// out, so no piece is empty.
pub fn split_content(text: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(2);
    let mut pieces = Vec::new();
    let mut rest = text.trim_matches(|c: char| c == '\n' || c == '\r');
    while content_units(rest) > limit {
        // The longest prefix that fits.
        let mut units = 0;
        let mut end = 0;
        for (i, c) in rest.char_indices() {
            let next = units + c.len_utf16();
            if next > limit {
                break;
            }
            units = next;
            end = i + c.len_utf8();
        }
        let window = &rest[..end];
        let (cut, skip) = if let Some(i) = window.rfind('\n').filter(|&i| i > 0) {
            (i, 1)
        } else if let Some((i, c)) = window
            .char_indices()
            .rev()
            .find(|(i, c)| *i > 0 && c.is_whitespace())
        {
            (i, c.len_utf8())
        } else {
            (hard_cut(window), 0)
        };
        let piece = rest[..cut].trim_end();
        if !piece.trim().is_empty() {
            pieces.push(piece.to_string());
        }
        rest = rest[cut + skip..].trim_start_matches(['\n', '\r']);
    }
    if !rest.trim().is_empty() {
        pieces.push(rest.trim_end().to_string());
    }
    pieces
}

/// Where to cut a window that has no line break and no space: at its end, or before a mention
/// (`<@...`) that would be cut in two.
fn hard_cut(window: &str) -> usize {
    if let Some(open) = window.rfind('<') {
        let tail = &window[open..];
        if open > 0 && tail.len() <= 30 && !tail.contains('>') {
            return open;
        }
    }
    window.len()
}

// ----- mentions --------------------------------------------------------------------------

/// The mentions (`<@id>`, `<@!id>`, `<@&id>`) in a text: users and roles, in order, without
/// repetition.
fn mentions_in(text: &str) -> (Vec<String>, Vec<String>) {
    let mut users: Vec<String> = Vec::new();
    let mut roles: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("<@") {
        rest = &rest[open + 2..];
        let (is_role, body) = match rest.strip_prefix('&') {
            Some(b) => (true, b),
            None => (false, rest.strip_prefix('!').unwrap_or(rest)),
        };
        let digits = body.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || body.as_bytes().get(digits) != Some(&b'>') {
            continue;
        }
        let id = &body[..digits];
        let list = if is_role { &mut roles } else { &mut users };
        if !list.iter().any(|x| x == id) {
            list.push(id.to_string());
        }
    }
    (users, roles)
}

/// The `allowed_mentions` of a message: nothing is parsed from the text, and only the user and
/// role ids that `cycle.yaml` gives to a participant and that the content contains may notify
/// anybody. `@everyone` and `@here` can never be allowed: `parse` is always empty.
pub fn allowed_mentions(content: &str, participants: &[Participant]) -> Value {
    let (users, roles) = mentions_in(content);
    let known = |id: &String, pick: fn(&cycle_store::DiscordRef) -> Option<&str>| {
        is_discord_id(id)
            && participants
                .iter()
                .filter_map(|p| p.discord.as_ref())
                .any(|d| pick(d) == Some(id.as_str()))
    };
    let users: Vec<&String> = users
        .iter()
        .filter(|id| known(id, |d| d.user_id.as_deref()))
        .take(MAX_ALLOWED_IDS)
        .collect();
    let roles: Vec<&String> = roles
        .iter()
        .filter(|id| known(id, |d| d.role_id.as_deref()))
        .take(MAX_ALLOWED_IDS)
        .collect();
    let mut value = json!({"parse": []});
    if !users.is_empty() {
        value["users"] = json!(users);
    }
    if !roles.is_empty() {
        value["roles"] = json!(roles);
    }
    value
}

// ----- attachments -----------------------------------------------------------------------

/// A file to attach, a screenshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub filename: String,
    pub bytes: Vec<u8>,
}

impl Attachment {
    /// The file name is reduced to ASCII letters, digits, `.`, `-` and `_`, so it cannot
    /// break the multipart headers.
    pub fn new(filename: &str, bytes: Vec<u8>) -> Self {
        Self {
            filename: sanitize_filename(filename),
            bytes,
        }
    }

    fn content_type(&self) -> &'static str {
        let ext = self
            .filename
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => "application/octet-stream",
        }
    }
}

fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim_start_matches('.');
    if clean.is_empty() {
        "file".to_string()
    } else {
        clean.to_string()
    }
}

/// Why a file is not attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Larger than [`MAX_BATCH_BYTES`] on its own.
    TooLarge,
    Empty,
    /// More than [`MAX_ATTACHMENT_MESSAGES`] messages of files would be needed.
    TooMany,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedFile {
    pub filename: String,
    pub reason: SkipReason,
}

/// Groups files, in order, into batches of at most [`MAX_FILES`] files and
/// [`MAX_BATCH_BYTES`] bytes, at most [`MAX_ATTACHMENT_MESSAGES`] batches. A file that is
/// empty or alone over the byte limit is skipped, and so is everything that does not fit in
/// the last batch.
pub fn plan_attachments(files: Vec<Attachment>) -> (Vec<Vec<Attachment>>, Vec<SkippedFile>) {
    let mut batches: Vec<Vec<Attachment>> = Vec::new();
    let mut skipped = Vec::new();
    let mut bytes = 0usize;
    let skip = |f: &Attachment, reason| SkippedFile {
        filename: f.filename.clone(),
        reason,
    };
    for file in files {
        if file.bytes.is_empty() {
            skipped.push(skip(&file, SkipReason::Empty));
            continue;
        }
        if file.bytes.len() > MAX_BATCH_BYTES {
            skipped.push(skip(&file, SkipReason::TooLarge));
            continue;
        }
        let fits = batches
            .last()
            .is_some_and(|b| b.len() < MAX_FILES && bytes + file.bytes.len() <= MAX_BATCH_BYTES);
        if !fits {
            if batches.len() == MAX_ATTACHMENT_MESSAGES {
                skipped.push(skip(&file, SkipReason::TooMany));
                continue;
            }
            batches.push(Vec::new());
            bytes = 0;
        }
        bytes += file.bytes.len();
        if let Some(batch) = batches.last_mut() {
            batch.push(file);
        }
    }
    (batches, skipped)
}

/// A line for the end of the message when screenshots were left out.
fn skipped_note(skipped: &[SkippedFile]) -> Option<String> {
    if skipped.is_empty() {
        return None;
    }
    let n = skipped.len();
    let what = if n == 1 { "screenshot" } else { "screenshots" };
    Some(format!(
        "_{n} {what} not attached (too large or too many), see the full report._"
    ))
}

// ----- messages --------------------------------------------------------------------------

/// One message to post.
#[derive(Debug, Clone, PartialEq)]
pub struct OutMessage {
    pub content: String,
    pub allowed_mentions: Value,
    pub embeds: Vec<Value>,
    pub files: Vec<Attachment>,
}

impl OutMessage {
    /// The JSON of the message; with files it is the `payload_json` part.
    pub fn payload(&self) -> Value {
        let mut payload = json!({
            "allowed_mentions": self.allowed_mentions,
            "flags": FLAG_SUPPRESS_EMBEDS,
        });
        if !self.content.is_empty() {
            payload["content"] = json!(self.content);
        }
        if !self.embeds.is_empty() {
            payload["embeds"] = json!(self.embeds);
        }
        if !self.files.is_empty() {
            payload["attachments"] = self
                .files
                .iter()
                .enumerate()
                .map(|(i, f)| json!({"id": i, "filename": f.filename}))
                .collect::<Vec<_>>()
                .into();
        }
        payload
    }
}

/// Everything to post for a meeting, in order, and the files that were left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub messages: Vec<OutMessage>,
    pub skipped: Vec<SkippedFile>,
}

/// Splits a text, embeds and files into messages that respect the limits. The result depends
/// only on the arguments, so a retry rebuilds the same messages and can resume by count.
///
/// Text pieces come first and carry the embeds (10 per message, after the pieces if there are
/// more embeds than pieces); the files follow in messages of their own, captioned
/// `Screenshots 1/2`. Every message has its own `allowed_mentions`.
pub fn build_plan(
    text: &str,
    embeds: Vec<Value>,
    files: Vec<Attachment>,
    participants: &[Participant],
) -> Plan {
    let (batches, skipped) = plan_attachments(files);
    let mut text = text.trim().to_string();
    if let Some(note) = skipped_note(&skipped) {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&note);
    }
    let pieces = split_content(&text, MAX_CONTENT_UNITS);
    let mut embed_chunks: Vec<Vec<Value>> =
        embeds.chunks(MAX_EMBEDS).map(<[Value]>::to_vec).collect();
    embed_chunks.reverse();

    let mut messages = Vec::new();
    for piece in pieces {
        messages.push(OutMessage {
            allowed_mentions: allowed_mentions(&piece, participants),
            content: piece,
            embeds: embed_chunks.pop().unwrap_or_default(),
            files: Vec::new(),
        });
    }
    while let Some(chunk) = embed_chunks.pop() {
        messages.push(OutMessage {
            content: String::new(),
            allowed_mentions: json!({"parse": []}),
            embeds: chunk,
            files: Vec::new(),
        });
    }
    let total = batches.len();
    for (i, batch) in batches.into_iter().enumerate() {
        let content = if total > 1 {
            format!("Screenshots {}/{total}", i + 1)
        } else {
            "Screenshots".to_string()
        };
        messages.push(OutMessage {
            allowed_mentions: json!({"parse": []}),
            content,
            embeds: Vec::new(),
            files: batch,
        });
    }
    Plan { messages, skipped }
}

// ----- multipart -------------------------------------------------------------------------

/// A boundary that occurs in none of the parts.
pub fn pick_boundary(payload: &[u8], files: &[Attachment]) -> String {
    let occurs = |b: &str| {
        let needle = b.as_bytes();
        let hit = |hay: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
        hit(payload) || files.iter().any(|f| hit(&f.bytes))
    };
    let mut n = 0u32;
    loop {
        let candidate = format!("bbb-boundary-{n:04}");
        if !occurs(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// The `multipart/form-data` body Discord expects: a `payload_json` part and a `files[n]`
/// part per file, built by hand.
pub fn multipart_body(boundary: &str, payload: &Value, files: &[Attachment]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(payload.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");
    for (i, file) in files.iter().enumerate() {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"files[{i}]\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                file.filename,
                file.content_type()
            )
            .as_bytes(),
        );
        body.extend_from_slice(&file.bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

fn request_for(hook: &Webhook, message: &OutMessage) -> Request {
    let url = format!("{}?wait=true", hook.url());
    let payload = message.payload();
    if message.files.is_empty() {
        return Request::post_json(url, &payload);
    }
    let boundary = pick_boundary(payload.to_string().as_bytes(), &message.files);
    Request::new(Method::Post, url).bytes(
        &format!("multipart/form-data; boundary={boundary}"),
        multipart_body(&boundary, &payload, &message.files),
    )
}

// ----- sending ---------------------------------------------------------------------------

/// How the sender deals with rate limits.
pub struct Pacing {
    /// A wait up to this long is slept through in place; a longer one is handed back as the
    /// failure so the outbox can schedule the retry.
    pub max_inline_wait: Duration,
    /// How often one message is retried in place after a rate limit.
    pub max_inline_retries: u32,
    pub sleep: Box<dyn Fn(Duration) + Send + Sync>,
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            max_inline_wait: Duration::from_secs(5),
            max_inline_retries: 2,
            sleep: Box::new(std::thread::sleep),
        }
    }
}

/// What a completed send did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReport {
    /// Messages sent in total, including those sent before the resume point.
    pub sent: usize,
    /// Ids of the messages sent by this call (empty strings when Discord sent none back).
    pub message_ids: Vec<String>,
}

/// A send that stopped. `sent` is how many messages are out, the index to resume from.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{error} (after {sent} messages)")]
pub struct SendFailure {
    pub error: HttpError,
    pub sent: usize,
}

fn rate_limit_wait(error: &HttpError) -> Option<Duration> {
    match (error.kind, error.failure) {
        (
            ErrorKind::RateLimited,
            Failure::Transient {
                retry_after_ms: Some(ms),
            },
        ) => Some(Duration::from_millis(ms.max(0) as u64)),
        _ => None,
    }
}

/// The wait Discord announces before the next request, when this one used up the bucket.
fn bucket_wait(response: &Response) -> Option<Duration> {
    if response.header("x-ratelimit-remaining")? != "0" {
        return None;
    }
    let secs: f64 = response
        .header("x-ratelimit-reset-after")?
        .trim()
        .parse()
        .ok()?;
    (secs.is_finite() && secs > 0.0).then(|| Duration::from_secs_f64(secs))
}

fn send_one(
    client: &HttpClient,
    hook: &Webhook,
    message: &OutMessage,
    pacing: &Pacing,
) -> Result<Response, HttpError> {
    let request = request_for(hook, message);
    let mut retries = 0;
    loop {
        match client.send(&request) {
            Ok(response) => return Ok(response),
            Err(error) => match rate_limit_wait(&error) {
                Some(wait)
                    if wait <= pacing.max_inline_wait && retries < pacing.max_inline_retries =>
                {
                    retries += 1;
                    (pacing.sleep)(wait);
                }
                _ => return Err(error),
            },
        }
    }
}

/// Posts `messages[start..]` in order. `on_sent(count, id)` is called after each message that
/// Discord accepted, with the number of messages that are out, so progress can be stored
/// before the next request.
///
/// `client` should be [`Webhook::client`], which scrubs the webhook from error texts.
pub fn send_messages(
    client: &HttpClient,
    hook: &Webhook,
    messages: &[OutMessage],
    start: usize,
    pacing: &Pacing,
    on_sent: &mut dyn FnMut(usize, &str),
) -> Result<SendReport, SendFailure> {
    let mut sent = start.min(messages.len());
    let mut ids = Vec::new();
    for (i, message) in messages.iter().enumerate().skip(sent) {
        let response =
            send_one(client, hook, message, pacing).map_err(|error| SendFailure { error, sent })?;
        let id = response
            .json()
            .ok()
            .and_then(|v| v.get("id").and_then(Value::as_str).map(String::from))
            .unwrap_or_default();
        sent = i + 1;
        on_sent(sent, &id);
        ids.push(id);
        if sent < messages.len() {
            if let Some(wait) = bucket_wait(&response).filter(|w| *w <= pacing.max_inline_wait) {
                (pacing.sleep)(wait);
            }
        }
    }
    Ok(SendReport {
        sent,
        message_ids: ids,
    })
}

#[cfg(test)]
mod tests;

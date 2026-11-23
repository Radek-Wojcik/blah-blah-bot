//! The Discord bot publisher: the meeting summary posted through a bot, with a thread.
//!
//! The bot talks to the REST API only. It opens no gateway connection and so needs no
//! intents: posting, threads, edits and the member search (`GET /guilds/{id}/members/search`)
//! are plain HTTP calls. Every request carries `Authorization: Bot <token>`; the token is
//! registered with the [`HttpClient`] so it never reaches an error text.
//!
//! What is here, and what is not:
//!
//! * The calls: who the bot is, the guild, its roles, the bot's own member, a channel and the
//!   member search; posting a message with a `nonce` (Discord returns the earlier message when
//!   the same author sends the same nonce again, which makes a lost answer harmless), creating
//!   a thread from a message, editing an own message and unarchiving a thread.
//! * [`perms`]: the permissions of the bot in a channel, computed as Discord does, so a setup
//!   mistake is named ("Create Public Threads") before anything is posted.
//! * The text of a message, the split into messages and the mentions are the ones of the
//!   webhook publisher ([`crate::discord`]); only the transport differs.
//!
//! Nothing here decides *what* to post or in which order; that is the dispatcher's job. The
//! functions are single steps that report exactly what Discord did.

use std::cell::Cell;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use store::Failure;

use crate::discord::{bucket_wait, payload_request_body, retry_rate_limited, OutMessage, Pacing};
use crate::http::{
    classify_status, parse_endpoint, redact, ErrorKind, HttpClient, HttpError, Method, Request,
    Response,
};
use crate::webhook::DISCORD_HOSTS;

pub mod perms;

/// The public Discord API.
pub const DEFAULT_API: &str = "https://discord.com/api/v10";
/// Longest thread name Discord accepts (characters).
pub const MAX_THREAD_NAME_CHARS: usize = 100;
/// Longest nonce Discord accepts (characters).
pub const MAX_NONCE_CHARS: usize = 25;
/// How long a thread stays open without a message: 7 days, in minutes.
pub const THREAD_ARCHIVE_MINUTES: u32 = 10_080;
/// `SUPPRESS_EMBEDS`: links in the message do not unfurl.
const FLAG_SUPPRESS_EMBEDS: u64 = 1 << 2;

/// Channel types the publisher knows.
pub const CHANNEL_TEXT: u8 = 0;
pub const CHANNEL_ANNOUNCEMENT: u8 = 5;
pub const CHANNEL_PUBLIC_THREAD: u8 = 11;
pub const CHANNEL_FORUM: u8 = 15;

/// Discord error codes the publisher reacts to.
const CODE_MISSING_ACCESS: u32 = 50001;
const CODE_MISSING_PERMISSIONS: u32 = 50013;
const CODE_THREAD_ARCHIVED: u32 = 50083;
const CODE_THREAD_EXISTS: u32 = 160_004;
const CODE_UNKNOWN_CHANNEL: u32 = 10003;
const CODE_UNKNOWN_GUILD: u32 = 10004;
const CODE_UNKNOWN_MESSAGE: u32 = 10008;
const CODE_UNKNOWN_MEMBER: u32 = 10007;

/// The start of the error text for a missing permission; [`DiscordBot::explain_denied`]
/// appends the names.
const MISSING_PERMISSION_TEXT: &str = "the bot is missing a permission in the channel";

// ----- addresses and small helpers --------------------------------------------------------

/// A checked API base without a trailing slash: `https://discord.com/api/v10` (also on
/// `discordapp.com`, `canary.discord.com` and `ptb.discord.com`), or `http` for this machine
/// only (tests), where any path is allowed. This is the rule that keeps the token from going
/// to another host.
pub fn normalize_api_base(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let endpoint = parse_endpoint(url).map_err(|e| e.message)?;
    if endpoint.path.contains(['?', '#']) {
        return Err("the API address must not have a query or a fragment".into());
    }
    if endpoint.scheme == "https" {
        if !DISCORD_HOSTS.contains(&endpoint.host.to_ascii_lowercase().as_str()) {
            return Err("the API address is not a Discord address".into());
        }
        if endpoint.port.is_some() || endpoint.path != "/api/v10" {
            return Err("the API address must be https://discord.com/api/v10".into());
        }
    }
    Ok(url.to_string())
}

/// Percent-encodes a query value: everything but ASCII letters, digits and `-._~` becomes
/// `%XX` per UTF-8 byte.
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// FNV-1a, 64 bit.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn base36(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// The nonce of message `index` of an operation: the same meeting, operation and index
/// always give the same nonce, so a message that was sent but not acknowledged is recognised
/// by Discord when it is sent again. At most 13 characters (`[0-9a-z]`).
pub fn nonce(meeting_id: &str, op_key: &str, index: usize) -> String {
    base36(fnv1a64(format!("{meeting_id}:{op_key}:{index}").as_bytes()))
}

/// A thread name from a meeting title: whitespace collapsed to single spaces, at most
/// [`MAX_THREAD_NAME_CHARS`] characters (cut at a character boundary), never empty.
pub fn thread_name(title: &str) -> String {
    let collapsed = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = collapsed.chars().take(MAX_THREAD_NAME_CHARS).collect();
    let name = name.trim_end().to_string();
    if name.is_empty() {
        "Meeting".to_string()
    } else {
        name
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn permanent(kind: ErrorKind, message: impl Into<String>) -> HttpError {
    HttpError {
        kind,
        message: message.into(),
        status: None,
        failure: Failure::Permanent,
    }
}

fn body_error(what: &str) -> HttpError {
    permanent(
        ErrorKind::Body,
        format!("the answer of Discord has no {what}"),
    )
}

fn snowflake(id: &str) -> Result<&str, HttpError> {
    if cycle_store::is_discord_id(id) {
        Ok(id)
    } else {
        Err(permanent(
            ErrorKind::InvalidRequest,
            "a Discord id has digits only",
        ))
    }
}

/// A permission value: Discord sends them as decimal strings, a number is accepted too.
fn permission_bits(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

fn text_of(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

// ----- what Discord tells ----------------------------------------------------------------

/// The bot itself (`GET /users/@me`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotUser {
    pub id: String,
    pub username: String,
}

/// A guild (server).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildInfo {
    pub id: String,
    pub name: String,
    pub owner_id: String,
}

/// A role of a guild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    pub id: String,
    pub name: String,
    pub permissions: u64,
    pub mentionable: bool,
}

/// Whose overwrite it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwriteKind {
    Role,
    Member,
}

/// A permission overwrite of a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overwrite {
    /// The id of the role or of the user.
    pub id: String,
    pub kind: OverwriteKind,
    pub allow: u64,
    pub deny: u64,
}

/// A channel or a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: String,
    pub guild_id: Option<String>,
    pub name: String,
    /// `0` text, `5` announcement, `11` public thread, `15` forum.
    pub kind: u8,
    pub overwrites: Vec<Overwrite>,
}

impl ChannelInfo {
    /// A summary can be posted here and a thread started under it: text and announcement
    /// channels only (not forums, not threads).
    pub fn supports_summary(&self) -> bool {
        matches!(self.kind, CHANNEL_TEXT | CHANNEL_ANNOUNCEMENT)
    }

    /// The kind in words, for the interface.
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            CHANNEL_TEXT => "text",
            CHANNEL_ANNOUNCEMENT => "announcement",
            CHANNEL_PUBLIC_THREAD => "thread",
            CHANNEL_FORUM => "forum",
            _ => "other",
        }
    }
}

/// A member of a guild, as the search returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub user_id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub nick: Option<String>,
    pub bot: bool,
    pub roles: Vec<String>,
}

/// A message Discord accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostedMessage {
    pub id: String,
    pub channel_id: String,
    /// The wait Discord announced before the next request (the bucket is used up), when
    /// there is one.
    pub wait: Option<Duration>,
}

/// What an edit did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    Edited,
    /// The message does not exist any more (someone deleted it).
    Deleted,
}

/// The result of checking the bot against a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelCheck {
    pub bot: BotUser,
    pub guild_name: String,
    pub channel: ChannelInfo,
    /// The effective permissions of the bot in the channel.
    pub permissions: u64,
    /// The needed permissions the bot lacks, by name.
    pub missing: Vec<&'static str>,
}

impl ChannelCheck {
    /// Posting can work: a suitable channel and nothing missing.
    pub fn ok(&self) -> bool {
        self.channel.supports_summary() && self.missing.is_empty()
    }
}

fn parse_member(value: &Value) -> Option<Member> {
    let user = value.get("user")?;
    Some(Member {
        user_id: text_of(user, "id")?,
        username: text_of(user, "username").unwrap_or_default(),
        global_name: text_of(user, "global_name"),
        nick: text_of(value, "nick"),
        bot: user.get("bot").and_then(Value::as_bool).unwrap_or(false),
        roles: value
            .get("roles")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn parse_channel(value: &Value) -> Option<ChannelInfo> {
    let overwrites = value
        .get("permission_overwrites")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|o| {
                    Some(Overwrite {
                        id: text_of(o, "id")?,
                        kind: match o.get("type").and_then(Value::as_u64) {
                            Some(1) => OverwriteKind::Member,
                            _ => OverwriteKind::Role,
                        },
                        allow: permission_bits(o.get("allow")),
                        deny: permission_bits(o.get("deny")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ChannelInfo {
        id: text_of(value, "id")?,
        guild_id: text_of(value, "guild_id"),
        name: text_of(value, "name").unwrap_or_default(),
        kind: value
            .get("type")
            .and_then(Value::as_u64)
            .and_then(|t| u8::try_from(t).ok())
            .unwrap_or(u8::MAX),
        overwrites,
    })
}

// ----- the client ------------------------------------------------------------------------

/// A failed call with the Discord error code, when the answer had one.
struct ApiFailure {
    error: HttpError,
    code: Option<u32>,
}

impl ApiFailure {
    fn of(error: HttpError) -> Self {
        Self { error, code: None }
    }
}

fn error_code(body: &[u8]) -> Option<u32> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .get("code")?
        .as_u64()
        .and_then(|c| u32::try_from(c).ok())
}

/// A Discord REST client for one API base and one bot token.
pub struct DiscordBot {
    client: HttpClient,
    api: String,
    token: String,
}

impl DiscordBot {
    /// A client with the default timeouts. `api` is checked with [`normalize_api_base`].
    pub fn new(api: &str, token: &str) -> Result<DiscordBot, HttpError> {
        Self::with_client(HttpClient::new(), api, token)
    }

    /// A client over a given [`HttpClient`] (tests use short timeouts). The token is
    /// registered with it so no error text can contain it.
    pub fn with_client(
        client: HttpClient,
        api: &str,
        token: &str,
    ) -> Result<DiscordBot, HttpError> {
        let api = normalize_api_base(api)
            .map_err(|message| permanent(ErrorKind::InvalidRequest, message))?;
        if token.trim().is_empty() {
            return Err(permanent(
                ErrorKind::InvalidRequest,
                "the bot token is empty",
            ));
        }
        Ok(DiscordBot {
            client: client.with_secret(token),
            api,
            token: token.to_string(),
        })
    }

    pub fn api_base(&self) -> &str {
        &self.api
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.api)
    }

    /// Sends one request with the bot's authorization and maps the failure.
    fn execute(&self, request: &Request) -> Result<Response, ApiFailure> {
        let request = request
            .clone()
            .header("Authorization", &format!("Bot {}", self.token));
        let response = self.client.send_raw(&request).map_err(ApiFailure::of)?;
        match classify_status(&response, now_ms()) {
            None => Ok(response),
            Some(error) => {
                let code = error_code(&response.body);
                Err(ApiFailure {
                    error: self.map_error(error, code),
                    code,
                })
            }
        }
    }

    /// The application words for the failures that need the user.
    fn map_error(&self, mut error: HttpError, code: Option<u32>) -> HttpError {
        if error.failure != Failure::Permanent {
            error.message = redact(&error.message, &[&self.token]);
            return error;
        }
        let known = match (error.status, code) {
            (Some(401), _) => Some("Discord rejected the bot token"),
            (_, Some(CODE_MISSING_PERMISSIONS)) => Some(MISSING_PERMISSION_TEXT),
            (_, Some(CODE_MISSING_ACCESS)) => Some("the bot cannot see the channel"),
            (_, Some(CODE_UNKNOWN_CHANNEL)) => {
                Some("Discord does not know this channel, check the channel id")
            }
            (_, Some(CODE_UNKNOWN_GUILD)) => {
                Some("Discord does not know this server, or the bot is not in it")
            }
            (_, Some(CODE_UNKNOWN_MESSAGE)) => Some("the message was deleted in Discord"),
            (_, Some(CODE_UNKNOWN_MEMBER)) => Some("that user is not a member of the server"),
            (_, Some(CODE_THREAD_ARCHIVED)) => Some("the thread is archived"),
            _ => None,
        };
        if let Some(text) = known {
            error.message = text.to_string();
        }
        error.message = redact(&error.message, &[&self.token]);
        error
    }

    fn get_json(&self, path: &str) -> Result<Value, HttpError> {
        self.execute(&Request::get(self.url(path)))
            .map_err(|f| f.error)?
            .json()
    }

    /// Sends a write; a rate limit with a short wait is slept through.
    fn send_paced(&self, request: &Request, pacing: &Pacing) -> Result<Response, ApiFailure> {
        let code = Cell::new(None);
        retry_rate_limited(pacing, || {
            self.execute(request).map_err(|f| {
                code.set(f.code);
                f.error
            })
        })
        .map_err(|error| ApiFailure {
            error,
            code: code.get(),
        })
    }

    // ----- reading ----------------------------------------------------------------------

    /// Who the bot is (`GET /users/@me`): the token is valid.
    pub fn current_user(&self) -> Result<BotUser, HttpError> {
        let v = self.get_json("/users/@me")?;
        Ok(BotUser {
            id: text_of(&v, "id").ok_or_else(|| body_error("user id"))?,
            username: text_of(&v, "username").unwrap_or_default(),
        })
    }

    pub fn guild(&self, guild_id: &str) -> Result<GuildInfo, HttpError> {
        let v = self.get_json(&format!("/guilds/{}", snowflake(guild_id)?))?;
        Ok(GuildInfo {
            id: text_of(&v, "id").unwrap_or_else(|| guild_id.to_string()),
            name: text_of(&v, "name").unwrap_or_default(),
            owner_id: text_of(&v, "owner_id").unwrap_or_default(),
        })
    }

    pub fn roles(&self, guild_id: &str) -> Result<Vec<Role>, HttpError> {
        let v = self.get_json(&format!("/guilds/{}/roles", snowflake(guild_id)?))?;
        Ok(v.as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|r| {
                        Some(Role {
                            id: text_of(r, "id")?,
                            name: text_of(r, "name").unwrap_or_default(),
                            permissions: permission_bits(r.get("permissions")),
                            mentionable: r
                                .get("mentionable")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn member(&self, guild_id: &str, user_id: &str) -> Result<Member, HttpError> {
        let v = self.get_json(&format!(
            "/guilds/{}/members/{}",
            snowflake(guild_id)?,
            snowflake(user_id)?
        ))?;
        parse_member(&v).ok_or_else(|| body_error("member"))
    }

    pub fn channel(&self, channel_id: &str) -> Result<ChannelInfo, HttpError> {
        let v = self.get_json(&format!("/channels/{}", snowflake(channel_id)?))?;
        parse_channel(&v).ok_or_else(|| body_error("channel id"))
    }

    /// Members whose username or nickname starts with `query` (`GET
    /// /guilds/{id}/members/search`, which needs no privileged intent). `limit` is kept
    /// between 1 and 1000.
    pub fn search_members(
        &self,
        guild_id: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Member>, HttpError> {
        let v = self.get_json(&format!(
            "/guilds/{}/members/search?query={}&limit={}",
            snowflake(guild_id)?,
            percent_encode(query.trim()),
            limit.clamp(1, 1000)
        ))?;
        Ok(v.as_array()
            .map(|list| list.iter().filter_map(parse_member).collect())
            .unwrap_or_default())
    }

    /// Checks the bot against a channel: who it is, the server, the channel and the
    /// permissions it has there compared with what posting needs (`thread`: also create a
    /// thread and write in it).
    pub fn check_channel(
        &self,
        guild_id: &str,
        channel_id: &str,
        thread: bool,
    ) -> Result<ChannelCheck, HttpError> {
        let bot = self.current_user()?;
        let guild = self.guild(guild_id)?;
        let roles = self.roles(guild_id)?;
        let member = self.member(guild_id, &bot.id)?;
        let channel = self.channel(channel_id)?;
        if channel.guild_id.as_deref().is_some_and(|g| g != guild.id) {
            return Err(permanent(
                ErrorKind::Status,
                "the channel is not in this server",
            ));
        }
        let permissions = perms::effective_channel_permissions(
            guild.owner_id == bot.id,
            &guild.id,
            &roles,
            &member.roles,
            &bot.id,
            &channel.overwrites,
        );
        Ok(ChannelCheck {
            missing: perms::missing(permissions, perms::required(thread)),
            bot,
            guild_name: guild.name,
            channel,
            permissions,
        })
    }

    /// Turns the failure of a post into one that names the missing permissions, when that
    /// can be found out; any other error is returned as it is. A failed check is not an error
    /// of its own: the original one is kept.
    pub fn explain_denied(
        &self,
        error: HttpError,
        guild_id: &str,
        channel_id: &str,
        thread: bool,
    ) -> HttpError {
        if error.status != Some(403) || !error.message.starts_with(MISSING_PERMISSION_TEXT) {
            return error;
        }
        match self.check_channel(guild_id, channel_id, thread) {
            Ok(check) if !check.missing.is_empty() => HttpError {
                message: format!("{MISSING_PERMISSION_TEXT}: {}", check.missing.join(", ")),
                ..error
            },
            _ => error,
        }
    }

    // ----- writing ----------------------------------------------------------------------

    /// Posts a message to a channel or a thread, with `nonce` and `enforce_nonce`: when the
    /// same nonce was already used by the bot in the channel a moment ago, Discord returns
    /// that message instead of a second one. A thread that was archived meanwhile is
    /// unarchived and the post repeated once.
    pub fn post_message(
        &self,
        channel_id: &str,
        message: &OutMessage,
        nonce: &str,
        pacing: &Pacing,
    ) -> Result<PostedMessage, HttpError> {
        let path = format!("/channels/{}/messages", snowflake(channel_id)?);
        let mut payload = message.payload();
        payload["nonce"] = json!(nonce.chars().take(MAX_NONCE_CHARS).collect::<String>());
        payload["enforce_nonce"] = json!(true);
        let (content_type, body) = payload_request_body(&payload, &message.files);
        let request = Request::new(Method::Post, self.url(&path)).bytes(&content_type, body);
        let response = match self.send_paced(&request, pacing) {
            Err(f) if f.code == Some(CODE_THREAD_ARCHIVED) => {
                self.unarchive_thread(channel_id)?;
                self.send_paced(&request, pacing)
            }
            other => other,
        }
        .map_err(|f| f.error)?;
        let id = text_of(&response.json()?, "id").ok_or_else(|| body_error("message id"))?;
        Ok(PostedMessage {
            id,
            channel_id: channel_id.to_string(),
            wait: bucket_wait(&response),
        })
    }

    /// Starts a public thread on a message of the channel. The thread has the id of the
    /// message. Discord answers `160004` when the message already has a thread; that is the
    /// goal reached, so it is success.
    pub fn create_thread(
        &self,
        channel_id: &str,
        message_id: &str,
        name: &str,
        pacing: &Pacing,
    ) -> Result<String, HttpError> {
        let path = format!(
            "/channels/{}/messages/{}/threads",
            snowflake(channel_id)?,
            snowflake(message_id)?
        );
        let request = Request::post_json(
            self.url(&path),
            &json!({
                "name": thread_name(name),
                "auto_archive_duration": THREAD_ARCHIVE_MINUTES,
                "type": CHANNEL_PUBLIC_THREAD,
            }),
        );
        match self.send_paced(&request, pacing) {
            Ok(response) => {
                // The thread has the id of its message; the answer says so too.
                Ok(text_of(&response.json()?, "id").unwrap_or_else(|| message_id.to_string()))
            }
            Err(f) if f.code == Some(CODE_THREAD_EXISTS) => Ok(message_id.to_string()),
            Err(f) => Err(f.error),
        }
    }

    /// Edits the content of a message the bot posted. A message that was deleted is an
    /// outcome, not an error. The edit is idempotent: sending it again changes nothing.
    pub fn edit_message(
        &self,
        channel_id: &str,
        message_id: &str,
        content: &str,
        allowed_mentions: &Value,
        pacing: &Pacing,
    ) -> Result<EditOutcome, HttpError> {
        let path = format!(
            "/channels/{}/messages/{}",
            snowflake(channel_id)?,
            snowflake(message_id)?
        );
        let request = Request::patch_json(
            self.url(&path),
            &json!({
                "content": content,
                "allowed_mentions": allowed_mentions,
                "flags": FLAG_SUPPRESS_EMBEDS,
            }),
        );
        let result = match self.send_paced(&request, pacing) {
            Err(f) if f.code == Some(CODE_THREAD_ARCHIVED) => {
                self.unarchive_thread(channel_id)?;
                self.send_paced(&request, pacing)
            }
            other => other,
        };
        match result {
            Ok(_) => Ok(EditOutcome::Edited),
            Err(f) if f.code == Some(CODE_UNKNOWN_MESSAGE) => Ok(EditOutcome::Deleted),
            Err(f) => Err(f.error),
        }
    }

    /// Opens an archived thread again (`PATCH /channels/{thread}`).
    pub fn unarchive_thread(&self, thread_id: &str) -> Result<(), HttpError> {
        let path = format!("/channels/{}", snowflake(thread_id)?);
        let request = Request::patch_json(self.url(&path), &json!({"archived": false}));
        self.execute(&request).map(|_| ()).map_err(|f| f.error)
    }
}

#[cfg(test)]
mod tests;

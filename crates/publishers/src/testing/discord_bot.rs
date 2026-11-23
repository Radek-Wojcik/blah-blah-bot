//! A stateful fake of the Discord bot API (REST, version 10).
//!
//! It implements what the bot publisher calls: `GET /users/@me`, the guild, its roles, the
//! bot's own member and `members/search`, channels (text, announcement and forum), posting a
//! message (JSON or multipart, with `nonce` and `enforce_nonce`), creating a thread from a
//! message, editing a message and unarchiving a thread. It checks `Authorization: Bot <token>`
//! and **enforces the permissions of the bot**: they are computed with the same algorithm as
//! the publisher's check ([`perms::effective_channel_permissions`]) from the roles, the
//! channel overwrites and the guild owner, and a call that needs a permission the bot lacks
//! gets `403` with code `50013` (or `50001` when the bot cannot even see the channel).
//!
//! The limits of the webhook fake apply to every message (2000 characters of content, 10
//! embeds, 10 files, 10 MiB of files, not empty), and Discord's error shapes and codes are
//! used: `50035` invalid form body, `10003` unknown channel, `10004` unknown guild, `10007`
//! unknown member, `10008` unknown message, `50024` wrong channel type, `50083` thread
//! archived, `160004` a thread already exists for the message.
//!
//! The ids are fixed and fictional; the fake starts with one guild, three members besides the
//! bot, a text channel, an announcement channel and a forum channel. The bot has one role
//! that holds exactly the permissions of the invite link, [`FakeDiscordBot::set_bot_permissions`]
//! changes them. Derefs to its [`FakeServer`] for faults, offline mode and the request log.

use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Value};

use super::discord::{
    parse_multipart, StoredFile, MAX_CONTENT_CHARS, MAX_EMBEDS, MAX_FILES, MAX_FILE_BYTES,
};
use super::server::{FakeServer, Recorded, Reply};
use crate::discord_bot::perms::{
    self, ATTACH_FILES, CREATE_PUBLIC_THREADS, INVITE_PERMISSIONS, SEND_MESSAGES,
    SEND_MESSAGES_IN_THREADS, VIEW_CHANNEL,
};
use crate::discord_bot::{
    Overwrite, OverwriteKind, Role, CHANNEL_ANNOUNCEMENT, CHANNEL_FORUM, CHANNEL_PUBLIC_THREAD,
    CHANNEL_TEXT, MAX_NONCE_CHARS, MAX_THREAD_NAME_CHARS,
};

/// A message the fake accepted.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredBotMessage {
    pub id: String,
    /// The channel, or the thread, it was posted to.
    pub channel_id: String,
    pub content: String,
    pub allowed_mentions: Option<Value>,
    pub embeds: usize,
    pub files: Vec<StoredFile>,
    /// The whole JSON payload as sent.
    pub payload: Value,
    pub nonce: Option<String>,
    /// How often the content was edited.
    pub edits: u32,
}

/// A thread of the fake. Its id is the id of the message it was started from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredThread {
    pub id: String,
    pub parent_id: String,
    pub name: String,
    pub archived: bool,
    pub auto_archive_duration: u64,
}

/// A member of the fake guild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeMember {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub nick: Option<String>,
    pub bot: bool,
    pub roles: Vec<String>,
}

impl FakeMember {
    pub fn new(id: &str, username: &str, global_name: Option<&str>, nick: Option<&str>) -> Self {
        Self {
            id: id.into(),
            username: username.into(),
            global_name: global_name.map(String::from),
            nick: nick.map(String::from),
            bot: false,
            roles: Vec::new(),
        }
    }
}

struct ChannelRec {
    id: String,
    name: String,
    kind: u8,
    overwrites: Vec<Overwrite>,
}

struct State {
    token: String,
    bot_id: String,
    guild_id: String,
    guild_name: String,
    owner_id: String,
    bot_role_id: String,
    roles: Vec<Role>,
    members: Vec<FakeMember>,
    channels: Vec<ChannelRec>,
    threads: Vec<StoredThread>,
    messages: Vec<StoredBotMessage>,
    /// `(channel, nonce, message id)` of the messages that carried a nonce.
    nonces: Vec<(String, String, String)>,
    next_id: u64,
}

/// The fake Discord bot API.
pub struct FakeDiscordBot {
    server: FakeServer,
    state: Arc<Mutex<State>>,
}

impl Deref for FakeDiscordBot {
    type Target = FakeServer;
    fn deref(&self) -> &FakeServer {
        &self.server
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

const GUILD_ID: &str = "400000000000000001";
const BOT_ID: &str = "500000000000000001";
const BOT_ROLE_ID: &str = "450000000000000001";
const REGULARS_ROLE_ID: &str = "450000000000000002";
const TEXT_CHANNEL_ID: &str = "300000000000000001";
const ANNOUNCEMENT_CHANNEL_ID: &str = "300000000000000002";
const FORUM_CHANNEL_ID: &str = "300000000000000003";

impl FakeDiscordBot {
    /// Starts the fake. The bot's token is [`bot_token`](Self::bot_token); its role holds the
    /// permissions of the invite link.
    pub fn start() -> FakeDiscordBot {
        let mut bot = FakeMember::new(BOT_ID, "blah-blah-bot", None, None);
        bot.bot = true;
        bot.roles = vec![BOT_ROLE_ID.to_string()];
        let state = Arc::new(Mutex::new(State {
            token: concat!("MTAwMDAwMDAwMDAwMDAwMDAx", ".GfAkE1.", "fake-Bot_token-AbC123xyz-0987654321").into(),
            bot_id: BOT_ID.into(),
            guild_id: GUILD_ID.into(),
            guild_name: "Nomad team".into(),
            owner_id: "400000000000000009".into(),
            bot_role_id: BOT_ROLE_ID.into(),
            roles: vec![
                role(GUILD_ID, "@everyone", 0, false),
                role(BOT_ROLE_ID, "Blah Blah Bot", INVITE_PERMISSIONS, false),
                role(REGULARS_ROLE_ID, "Meeting regulars", 0, true),
            ],
            members: vec![
                bot,
                FakeMember::new(
                    "600000000000000001",
                    "alex_k",
                    Some("Alex Kowal"),
                    Some("Alex"),
                ),
                FakeMember::new(
                    "600000000000000002",
                    "alexandra_n",
                    Some("Alexandra Nowak"),
                    None,
                ),
                FakeMember::new(
                    "600000000000000003",
                    "maria_l",
                    Some("Maria Lis"),
                    Some("Mia"),
                ),
            ],
            channels: vec![
                channel(TEXT_CHANNEL_ID, "meetings", CHANNEL_TEXT),
                channel(
                    ANNOUNCEMENT_CHANNEL_ID,
                    "announcements",
                    CHANNEL_ANNOUNCEMENT,
                ),
                channel(FORUM_CHANNEL_ID, "ideas", CHANNEL_FORUM),
            ],
            threads: Vec::new(),
            messages: Vec::new(),
            nonces: Vec::new(),
            next_id: 700_000_000_000_000_001,
        }));
        let shared = Arc::clone(&state);
        let server = FakeServer::start(move |req| handle(&shared, req));
        FakeDiscordBot { server, state }
    }

    /// The API base to give the publisher: `http://127.0.0.1:<port>/api/v10`.
    pub fn api_base(&self) -> String {
        format!("{}/api/v10", self.server.base_url())
    }

    /// The token the fake accepts.
    pub fn bot_token(&self) -> String {
        lock(&self.state).token.clone()
    }

    pub fn bot_id(&self) -> String {
        BOT_ID.to_string()
    }

    pub fn guild_id(&self) -> String {
        GUILD_ID.to_string()
    }

    /// A text channel.
    pub fn channel_id(&self) -> String {
        TEXT_CHANNEL_ID.to_string()
    }

    pub fn announcement_channel_id(&self) -> String {
        ANNOUNCEMENT_CHANNEL_ID.to_string()
    }

    pub fn forum_channel_id(&self) -> String {
        FORUM_CHANNEL_ID.to_string()
    }

    /// The id of the mentionable role "Meeting regulars".
    pub fn regulars_role_id(&self) -> String {
        REGULARS_ROLE_ID.to_string()
    }

    /// The messages in a channel or a thread, oldest first.
    pub fn messages(&self, channel_id: &str) -> Vec<StoredBotMessage> {
        lock(&self.state)
            .messages
            .iter()
            .filter(|m| m.channel_id == channel_id)
            .cloned()
            .collect()
    }

    /// Every thread, oldest first.
    pub fn threads(&self) -> Vec<StoredThread> {
        lock(&self.state).threads.clone()
    }

    /// Someone deletes a message in Discord.
    pub fn delete_message(&self, channel_id: &str, message_id: &str) {
        lock(&self.state)
            .messages
            .retain(|m| !(m.channel_id == channel_id && m.id == message_id));
    }

    /// The thread is archived (it was quiet for the auto-archive time).
    pub fn archive(&self, thread_id: &str) {
        if let Some(t) = lock(&self.state)
            .threads
            .iter_mut()
            .find(|t| t.id == thread_id)
        {
            t.archived = true;
        }
    }

    /// Replaces the permissions of the bot's role (the bits of the Discord documentation).
    pub fn set_bot_permissions(&self, bits: u64) {
        let mut s = lock(&self.state);
        let id = s.bot_role_id.clone();
        if let Some(r) = s.roles.iter_mut().find(|r| r.id == id) {
            r.permissions = bits;
        }
    }

    /// Makes `user_id` the owner of the guild.
    pub fn set_guild_owner(&self, user_id: &str) {
        lock(&self.state).owner_id = user_id.to_string();
    }

    /// Replaces the permission overwrites of a channel.
    pub fn set_overwrites(&self, channel_id: &str, overwrites: Vec<Overwrite>) {
        if let Some(c) = lock(&self.state)
            .channels
            .iter_mut()
            .find(|c| c.id == channel_id)
        {
            c.overwrites = overwrites;
        }
    }

    /// Adds a channel of a kind (`0` text, `5` announcement, `15` forum).
    pub fn add_channel(&self, id: &str, name: &str, kind: u8) {
        lock(&self.state).channels.push(channel(id, name, kind));
    }

    pub fn add_member(&self, member: FakeMember) {
        lock(&self.state).members.push(member);
    }

    /// The nonces are forgotten, as when Discord's short dedup window has passed.
    pub fn forget_nonces(&self) {
        lock(&self.state).nonces.clear();
    }
}

fn role(id: &str, name: &str, permissions: u64, mentionable: bool) -> Role {
    Role {
        id: id.into(),
        name: name.into(),
        permissions,
        mentionable,
    }
}

fn channel(id: &str, name: &str, kind: u8) -> ChannelRec {
    ChannelRec {
        id: id.into(),
        name: name.into(),
        kind,
        overwrites: Vec::new(),
    }
}

// ----- answers ---------------------------------------------------------------------------

fn api_error(status: u16, code: u32, message: &str) -> Reply {
    Reply::json(status, &json!({"message": message, "code": code}))
}

fn invalid_form(field: &str, rule: &str, message: &str) -> Reply {
    Reply::json(
        400,
        &json!({
            "message": "Invalid Form Body",
            "code": 50035,
            "errors": {field: {"_errors": [{"code": rule, "message": message}]}},
        }),
    )
}

fn missing_access() -> Reply {
    api_error(403, 50001, "Missing Access")
}

fn missing_permissions() -> Reply {
    api_error(403, 50013, "Missing Permissions")
}

fn unknown_message() -> Reply {
    api_error(404, 10008, "Unknown Message")
}

fn user_json(m: &FakeMember) -> Value {
    let mut user = json!({"id": m.id, "username": m.username, "bot": m.bot});
    if let Some(g) = &m.global_name {
        user["global_name"] = json!(g);
    }
    user
}

fn member_json(m: &FakeMember) -> Value {
    let mut v = json!({"user": user_json(m), "roles": m.roles});
    if let Some(n) = &m.nick {
        v["nick"] = json!(n);
    }
    v
}

fn message_json(state: &State, m: &StoredBotMessage) -> Value {
    let mut v = json!({
        "id": m.id,
        "type": 0,
        "channel_id": m.channel_id,
        "content": m.content,
        "author": {"id": state.bot_id, "username": "blah-blah-bot", "bot": true},
        "attachments": m.files.iter().enumerate().map(|(i, f)| json!({
            "id": format!("{i}"), "filename": f.filename, "size": f.size,
        })).collect::<Vec<_>>(),
        "embeds": m.payload.get("embeds").cloned().unwrap_or_else(|| json!([])),
    });
    if let Some(n) = &m.nonce {
        v["nonce"] = json!(n);
    }
    v
}

fn overwrite_json(o: &Overwrite) -> Value {
    json!({
        "id": o.id,
        "type": if o.kind == OverwriteKind::Member { 1 } else { 0 },
        "allow": o.allow.to_string(),
        "deny": o.deny.to_string(),
    })
}

/// Where a channel id points: a channel, or a thread with the overwrites of its parent.
struct Located {
    id: String,
    name: String,
    kind: u8,
    overwrites: Vec<Overwrite>,
    thread: Option<StoredThread>,
}

fn locate(state: &State, id: &str) -> Option<Located> {
    if let Some(c) = state.channels.iter().find(|c| c.id == id) {
        return Some(Located {
            id: c.id.clone(),
            name: c.name.clone(),
            kind: c.kind,
            overwrites: c.overwrites.clone(),
            thread: None,
        });
    }
    let thread = state.threads.iter().find(|t| t.id == id)?;
    let parent = state.channels.iter().find(|c| c.id == thread.parent_id)?;
    Some(Located {
        id: thread.id.clone(),
        name: thread.name.clone(),
        kind: CHANNEL_PUBLIC_THREAD,
        overwrites: parent.overwrites.clone(),
        thread: Some(thread.clone()),
    })
}

fn bot_permissions(state: &State, at: &Located) -> u64 {
    let roles = state
        .members
        .iter()
        .find(|m| m.id == state.bot_id)
        .map(|m| m.roles.clone())
        .unwrap_or_default();
    perms::effective_channel_permissions(
        state.owner_id == state.bot_id,
        &state.guild_id,
        &state.roles,
        &roles,
        &state.bot_id,
        &at.overwrites,
    )
}

fn channel_json(state: &State, at: &Located) -> Value {
    let mut v = json!({
        "id": at.id,
        "type": at.kind,
        "guild_id": state.guild_id,
        "name": at.name,
        "permission_overwrites": at.overwrites.iter().map(overwrite_json).collect::<Vec<_>>(),
    });
    if let Some(t) = &at.thread {
        v["parent_id"] = json!(t.parent_id);
        v["thread_metadata"] = json!({
            "archived": t.archived,
            "auto_archive_duration": t.auto_archive_duration,
        });
    }
    v
}

// ----- routing ---------------------------------------------------------------------------

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_value(req: &Recorded, name: &str) -> Option<String> {
    req.query_param(name).map(percent_decode)
}

fn handle(shared: &Mutex<State>, req: &Recorded) -> Reply {
    let mut state = lock(shared);
    if req.header("authorization") != Some(format!("Bot {}", state.token).as_str()) {
        return api_error(401, 0, "401: Unauthorized");
    }
    let Some(path) = req.path.strip_prefix("/api/v10") else {
        return api_error(404, 0, "404: Not Found");
    };
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["users", "@me"]) => Reply::json(
            200,
            &json!({"id": state.bot_id, "username": "blah-blah-bot", "bot": true}),
        ),
        ("GET", ["guilds", g]) => guild_scope(&state, g, |s| {
            Reply::json(
                200,
                &json!({"id": s.guild_id, "name": s.guild_name, "owner_id": s.owner_id}),
            )
        }),
        ("GET", ["guilds", g, "roles"]) => guild_scope(&state, g, |s| {
            Reply::json(
                200,
                &Value::Array(
                    s.roles
                        .iter()
                        .enumerate()
                        .map(|(i, r)| {
                            json!({
                                "id": r.id, "name": r.name, "position": i,
                                "permissions": r.permissions.to_string(),
                                "mentionable": r.mentionable,
                            })
                        })
                        .collect(),
                ),
            )
        }),
        ("GET", ["guilds", g, "members", "search"]) => {
            guild_scope(&state, g, |s| search_members(s, req))
        }
        ("GET", ["guilds", g, "members", id]) => guild_scope(&state, g, |s| {
            match s.members.iter().find(|m| m.id == *id) {
                Some(m) => Reply::json(200, &member_json(m)),
                None => api_error(404, 10007, "Unknown Member"),
            }
        }),
        ("GET", ["channels", c]) => get_channel(&state, c),
        ("PATCH", ["channels", c]) => patch_channel(&mut state, c, req),
        ("POST", ["channels", c, "messages"]) => post_message(&mut state, c, req),
        ("PATCH", ["channels", c, "messages", m]) => edit_message(&mut state, c, m, req),
        ("POST", ["channels", c, "messages", m, "threads"]) => create_thread(&mut state, c, m, req),
        _ => api_error(404, 0, "404: Not Found"),
    }
}

fn guild_scope(state: &State, guild: &str, then: impl FnOnce(&State) -> Reply) -> Reply {
    if guild != state.guild_id {
        return api_error(404, 10004, "Unknown Guild");
    }
    then(state)
}

fn search_members(state: &State, req: &Recorded) -> Reply {
    let Some(query) = query_value(req, "query").filter(|q| !q.is_empty()) else {
        return invalid_form("query", "BASE_TYPE_REQUIRED", "This field is required");
    };
    let limit = match query_value(req, "limit") {
        None => 1,
        Some(l) => match l.parse::<usize>() {
            Ok(n) if (1..=1000).contains(&n) => n,
            _ => return invalid_form("limit", "NUMBER_TYPE_MAX", "Must be between 1 and 1000."),
        },
    };
    // Prefix match on username or nickname, as Discord documents; no privileged intent.
    let query = query.to_lowercase();
    let starts = |s: &str| s.to_lowercase().starts_with(&query);
    let hits: Vec<Value> = state
        .members
        .iter()
        .filter(|m| starts(&m.username) || m.nick.as_deref().is_some_and(starts))
        .take(limit)
        .map(member_json)
        .collect();
    Reply::json(200, &Value::Array(hits))
}

fn get_channel(state: &State, id: &str) -> Reply {
    let Some(at) = locate(state, id) else {
        return api_error(404, 10003, "Unknown Channel");
    };
    if bot_permissions(state, &at) & VIEW_CHANNEL == 0 {
        return missing_access();
    }
    Reply::json(200, &channel_json(state, &at))
}

fn patch_channel(state: &mut State, id: &str, req: &Recorded) -> Reply {
    let Some(at) = locate(state, id) else {
        return api_error(404, 10003, "Unknown Channel");
    };
    let perms = bot_permissions(state, &at);
    if perms & VIEW_CHANNEL == 0 {
        return missing_access();
    }
    if at.thread.is_none() {
        return api_error(403, 50013, "Missing Permissions");
    }
    if perms & (SEND_MESSAGES | SEND_MESSAGES_IN_THREADS) == 0 {
        return missing_permissions();
    }
    let Some(body) = req.json().filter(Value::is_object) else {
        return api_error(400, 50109, "The request body contains invalid JSON.");
    };
    if let Some(archived) = body.get("archived").and_then(Value::as_bool) {
        if let Some(t) = state.threads.iter_mut().find(|t| t.id == id) {
            t.archived = archived;
        }
    }
    let at = locate(state, id).expect("the thread exists");
    Reply::json(200, &channel_json(state, &at))
}

/// The JSON payload of a message request and its files.
fn request_payload(req: &Recorded) -> Result<(Value, Vec<StoredFile>), Reply> {
    let content_type = req.header("content-type").unwrap_or("");
    if content_type.starts_with("multipart/form-data") {
        parse_multipart(content_type, &req.body)
            .ok_or_else(|| api_error(400, 50035, "Invalid Form Body"))
    } else if content_type.starts_with("application/json") {
        match req.json() {
            Some(v) if v.is_object() => Ok((v, Vec::new())),
            _ => Err(api_error(
                400,
                50109,
                "The request body contains invalid JSON.",
            )),
        }
    } else {
        Err(api_error(400, 50035, "Invalid Form Body"))
    }
}

fn check_content(content: &str, embeds: usize) -> Option<Reply> {
    if content.chars().count() > MAX_CONTENT_CHARS {
        return Some(invalid_form(
            "content",
            "BASE_TYPE_MAX_LENGTH",
            "Must be 2000 or fewer in length.",
        ));
    }
    if embeds > MAX_EMBEDS {
        return Some(invalid_form(
            "embeds",
            "BASE_TYPE_MAX_LENGTH",
            "Must be 10 or fewer in length.",
        ));
    }
    None
}

fn post_message(state: &mut State, channel_id: &str, req: &Recorded) -> Reply {
    let Some(at) = locate(state, channel_id) else {
        return api_error(404, 10003, "Unknown Channel");
    };
    let perms = bot_permissions(state, &at);
    if perms & VIEW_CHANNEL == 0 {
        return missing_access();
    }
    match &at.thread {
        Some(thread) => {
            if perms & SEND_MESSAGES_IN_THREADS == 0 {
                return missing_permissions();
            }
            if thread.archived {
                return api_error(400, 50083, "Thread is archived");
            }
        }
        None => {
            if at.kind == CHANNEL_FORUM {
                return api_error(400, 50024, "Cannot execute action on this channel type");
            }
            if perms & SEND_MESSAGES == 0 {
                return missing_permissions();
            }
        }
    }
    let (payload, files) = match request_payload(req) {
        Ok(parsed) => parsed,
        Err(reply) => return reply,
    };
    if !files.is_empty() && perms & ATTACH_FILES == 0 {
        return missing_permissions();
    }
    let content = payload
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let embeds = payload
        .get("embeds")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if let Some(reply) = check_content(&content, embeds) {
        return reply;
    }
    if files.len() > MAX_FILES {
        return invalid_form(
            "files",
            "BASE_TYPE_MAX_LENGTH",
            "Must be 10 or fewer in length.",
        );
    }
    if files.iter().map(|f| f.size).sum::<usize>() > MAX_FILE_BYTES {
        return api_error(413, 40005, "Request entity too large");
    }
    if content.trim().is_empty() && embeds == 0 && files.is_empty() {
        return api_error(400, 50006, "Cannot send an empty message");
    }
    let nonce = match payload.get("nonce") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    if nonce
        .as_deref()
        .is_some_and(|n| n.chars().count() > MAX_NONCE_CHARS)
    {
        return invalid_form(
            "nonce",
            "BASE_TYPE_MAX_LENGTH",
            "Must be 25 or fewer in length.",
        );
    }
    let enforce = payload
        .get("enforce_nonce")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let (true, Some(nonce)) = (enforce, &nonce) {
        let earlier = state
            .nonces
            .iter()
            .find(|(c, n, _)| c == channel_id && n == nonce)
            .map(|(_, _, id)| id.clone());
        let existing = earlier.and_then(|id| state.messages.iter().find(|m| m.id == id));
        if let Some(existing) = existing {
            return Reply::json(200, &message_json(state, existing));
        }
    }

    let id = state.next_id.to_string();
    state.next_id += 1;
    if let Some(nonce) = &nonce {
        state
            .nonces
            .push((channel_id.to_string(), nonce.clone(), id.clone()));
    }
    let message = StoredBotMessage {
        id,
        channel_id: channel_id.to_string(),
        content,
        allowed_mentions: payload.get("allowed_mentions").cloned(),
        embeds,
        files,
        payload,
        nonce,
        edits: 0,
    };
    let reply = Reply::json(200, &message_json(state, &message));
    state.messages.push(message);
    reply
}

fn edit_message(state: &mut State, channel_id: &str, message_id: &str, req: &Recorded) -> Reply {
    let Some(at) = locate(state, channel_id) else {
        return api_error(404, 10003, "Unknown Channel");
    };
    if bot_permissions(state, &at) & VIEW_CHANNEL == 0 {
        return missing_access();
    }
    if at.thread.as_ref().is_some_and(|t| t.archived) {
        return api_error(400, 50083, "Thread is archived");
    }
    let Some(index) = state
        .messages
        .iter()
        .position(|m| m.id == message_id && m.channel_id == channel_id)
    else {
        return unknown_message();
    };
    let Some(body) = req.json().filter(Value::is_object) else {
        return api_error(400, 50109, "The request body contains invalid JSON.");
    };
    let content = body.get("content").and_then(Value::as_str);
    if let Some(reply) = check_content(content.unwrap_or(""), 0) {
        return reply;
    }
    if content.is_some_and(|c| c.trim().is_empty()) && state.messages[index].files.is_empty() {
        return api_error(400, 50006, "Cannot send an empty message");
    }
    let message = &mut state.messages[index];
    if let Some(content) = content {
        message.content = content.to_string();
    }
    if let Some(mentions) = body.get("allowed_mentions") {
        message.allowed_mentions = Some(mentions.clone());
    }
    message.edits += 1;
    let message = message.clone();
    Reply::json(200, &message_json(state, &message))
}

fn create_thread(state: &mut State, channel_id: &str, message_id: &str, req: &Recorded) -> Reply {
    let Some(at) = locate(state, channel_id) else {
        return api_error(404, 10003, "Unknown Channel");
    };
    let perms = bot_permissions(state, &at);
    if perms & VIEW_CHANNEL == 0 {
        return missing_access();
    }
    if !matches!(at.kind, CHANNEL_TEXT | CHANNEL_ANNOUNCEMENT) {
        return api_error(400, 50024, "Cannot execute action on this channel type");
    }
    if perms & CREATE_PUBLIC_THREADS == 0 {
        return missing_permissions();
    }
    if !state
        .messages
        .iter()
        .any(|m| m.id == message_id && m.channel_id == channel_id)
    {
        return unknown_message();
    }
    if state.threads.iter().any(|t| t.id == message_id) {
        return api_error(
            400,
            160_004,
            "A thread has already been created for this message",
        );
    }
    let Some(body) = req.json().filter(Value::is_object) else {
        return api_error(400, 50109, "The request body contains invalid JSON.");
    };
    let name = body.get("name").and_then(Value::as_str).unwrap_or("");
    let length = name.chars().count();
    if length == 0 || length > MAX_THREAD_NAME_CHARS {
        return invalid_form(
            "name",
            "BASE_TYPE_BAD_LENGTH",
            "Must be between 1 and 100 in length.",
        );
    }
    let thread = StoredThread {
        id: message_id.to_string(),
        parent_id: channel_id.to_string(),
        name: name.to_string(),
        archived: false,
        auto_archive_duration: body
            .get("auto_archive_duration")
            .and_then(Value::as_u64)
            .unwrap_or(4320),
    };
    state.threads.push(thread);
    let created = locate(state, message_id).expect("the thread was just stored");
    Reply::json(201, &channel_json(state, &created))
}

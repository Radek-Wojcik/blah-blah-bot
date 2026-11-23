//! A stateful fake of the Notion API.
//!
//! It implements what the publisher needs: `GET /v1/users/me`, `GET /v1/databases/{id}`
//! (with its data sources), `GET /v1/data_sources/{id}` (the schema),
//! `POST /v1/data_sources/{id}/query` (a `rich_text` `equals` filter, `created_time` sorts,
//! `page_size`), `POST /v1/pages`, `GET /v1/pages/{id}` and `PATCH /v1/pages/{id}`.
//!
//! And it enforces what Notion enforces, with the shapes and codes of its errors:
//!
//! * `Authorization: Bearer <token>` (`401 unauthorized`) and the `Notion-Version` of the
//!   publisher (`400 missing_version` when it is absent or another one);
//! * the schema: a property that does not exist or a value of another type is a
//!   `400 validation_error` that names the property;
//! * a text of at most 2000 characters, a select name without a comma and of at most 100
//!   characters, a status that is one of its options, a date that is a date;
//! * a trashed page (`in_trash`) is not part of a query, answers a `GET`, and cannot be edited;
//! * [`share(false)`](FakeNotion::share) makes the database, its data sources and pages
//!   answer `404 object_not_found`, as an integration that was never added to the database
//!   sees them;
//! * [`lag_queries(n)`](FakeNotion::lag_queries): pages created afterwards are invisible to
//!   the next `n` queries (the lag of a query after a creation, which Notion does not rule
//!   out).
//!
//! Faults come from the [`FakeServer`] it derefs to; [`conflict`] and [`rate_limited`] build
//! the answers of a `409` and a `429`.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Map, Value};

use super::server::{FakeServer, Recorded, Reply};
use crate::notion::NOTION_VERSION;

/// The database the fake starts with.
pub const DATABASE_ID: &str = "0123456789abcdef0123456789abcdef";
/// Its data source.
pub const DATA_SOURCE_ID: &str = "fedcba98-7654-3210-fedc-ba9876543210";
/// The token the fake accepts until [`FakeNotion::require_token`] changes it.
pub const DEFAULT_TOKEN: &str = concat!("ntn_", "FakeNotionTokenForTests0123456789abcdef");
/// Most characters of one text.
pub const MAX_TEXT_CHARS: usize = 2000;
/// Most characters of the name of a select option.
pub const MAX_OPTION_CHARS: usize = 100;

/// A page the fake holds.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredPage {
    pub id: String,
    pub data_source: String,
    pub url: String,
    pub in_trash: bool,
    /// The properties by name, each `{"type": t, t: value}`.
    pub properties: BTreeMap<String, Value>,
}

impl StoredPage {
    /// The text of a title or rich text property.
    pub fn text(&self, property: &str) -> Option<String> {
        let value = self.properties.get(property)?;
        let kind = value.get("type")?.as_str()?;
        let parts = value.get(kind)?.as_array()?;
        Some(
            parts
                .iter()
                .filter_map(|p| p.pointer("/text/content").and_then(Value::as_str))
                .collect(),
        )
    }

    /// The name of a select or status option.
    pub fn option(&self, property: &str) -> Option<String> {
        let value = self.properties.get(property)?;
        let kind = value.get("type")?.as_str()?;
        value
            .get(kind)?
            .get("name")
            .and_then(Value::as_str)
            .map(String::from)
    }

    /// The start of a date property.
    pub fn date(&self, property: &str) -> Option<String> {
        self.properties
            .get(property)?
            .pointer("/date/start")
            .and_then(Value::as_str)
            .map(String::from)
    }

    /// The address of a url property.
    pub fn url_value(&self, property: &str) -> Option<String> {
        self.properties
            .get(property)?
            .get("url")?
            .as_str()
            .map(String::from)
    }
}

struct Source {
    id: String,
    name: String,
    /// Property name -> type name.
    schema: BTreeMap<String, String>,
    /// Property name -> option names (select, multi_select, status).
    options: BTreeMap<String, Vec<String>>,
}

struct Page {
    stored: StoredPage,
    seq: usize,
    /// Queries that still do not see the page.
    hidden_for: usize,
}

struct State {
    token: String,
    shared: bool,
    database_title: String,
    sources: Vec<Source>,
    pages: Vec<Page>,
    next_page: usize,
    lag: usize,
}

/// The fake Notion. Derefs to its [`FakeServer`] for faults, offline mode and the log of
/// requests.
pub struct FakeNotion {
    server: FakeServer,
    state: Arc<Mutex<State>>,
}

impl Deref for FakeNotion {
    type Target = FakeServer;
    fn deref(&self) -> &FakeServer {
        &self.server
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The id without dashes, lower case: how the fake compares ids.
fn flat(id: &str) -> String {
    id.chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// `409 conflict_error`.
pub fn conflict() -> Reply {
    error(
        409,
        "conflict_error",
        "Conflict occurred while saving. Please try again.",
    )
}

/// `429 rate_limited` with the wait in `Retry-After`.
pub fn rate_limited(retry_after_secs: u64) -> Reply {
    error(
        429,
        "rate_limited",
        "You have been rate limited. Please try again in a few minutes.",
    )
    .with_header("Retry-After", &retry_after_secs.to_string())
}

fn error(status: u16, code: &str, message: &str) -> Reply {
    Reply::json(
        status,
        &json!({"object": "error", "status": status, "code": code, "message": message}),
    )
}

fn validation(message: &str) -> Reply {
    error(400, "validation_error", message)
}

/// The data source of the default database: a Notion task list.
fn default_source(id: &str, name: &str) -> Source {
    let mut schema = BTreeMap::new();
    for (property, kind) in [
        ("Name", "title"),
        ("BBB key", "rich_text"),
        ("Assignee", "select"),
        ("Due", "date"),
        ("Status", "status"),
        ("Meeting", "url"),
        ("Cycle", "rich_text"),
    ] {
        schema.insert(property.to_string(), kind.to_string());
    }
    let mut options = BTreeMap::new();
    options.insert(
        "Status".to_string(),
        ["Not started", "In progress", "Done"]
            .map(String::from)
            .to_vec(),
    );
    Source {
        id: id.to_string(),
        name: name.to_string(),
        schema,
        options,
    }
}

impl FakeNotion {
    /// Starts the fake with one database (`DATABASE_ID`) that has one data source
    /// (`DATA_SOURCE_ID`) with the properties Name (title), BBB key (rich_text), Assignee
    /// (select), Due (date), Status (status: Not started, In progress, Done), Meeting (url)
    /// and Cycle (rich_text), and accepts [`DEFAULT_TOKEN`].
    pub fn start() -> FakeNotion {
        let state = Arc::new(Mutex::new(State {
            token: DEFAULT_TOKEN.to_string(),
            shared: true,
            database_title: "Tasks".to_string(),
            sources: vec![default_source(DATA_SOURCE_ID, "Tasks")],
            pages: Vec::new(),
            next_page: 1,
            lag: 0,
        }));
        let shared = Arc::clone(&state);
        let server = FakeServer::start(move |req| handle(&shared, req));
        FakeNotion { server, state }
    }

    /// `http://127.0.0.1:<port>/v1`: the API base to give the client.
    pub fn api_base(&self) -> String {
        format!("{}/v1", self.server.base_url())
    }

    /// The token the fake accepts.
    pub fn token(&self) -> String {
        lock(&self.state).token.clone()
    }

    /// Accept this token instead.
    pub fn require_token(&self, token: &str) {
        lock(&self.state).token = token.to_string();
    }

    /// Adds a second data source to the database, with the same properties as the first.
    /// Returns its id.
    pub fn add_data_source(&self, name: &str) -> String {
        let mut s = lock(&self.state);
        let id = format!("aaaaaaaa-0000-4000-8000-{:012x}", s.sources.len() + 1);
        s.sources.push(default_source(&id, name));
        id
    }

    /// Removes a data source (removing the only one leaves a database without any).
    pub fn remove_data_source(&self, id: &str) {
        lock(&self.state)
            .sources
            .retain(|s| flat(&s.id) != flat(id));
    }

    /// Replaces the properties of a data source: `(name, type)` pairs, with the types as
    /// the API names them (`title`, `rich_text`, `select`, `multi_select`, `date`, `status`,
    /// `url`, `people`, `number`, `checkbox`). A status property gets the options Not
    /// started, In progress and Done.
    pub fn set_schema(&self, data_source: &str, properties: &[(&str, &str)]) {
        let mut s = lock(&self.state);
        if let Some(source) = s
            .sources
            .iter_mut()
            .find(|x| flat(&x.id) == flat(data_source))
        {
            source.schema = properties
                .iter()
                .map(|(n, t)| ((*n).to_string(), (*t).to_string()))
                .collect();
            source.options = properties
                .iter()
                .filter(|(_, t)| *t == "status")
                .map(|(n, _)| {
                    (
                        (*n).to_string(),
                        ["Not started", "In progress", "Done"]
                            .map(String::from)
                            .to_vec(),
                    )
                })
                .collect();
        }
    }

    /// Whether the database is shared with the integration. When it is not, the database, its
    /// data sources and its pages answer `404 object_not_found`.
    pub fn share(&self, shared: bool) {
        lock(&self.state).shared = shared;
    }

    /// Pages created from now on are invisible to the next `queries` queries.
    pub fn lag_queries(&self, queries: usize) {
        lock(&self.state).lag = queries;
    }

    /// Every page, trashed ones included, oldest first.
    pub fn pages(&self) -> Vec<StoredPage> {
        lock(&self.state)
            .pages
            .iter()
            .map(|p| p.stored.clone())
            .collect()
    }

    /// A person moves a page to the trash.
    pub fn trash(&self, page: &str) {
        let mut s = lock(&self.state);
        if let Some(p) = s
            .pages
            .iter_mut()
            .find(|p| flat(&p.stored.id) == flat(page))
        {
            p.stored.in_trash = true;
        }
    }

    /// A person sets a property in Notion, bypassing the checks of the API. `value` is the
    /// value as the API writes it, for example `{"status": {"name": "Done"}}`.
    pub fn set_property(&self, page: &str, property: &str, value: Value) {
        let mut s = lock(&self.state);
        if let Some(p) = s
            .pages
            .iter_mut()
            .find(|p| flat(&p.stored.id) == flat(page))
        {
            p.stored
                .properties
                .insert(property.to_string(), typed(&value));
        }
    }
}

/// `{"select": ..}` as `{"type": "select", "select": ..}`.
fn typed(value: &Value) -> Value {
    let mut out = Map::new();
    if let Some((kind, inner)) = value.as_object().and_then(|o| o.iter().next()) {
        out.insert("type".into(), json!(kind));
        out.insert(kind.clone(), inner.clone());
    }
    Value::Object(out)
}

// ----- the API ---------------------------------------------------------------------------

fn not_found() -> Reply {
    error(
        404,
        "object_not_found",
        "Could not find the object. Make sure the object exists and is shared with your integration.",
    )
}

fn handle(shared: &Mutex<State>, req: &Recorded) -> Reply {
    let mut state = lock(shared);
    if req.header("notion-version") != Some(NOTION_VERSION) {
        return error(
            400,
            "missing_version",
            &format!("Notion-Version header failed validation: it must be {NOTION_VERSION}."),
        );
    }
    let expected = format!("Bearer {}", state.token);
    if req.header("authorization") != Some(expected.as_str()) {
        return error(401, "unauthorized", "API token is invalid.");
    }
    let segments: Vec<&str> = req.path.trim_matches('/').split('/').collect();
    let method = req.method.as_str();
    match (method, segments.as_slice()) {
        ("GET", ["v1", "users", "me"]) => Reply::json(
            200,
            &json!({
                "object": "user", "id": "11111111-2222-4333-8444-555555555555",
                "type": "bot", "name": "Blah Blah Bot test",
                "bot": {"workspace_name": "Nomad workspace"},
            }),
        ),
        _ if !state.shared => not_found(),
        ("GET", ["v1", "databases", id]) => get_database(&state, id),
        ("GET", ["v1", "data_sources", id]) => get_data_source(&state, id),
        ("POST", ["v1", "data_sources", id, "query"]) => query(&mut state, id, req),
        ("POST", ["v1", "pages"]) => create_page(&mut state, req),
        ("GET", ["v1", "pages", id]) => match find_page(&state, id) {
            Some(i) => Reply::json(200, &page_json(&state.pages[i].stored)),
            None => not_found(),
        },
        ("PATCH", ["v1", "pages", id]) => update_page(&mut state, id, req),
        _ => error(404, "invalid_request_url", "Invalid request URL."),
    }
}

fn find_page(state: &State, id: &str) -> Option<usize> {
    state
        .pages
        .iter()
        .position(|p| flat(&p.stored.id) == flat(id))
}

fn get_database(state: &State, id: &str) -> Reply {
    if flat(id) != flat(DATABASE_ID) {
        return not_found();
    }
    let sources: Vec<Value> = state
        .sources
        .iter()
        .map(|s| json!({"id": s.id, "name": s.name}))
        .collect();
    Reply::json(
        200,
        &json!({
            "object": "database", "id": DATABASE_ID,
            "title": [{"type": "text", "text": {"content": state.database_title},
                       "plain_text": state.database_title}],
            "data_sources": sources,
        }),
    )
}

fn get_data_source(state: &State, id: &str) -> Reply {
    let Some(source) = state.sources.iter().find(|s| flat(&s.id) == flat(id)) else {
        return not_found();
    };
    let mut properties = Map::new();
    for (name, kind) in &source.schema {
        let mut property = json!({"id": flat(name), "name": name, "type": kind});
        let options: Vec<Value> = source
            .options
            .get(name)
            .map(|o| o.iter().map(|n| json!({"name": n})).collect())
            .unwrap_or_default();
        property[kind] = match kind.as_str() {
            "select" | "multi_select" => json!({"options": options}),
            "status" => json!({"options": options, "groups": []}),
            _ => json!({}),
        };
        properties.insert(name.clone(), property);
    }
    Reply::json(
        200,
        &json!({"object": "data_source", "id": source.id, "properties": properties}),
    )
}

fn page_json(page: &StoredPage) -> Value {
    let mut properties = Map::new();
    for (name, value) in &page.properties {
        let mut value = value.clone();
        // Texts carry `plain_text` on the way out, as in Notion.
        if let Some(kind) = value.get("type").and_then(Value::as_str).map(String::from) {
            if matches!(kind.as_str(), "title" | "rich_text") {
                if let Some(parts) = value.get_mut(&kind).and_then(Value::as_array_mut) {
                    for part in parts {
                        let content = part
                            .pointer("/text/content")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        part["type"] = json!("text");
                        part["plain_text"] = json!(content);
                    }
                }
            }
        }
        properties.insert(name.clone(), value);
    }
    json!({
        "object": "page", "id": page.id, "created_time": "2026-11-13T10:00:00.000Z",
        "in_trash": page.in_trash, "url": page.url, "properties": properties,
    })
}

// ----- queries ---------------------------------------------------------------------------

fn query(state: &mut State, id: &str, req: &Recorded) -> Reply {
    let Some(source) = state.sources.iter().find(|s| flat(&s.id) == flat(id)) else {
        return not_found();
    };
    let source_id = source.id.clone();
    let schema = source.schema.clone();
    let Some(body) = req.json().filter(Value::is_object) else {
        return validation("body failed validation: body should be an object.");
    };
    let mut wanted: Option<(String, String)> = None;
    if let Some(filter) = body.get("filter") {
        let property = filter.get("property").and_then(Value::as_str).unwrap_or("");
        match schema.get(property).map(String::as_str) {
            None => {
                return validation(&format!(
                    "Could not find property with name or id: {property}"
                ))
            }
            Some("rich_text") => {}
            Some(other) => {
                return validation(&format!(
                    "The provided filter type ({other}) does not match the property type of {property}."
                ))
            }
        }
        match filter.pointer("/rich_text/equals").and_then(Value::as_str) {
            Some(text) => wanted = Some((property.to_string(), text.to_string())),
            None => return validation("Only the rich_text equals filter is supported."),
        }
    }
    let page_size = body
        .get("page_size")
        .and_then(Value::as_u64)
        .map_or(100, |n| n.clamp(1, 100) as usize);
    let descending =
        body.pointer("/sorts/0/direction").and_then(Value::as_str) == Some("descending");

    let mut hits: Vec<(usize, Value)> = Vec::new();
    for page in &state.pages {
        if page.stored.data_source != source_id || page.stored.in_trash || page.hidden_for > 0 {
            continue;
        }
        if let Some((property, text)) = &wanted {
            if page.stored.text(property).as_deref() != Some(text.as_str()) {
                continue;
            }
        }
        hits.push((page.seq, page_json(&page.stored)));
    }
    hits.sort_by_key(|(seq, _)| *seq);
    if descending {
        hits.reverse();
    }
    // The lag counts queries: every query ages the pages that are still hidden.
    for page in &mut state.pages {
        page.hidden_for = page.hidden_for.saturating_sub(1);
    }
    let has_more = hits.len() > page_size;
    let results: Vec<Value> = hits.into_iter().take(page_size).map(|(_, v)| v).collect();
    Reply::json(
        200,
        &json!({
            "object": "list", "results": results, "has_more": has_more,
            "next_cursor": null, "type": "page", "page": {},
        }),
    )
}

// ----- writes ----------------------------------------------------------------------------

fn create_page(state: &mut State, req: &Recorded) -> Reply {
    let Some(body) = req.json().filter(Value::is_object) else {
        return validation("body failed validation: body should be an object.");
    };
    let Some(parent_id) = body
        .pointer("/parent/data_source_id")
        .and_then(Value::as_str)
    else {
        return validation(
            "body.parent.data_source_id should be defined, instead was `undefined`.",
        );
    };
    let Some(source) = state
        .sources
        .iter_mut()
        .find(|s| flat(&s.id) == flat(parent_id))
    else {
        return not_found();
    };
    let Some(properties) = body.get("properties").and_then(Value::as_object) else {
        return validation("body.properties should be defined, instead was `undefined`.");
    };
    let written = match validate_properties(source, properties) {
        Ok(w) => w,
        Err(reply) => return reply,
    };
    if !source.schema.values().any(|k| k == "title") {
        return validation("The data source has no title property.");
    }
    let data_source = source.id.clone();
    let n = state.next_page;
    state.next_page += 1;
    let id = format!("c0ffee00-0000-4000-8000-{n:012x}");
    let stored = StoredPage {
        url: format!("https://www.notion.so/{}", flat(&id)),
        id,
        data_source,
        in_trash: false,
        properties: written,
    };
    let reply = Reply::json(200, &page_json(&stored));
    let lag = state.lag;
    state.pages.push(Page {
        stored,
        seq: n,
        hidden_for: lag,
    });
    reply
}

fn update_page(state: &mut State, id: &str, req: &Recorded) -> Reply {
    let Some(index) = find_page(state, id) else {
        return not_found();
    };
    if state.pages[index].stored.in_trash {
        return validation("Can't edit page on block with an archived ancestor.");
    }
    let Some(body) = req.json().filter(Value::is_object) else {
        return validation("body failed validation: body should be an object.");
    };
    let Some(properties) = body.get("properties").and_then(Value::as_object) else {
        return validation("body.properties should be defined, instead was `undefined`.");
    };
    let source_id = state.pages[index].stored.data_source.clone();
    let Some(source) = state.sources.iter_mut().find(|s| s.id == source_id) else {
        return not_found();
    };
    let written = match validate_properties(source, properties) {
        Ok(w) => w,
        Err(reply) => return reply,
    };
    let page = &mut state.pages[index].stored;
    page.properties.extend(written);
    Reply::json(200, &page_json(page))
}

/// Checks written properties against the schema and returns them in their stored form. A
/// select name that does not exist yet becomes an option, as in Notion.
fn validate_properties(
    source: &mut Source,
    properties: &Map<String, Value>,
) -> Result<BTreeMap<String, Value>, Reply> {
    let mut out = BTreeMap::new();
    for (name, value) in properties {
        let Some(kind) = source.schema.get(name).cloned() else {
            return Err(validation(&format!(
                "{name} is not a property that exists."
            )));
        };
        let inner = value
            .as_object()
            .and_then(|o| o.get(&kind))
            .filter(|_| value.as_object().is_some_and(|o| o.len() == 1));
        let Some(inner) = inner else {
            return Err(validation(&format!("{name} is expected to be {kind}.")));
        };
        validate_value(source, name, &kind, inner).map_err(|m| validation(&m))?;
        out.insert(name.clone(), typed(value));
    }
    Ok(out)
}

fn texts_ok(name: &str, kind: &str, inner: &Value) -> Result<(), String> {
    let parts = inner
        .as_array()
        .ok_or_else(|| format!("{name} is expected to be {kind}."))?;
    if parts.len() > 100 {
        return Err(format!(
            "body.properties.{name}.{kind}.length should be <= 100, instead was {}.",
            parts.len()
        ));
    }
    for (i, part) in parts.iter().enumerate() {
        let content = part
            .pointer("/text/content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("body.properties.{name}.{kind}[{i}].text.content should be defined.")
            })?;
        let length = content.chars().count();
        if length > MAX_TEXT_CHARS {
            return Err(format!(
                "body.properties.{name}.{kind}[{i}].text.content.length should be <= {MAX_TEXT_CHARS}, instead was {length}."
            ));
        }
    }
    Ok(())
}

fn option_ok(name: &str, option: &Value) -> Result<String, String> {
    let text = option
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| format!("body.properties.{name} option name should be defined."))?;
    if text.contains(',') {
        return Err(format!(
            "body.properties.{name} option name cannot contain commas."
        ));
    }
    let length = text.chars().count();
    if length > MAX_OPTION_CHARS {
        return Err(format!(
            "body.properties.{name} option name.length should be <= {MAX_OPTION_CHARS}, instead was {length}."
        ));
    }
    Ok(text.to_string())
}

fn is_date(text: &str) -> bool {
    let date = text.get(..10).unwrap_or("");
    let b = date.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && (text.len() == 10 || text.get(10..).is_some_and(|t| t.starts_with('T')))
}

fn validate_value(
    source: &mut Source,
    name: &str,
    kind: &str,
    inner: &Value,
) -> Result<(), String> {
    let mismatch = || format!("{name} is expected to be {kind}.");
    match kind {
        "title" | "rich_text" => texts_ok(name, kind, inner),
        "select" => {
            if inner.is_null() {
                return Ok(());
            }
            let text = option_ok(name, inner)?;
            let options = source.options.entry(name.to_string()).or_default();
            if !options.contains(&text) {
                options.push(text);
            }
            Ok(())
        }
        "multi_select" => {
            let list = inner.as_array().ok_or_else(mismatch)?;
            for option in list {
                let text = option_ok(name, option)?;
                let options = source.options.entry(name.to_string()).or_default();
                if !options.contains(&text) {
                    options.push(text);
                }
            }
            Ok(())
        }
        "status" => {
            let text = option_ok(name, inner)?;
            let known = source.options.get(name).is_some_and(|o| o.contains(&text));
            if known {
                Ok(())
            } else {
                Err(format!(
                    "Invalid status option. Status option \"{text}\" does not exist for property \"{name}\"."
                ))
            }
        }
        "date" => {
            if inner.is_null() {
                return Ok(());
            }
            let start = inner
                .get("start")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("body.properties.{name}.date.start should be defined."))?;
            if is_date(start) {
                Ok(())
            } else {
                Err(format!(
                    "body.properties.{name}.date.start should be a valid ISO 8601 date string, instead was \"{start}\"."
                ))
            }
        }
        "url" => match inner {
            Value::Null => Ok(()),
            Value::String(url) if url.chars().count() <= 2000 => Ok(()),
            Value::String(_) => Err(format!(
                "body.properties.{name}.url.length should be <= 2000."
            )),
            _ => Err(mismatch()),
        },
        "people" | "relation" => inner.as_array().map(|_| ()).ok_or_else(mismatch),
        "number" => (inner.is_null() || inner.is_number())
            .then_some(())
            .ok_or_else(mismatch),
        "checkbox" => inner.is_boolean().then_some(()).ok_or_else(mismatch),
        _ => Err(format!("{name} cannot be written (type {kind}).")),
    }
}

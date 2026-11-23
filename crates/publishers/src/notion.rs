//! The Notion publisher: tasks exported to a Notion database.
//!
//! * [`Notion`] is a REST client for one API base and one internal-integration token. Every
//!   request carries `Authorization: Bearer`, the pinned `Notion-Version` and JSON. The token
//!   is registered with the [`HttpClient`] so no error text can contain it, and the API base
//!   must be `https://api.notion.com/v1` (or a loopback address, for tests), so a token can
//!   never be sent anywhere else.
//! * Since version `2025-09-03` a database holds one or more *data sources*; pages are
//!   created in, and queried from, a data source. [`Notion::resolve_data_source`] takes the
//!   configured one or the only one.
//! * [`NotionTasks`] is the [`TaskExporter`]: the key of a task is written into a text
//!   property, a lookup is a query on it, and the field mapping is [`mapping`].
//! * Errors are classified on top of [`classify_status`](crate::http::classify_status) by
//!   the `code` of the error body: see [`notion_error`].
//!
//! Pacing between writes (Notion allows about three requests per second) is the dispatcher's
//! business; nothing here sleeps.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use cycle_store::{normalize_notion_id, NotionProperties};
use serde_json::{json, Map, Value};

use crate::export::{permanent_error, ExternalTask, TaskExporter, TaskKey, TaskRecord};
use crate::http::{parse_endpoint, ErrorKind, HttpClient, HttpError, Method, Request, Response};

pub mod mapping;

pub use mapping::{
    check, option_name, property_values, propose, propose_with_options, DataSourceSchema, Field,
    MappingError, MappingProblem, Options, PropertyType, Schema,
};

/// The public Notion API.
pub const DEFAULT_API: &str = "https://api.notion.com/v1";
/// The API version every request pins: `2025-09-03` introduced data sources, `2026-03-11`
/// renamed `archived` to `in_trash`.
pub const NOTION_VERSION: &str = "2026-03-11";
/// Pause after a write: Notion allows roughly three requests per second per connection.
pub const WRITE_PACING: Duration = Duration::from_millis(350);
/// Pages a key query asks for; a key is on one page, anything more is a duplicate to report.
const KEY_QUERY_PAGE_SIZE: usize = 10;
/// The hint of a 404: the usual cause is a database that was not shared with the integration.
const NOT_SHARED_HINT: &str = "Notion cannot find it: the database is not shared with the \
integration (in Notion open the database, then ... > Connections, and add the integration)";

// ----- the API base ------------------------------------------------------------------------

/// A checked API base without a trailing slash: exactly `https://api.notion.com/v1`, or
/// `http` on this machine (tests, local servers) with any path and no query.
pub fn normalize_api_base(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let endpoint = parse_endpoint(url).map_err(|e| e.message)?;
    if endpoint.path.contains(['?', '#']) {
        return Err("the API address must not have a query or a fragment".into());
    }
    if endpoint.scheme == "https" {
        let default_port = endpoint.port.is_none_or(|p| p == 443);
        let ok = endpoint.host.eq_ignore_ascii_case("api.notion.com")
            && default_port
            && endpoint.path == "/v1";
        if !ok {
            return Err(format!("the Notion API address must be {DEFAULT_API}"));
        }
    }
    Ok(url.to_string())
}

fn invalid(message: impl Into<String>) -> HttpError {
    permanent_error(ErrorKind::InvalidRequest, message)
}

/// The canonical id for a path: 32 lowercase hex digits.
fn id_for_path(id: &str) -> Result<String, HttpError> {
    normalize_notion_id(id).ok_or_else(|| invalid("a Notion id is malformed"))
}

// ----- answers -----------------------------------------------------------------------------

/// The integration the token belongs to (`GET /users/me`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Me {
    pub id: String,
    pub name: Option<String>,
    pub workspace: Option<String>,
}

/// A data source of a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSourceRef {
    pub id: String,
    pub name: String,
}

/// A database and its data sources (`GET /databases/{id}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseInfo {
    pub id: String,
    pub title: String,
    pub data_sources: Vec<DataSourceRef>,
}

/// A page of a data source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub id: String,
    pub url: Option<String>,
    /// In the trash (`in_trash`).
    pub in_trash: bool,
}

impl Page {
    fn parse(value: &Value) -> Result<Page, HttpError> {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| body_error("the answer of Notion has no page id"))?;
        Ok(Page {
            id: id.to_string(),
            url: value.get("url").and_then(Value::as_str).map(String::from),
            in_trash: value
                .get("in_trash")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn external(&self) -> ExternalTask {
        ExternalTask {
            id: self.id.clone(),
            url: self.url.clone(),
            trashed: self.in_trash,
        }
    }
}

fn body_error(message: &str) -> HttpError {
    permanent_error(ErrorKind::Body, message)
}

fn plain_text(rich: Option<&Value>) -> String {
    rich.and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("plain_text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

// ----- error classification --------------------------------------------------------------

/// Refines the classification of a failing answer by the `code` of its body.
///
/// * `409` (`conflict_error`, another write in progress): transient, it is tried again.
/// * `404 object_not_found`: permanent, with the hint that the database is not shared with
///   the integration.
/// * `401`: permanent, "Notion rejected the token".
/// * `403 restricted_resource`: permanent, the integration lacks a capability.
/// * `400 validation_error`: permanent, with Notion's message (it names the property).
/// * `429` keeps the wait of `Retry-After` from [`classify_status`](crate::http::classify_status).
pub fn notion_error(mut err: HttpError, response: &Response) -> HttpError {
    let body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    let code = body.get("code").and_then(Value::as_str).unwrap_or("");
    let message = crate::http::api_message(&response.body);
    match (response.status, code) {
        (409, _) => {
            err.message =
                "Notion reported a conflict with another change, to be tried again".to_string();
            err.failure = store::Failure::Transient {
                retry_after_ms: None,
            };
        }
        (401, _) => {
            err.message = "Notion rejected the token".to_string();
            err.failure = store::Failure::Permanent;
        }
        (404, "object_not_found") => {
            err.message = NOT_SHARED_HINT.to_string();
            err.failure = store::Failure::Permanent;
        }
        (403, "restricted_resource") => {
            err.message = "the Notion integration lacks a capability: enable Read, Insert and \
Update content for it in the Notion integration settings"
                .to_string();
            err.failure = store::Failure::Permanent;
        }
        (400, "validation_error") => {
            err.message = match message {
                Some(m) => format!("Notion refused the values: {m}"),
                None => "Notion refused the values".to_string(),
            };
            err.failure = store::Failure::Permanent;
        }
        _ => {}
    }
    err
}

// ----- the client --------------------------------------------------------------------------

/// A Notion REST client for one API base and one token.
pub struct Notion {
    client: HttpClient,
    api: String,
    token: String,
    /// The single data source of a database, by database id, for this client's lifetime.
    sources: Mutex<HashMap<String, String>>,
}

impl Notion {
    /// A client with the default timeouts. `api` is checked with [`normalize_api_base`].
    pub fn new(api: &str, token: &str) -> Result<Notion, HttpError> {
        Self::with_client(HttpClient::new(), api, token)
    }

    /// A client over a given [`HttpClient`] (tests use short timeouts). The token is
    /// registered with it so no error text can contain it.
    pub fn with_client(client: HttpClient, api: &str, token: &str) -> Result<Notion, HttpError> {
        let api = normalize_api_base(api).map_err(invalid)?;
        Ok(Notion {
            client: client.with_secret(token),
            api,
            token: token.to_string(),
            sources: Mutex::new(HashMap::new()),
        })
    }

    pub fn api_base(&self) -> &str {
        &self.api
    }

    fn request(&self, method: Method, path: &str, body: Option<&Value>) -> Request {
        let request = Request::new(method, format!("{}{path}", self.api))
            .bearer(&self.token)
            .header("Notion-Version", NOTION_VERSION);
        match body {
            Some(body) => request.json(body),
            None => request.header("Content-Type", "application/json"),
        }
    }

    fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value, HttpError> {
        let request = self.request(method, path, body);
        self.client.send_refined(&request, &notion_error)?.json()
    }

    /// The integration the token belongs to: the token is valid (`GET /users/me`).
    pub fn current_user(&self) -> Result<Me, HttpError> {
        let me = self.call(Method::Get, "/users/me", None)?;
        let id = me
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| body_error("the answer of Notion has no user id"))?;
        Ok(Me {
            id: id.to_string(),
            name: me.get("name").and_then(Value::as_str).map(String::from),
            workspace: me
                .pointer("/bot/workspace_name")
                .and_then(Value::as_str)
                .map(String::from),
        })
    }

    /// The title and the data sources of a database (`GET /databases/{id}`).
    pub fn database(&self, database: &str) -> Result<DatabaseInfo, HttpError> {
        let id = id_for_path(database)?;
        let value = self.call(Method::Get, &format!("/databases/{id}"), None)?;
        let data_sources = value
            .get("data_sources")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|s| {
                        Some(DataSourceRef {
                            id: s.get("id")?.as_str()?.to_string(),
                            name: s
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(DatabaseInfo {
            id,
            title: plain_text(value.get("title")),
            data_sources,
        })
    }

    /// The data source pages go to: `configured` (`targets.notion.data_source`) when set,
    /// else the only data source of the database. None or several is a permanent error that
    /// tells the user to choose one in the Notion mapping.
    pub fn resolve_data_source(
        &self,
        database: &str,
        configured: Option<&str>,
    ) -> Result<String, HttpError> {
        if let Some(id) = configured.map(str::trim).filter(|s| !s.is_empty()) {
            return id_for_path(id);
        }
        let key = id_for_path(database)?;
        if let Some(known) = self
            .sources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Ok(known.clone());
        }
        let info = self.database(database)?;
        match info.data_sources.as_slice() {
            [only] => {
                let id = id_for_path(&only.id)?;
                self.sources
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key, id.clone());
                Ok(id)
            }
            [] => Err(invalid(
                "the Notion database has no data source the integration can use: choose the \
data source in the Notion mapping",
            )),
            many => Err(invalid(format!(
                "the Notion database has {} data sources: choose the data source in the Notion \
mapping",
                many.len()
            ))),
        }
    }

    /// The properties of a data source (`GET /data_sources/{id}`).
    pub fn data_source_schema(&self, data_source: &str) -> Result<DataSourceSchema, HttpError> {
        let id = id_for_path(data_source)?;
        let value = self.call(Method::Get, &format!("/data_sources/{id}"), None)?;
        let properties = value
            .get("properties")
            .ok_or_else(|| body_error("the data source has no properties"))?;
        Ok(DataSourceSchema::from_properties(properties))
    }

    /// The pages whose text property `property` equals `text`, oldest first
    /// (`POST /data_sources/{id}/query`). Pages in the trash are not part of a query.
    pub fn query_by_key(
        &self,
        data_source: &str,
        property: &str,
        text: &str,
    ) -> Result<Vec<Page>, HttpError> {
        let id = id_for_path(data_source)?;
        let body = json!({
            "filter": {"property": property, "rich_text": {"equals": text}},
            "sorts": [{"timestamp": "created_time", "direction": "ascending"}],
            "page_size": KEY_QUERY_PAGE_SIZE,
        });
        let value = self.call(
            Method::Post,
            &format!("/data_sources/{id}/query"),
            Some(&body),
        )?;
        value
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| body_error("the answer of Notion has no results"))?
            .iter()
            .map(Page::parse)
            .collect()
    }

    /// Creates a page in a data source (`POST /pages`).
    pub fn create_page(
        &self,
        data_source: &str,
        properties: &Map<String, Value>,
    ) -> Result<Page, HttpError> {
        let body = json!({
            "parent": {"type": "data_source_id", "data_source_id": data_source},
            "properties": properties,
        });
        Page::parse(&self.call(Method::Post, "/pages", Some(&body))?)
    }

    /// A page; `None` when it is gone or not shared with the integration (404).
    pub fn get_page(&self, page: &str) -> Result<Option<Page>, HttpError> {
        let id = id_for_path(page)?;
        match self.call(Method::Get, &format!("/pages/{id}"), None) {
            Ok(value) => Page::parse(&value).map(Some),
            Err(e) if e.status == Some(404) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Writes properties of a page (`PATCH /pages/{id}`).
    pub fn update_page(
        &self,
        page: &str,
        properties: &Map<String, Value>,
    ) -> Result<Page, HttpError> {
        let id = id_for_path(page)?;
        let body = json!({ "properties": properties });
        Page::parse(&self.call(Method::Patch, &format!("/pages/{id}"), Some(&body))?)
    }
}

// ----- the exporter ------------------------------------------------------------------------

/// The Notion side of the exporter: one data source, one mapping.
pub struct NotionTasks<'a> {
    notion: &'a Notion,
    data_source: String,
    map: &'a NotionProperties,
    schema: Schema,
}

impl<'a> NotionTasks<'a> {
    /// An exporter for a data source whose schema is already known.
    pub fn new(
        notion: &'a Notion,
        data_source: String,
        map: &'a NotionProperties,
        schema: Schema,
    ) -> Self {
        Self {
            notion,
            data_source,
            map,
            schema,
        }
    }

    /// Resolves the data source of `database` (see [`Notion::resolve_data_source`]), reads
    /// its schema and checks the mapping against it. A mapping that does not fit is a
    /// permanent error listing the problems, found before anything is written.
    pub fn prepare(
        notion: &'a Notion,
        database: &str,
        data_source: Option<&str>,
        map: &'a NotionProperties,
    ) -> Result<NotionTasks<'a>, HttpError> {
        let data_source = notion.resolve_data_source(database, data_source)?;
        let schema = notion.data_source_schema(&data_source)?.properties;
        let problems = check(map, &schema);
        if !problems.is_empty() {
            return Err(mapping_error(MappingError { problems }));
        }
        Ok(Self::new(notion, data_source, map, schema))
    }

    pub fn data_source(&self) -> &str {
        &self.data_source
    }
}

fn mapping_error(e: MappingError) -> HttpError {
    invalid(format!("the Notion mapping does not fit the database: {e}"))
}

impl TaskExporter for NotionTasks<'_> {
    fn name(&self) -> &'static str {
        "notion"
    }

    fn noun(&self) -> &'static str {
        "page"
    }

    fn get(&self, id: &str) -> Result<Option<ExternalTask>, HttpError> {
        Ok(self.notion.get_page(id)?.map(|p| p.external()))
    }

    fn find(&self, key: &TaskKey) -> Result<Vec<ExternalTask>, HttpError> {
        Ok(self
            .notion
            .query_by_key(&self.data_source, &self.map.key, &key.text())?
            .iter()
            .map(Page::external)
            .collect())
    }

    fn create(&self, task: &TaskRecord) -> Result<ExternalTask, HttpError> {
        let properties =
            property_values(self.map, &self.schema, task, true).map_err(mapping_error)?;
        Ok(self
            .notion
            .create_page(&self.data_source, &properties)?
            .external())
    }

    fn update(&self, id: &str, task: &TaskRecord) -> Result<ExternalTask, HttpError> {
        let properties =
            property_values(self.map, &self.schema, task, false).map_err(mapping_error)?;
        Ok(self.notion.update_page(id, &properties)?.external())
    }
}

#[cfg(test)]
mod tests;

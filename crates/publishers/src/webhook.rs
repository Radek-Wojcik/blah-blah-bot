//! Discord webhook addresses: validation when a webhook is saved, a masked form that is safe
//! to show, and a read-only check that the webhook exists.
//!
//! A webhook URL is a secret (whoever has it can post to the channel). It is accepted only in
//! the shape Discord uses, on Discord's own hosts, over `https`; plain `http` is accepted for
//! this machine so tests and local development can use a fake server.

use serde_json::Value;

use crate::http::{plain_http_allowed, HttpClient, HttpError, Request};

/// Hosts a Discord webhook can live on.
pub const DISCORD_HOSTS: [&str; 4] = [
    "discord.com",
    "discordapp.com",
    "canary.discord.com",
    "ptb.discord.com",
];

/// A validated webhook address.
#[derive(Clone, PartialEq, Eq)]
pub struct Webhook {
    /// `https://discord.com` (or `http://127.0.0.1:port` for a local fake).
    origin: String,
    host: String,
    id: String,
    token: String,
}

// The token must not reach a log through `{:?}`.
impl std::fmt::Debug for Webhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Webhook({})", self.masked())
    }
}

/// What Discord says about a webhook (a read-only call).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookInfo {
    pub name: Option<String>,
    pub channel_id: Option<String>,
}

impl Webhook {
    /// Checks and splits a webhook URL. The error says what is wrong without repeating the
    /// value.
    pub fn parse(url: &str) -> Result<Webhook, String> {
        let url = url.trim();
        let (scheme, rest) = url
            .split_once("://")
            .ok_or("this is not a web address (expected https://discord.com/api/webhooks/...)")?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "https" && scheme != "http" {
            return Err("the address must start with https://".into());
        }
        if rest.contains(['?', '#']) {
            return Err("the webhook address must not have a query or a fragment".into());
        }
        let (authority, path) = rest
            .split_once('/')
            .map(|(a, p)| (a, format!("/{p}")))
            .ok_or("the address has no path (expected /api/webhooks/<id>/<token>)")?;
        if authority.contains('@') {
            return Err("the webhook address must not contain credentials".into());
        }
        let host = authority
            .rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty())
            .map_or(authority, |(h, _)| h)
            .to_ascii_lowercase();
        let has_port = host.len() != authority.len();
        if scheme == "https" {
            if !DISCORD_HOSTS.contains(&host.as_str()) {
                return Err(
                    "the host is not a Discord address (discord.com, discordapp.com, canary.discord.com or ptb.discord.com)"
                        .into(),
                );
            }
            if has_port {
                return Err("a Discord webhook address has no port".into());
            }
        } else if !plain_http_allowed(&host) {
            return Err("the webhook address must use https".into());
        }

        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        let (id, token) = match segments.as_slice() {
            ["api", "webhooks", id, token] => (*id, *token),
            ["api", version, "webhooks", id, token]
                if version.len() > 1
                    && version.starts_with('v')
                    && version[1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                (*id, *token)
            }
            _ => return Err("the path must be /api/webhooks/<id>/<token>".into()),
        };
        if !(17..=20).contains(&id.len()) || !id.chars().all(|c| c.is_ascii_digit()) {
            return Err("the webhook id must be a number of 17 to 20 digits".into());
        }
        if !(8..=256).contains(&token.len())
            || !token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err("the webhook token has an unexpected shape".into());
        }
        let port_part = &authority[host.len()..];
        Ok(Webhook {
            origin: format!("{scheme}://{host}{port_part}"),
            host,
            id: id.to_string(),
            token: token.to_string(),
        })
    }

    /// The full address, to be sent to Discord and nowhere else.
    pub fn url(&self) -> String {
        format!("{}/api/webhooks/{}/{}", self.origin, self.id, self.token)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// A form that is safe to show: the host and the first and last four digits of the id.
    pub fn masked(&self) -> String {
        let id = &self.id;
        format!(
            "{} webhook {}...{}",
            self.host,
            &id[..4],
            &id[id.len() - 4..]
        )
    }

    /// An HTTP client that scrubs this webhook from every error.
    pub fn client(&self) -> HttpClient {
        HttpClient::new()
            .with_secret(&self.url())
            .with_secret(&self.token)
    }

    /// `GET` on the webhook: it exists, and its name and channel. Sends no message.
    pub fn check(&self, client: &HttpClient) -> Result<WebhookInfo, HttpError> {
        let response = client.send(&Request::get(self.url()))?;
        let body: Value = response.json()?;
        let text = |key: &str| body.get(key).and_then(Value::as_str).map(String::from);
        Ok(WebhookInfo {
            name: text("name"),
            channel_id: text("channel_id"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "123456789012345678";
    const TOKEN: &str = "abcDEF_123-xyz";

    fn ok(url: &str) -> Webhook {
        Webhook::parse(url).unwrap_or_else(|e| panic!("{url}: {e}"))
    }

    fn bad(url: &str) -> String {
        Webhook::parse(url).expect_err(url)
    }

    #[test]
    fn accepts_the_shapes_discord_uses() {
        for host in DISCORD_HOSTS {
            let url = format!("https://{host}/api/webhooks/{ID}/{TOKEN}");
            let hook = ok(&url);
            assert_eq!((hook.id(), hook.token(), hook.host()), (ID, TOKEN, host));
            assert_eq!(hook.url(), url);
        }
        // A versioned path is normalised away; surrounding spaces and a trailing slash are fine.
        let hook = ok(&format!(
            " https://DISCORD.com/api/v10/webhooks/{ID}/{TOKEN}/ "
        ));
        assert_eq!(
            hook.url(),
            format!("https://discord.com/api/webhooks/{ID}/{TOKEN}")
        );
    }

    #[test]
    fn accepts_a_loopback_http_address_for_tests() {
        for origin in [
            "http://127.0.0.1:8123",
            "http://localhost:9",
            "http://[::1]:4000",
        ] {
            let hook = ok(&format!("{origin}/api/webhooks/{ID}/{TOKEN}"));
            assert_eq!(hook.url(), format!("{origin}/api/webhooks/{ID}/{TOKEN}"));
        }
    }

    #[test]
    fn rejects_everything_else_without_repeating_the_value() {
        let secret = "SECRETtokenValue99";
        let cases = [
            format!("http://discord.com/api/webhooks/{ID}/{secret}"),
            format!("https://evil.example/api/webhooks/{ID}/{secret}"),
            format!("https://discord.com.evil.example/api/webhooks/{ID}/{secret}"),
            format!("https://notdiscord.com/api/webhooks/{ID}/{secret}"),
            format!("https://discord.com:8443/api/webhooks/{ID}/{secret}"),
            format!("https://user:pw@discord.com/api/webhooks/{ID}/{secret}"),
            format!("https://discord.com/api/webhooks/{ID}/{secret}?wait=true"),
            format!("https://discord.com/api/webhooks/{ID}/{secret}#x"),
            format!("https://discord.com/api/webhooks/{ID}"),
            format!("https://discord.com/webhooks/{ID}/{secret}"),
            format!("https://discord.com/api/webhooks/12345/{secret}"),
            format!("https://discord.com/api/webhooks/{ID}x/{secret}"),
            format!("https://discord.com/api/webhooks/{ID}/{secret}/extra"),
            format!("https://discord.com/api/webhooks/{ID}/sh"),
            format!("https://discord.com/api/webhooks/{ID}/bad token!"),
            format!("ftp://discord.com/api/webhooks/{ID}/{secret}"),
            format!("http://192.168.1.5/api/webhooks/{ID}/{secret}"),
            "discord.com/api/webhooks/1/2".to_string(),
            "https://discord.com".to_string(),
            String::new(),
        ];
        for url in cases {
            let message = bad(&url);
            assert!(
                !message.contains(secret) && !message.contains(ID),
                "{url}: {message}"
            );
        }
    }

    #[test]
    fn the_masked_form_and_debug_hide_the_token() {
        let hook = ok(&format!("https://discord.com/api/webhooks/{ID}/{TOKEN}"));
        assert_eq!(hook.masked(), "discord.com webhook 1234...5678");
        assert!(!format!("{hook:?}").contains(TOKEN));
        assert!(!format!("{hook:?}").contains(ID));
    }
    #[test]
    fn check_reads_the_webhook_without_posting() {
        use crate::testing::FakeDiscord;
        let fake = FakeDiscord::start();
        let hook = ok(&fake.webhook_url());
        let info = hook.check(&hook.client()).unwrap();
        assert_eq!(info.name.as_deref(), Some("Fake webhook"));
        assert!(info.channel_id.is_some());
        assert!(fake.messages().is_empty());
        assert!(fake.requests().iter().all(|r| r.method == "GET"));

        fake.delete_webhook();
        let err = hook.check(&hook.client()).unwrap_err();
        assert_eq!(err.status, Some(404));
        assert!(!err.message.contains(hook.token()));

        let gone = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let hook = ok(&format!("http://{gone}/api/webhooks/{ID}/{TOKEN}"));
        let client = HttpClient::with_timeouts(
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(5),
        )
        .with_secret(hook.token());
        let err = hook.check(&client).unwrap_err();
        assert!(err.is_offline() && !err.message.contains(TOKEN), "{err:?}");
    }
}

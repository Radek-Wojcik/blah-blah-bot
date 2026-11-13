# 0011. Secret storage

**Status:** Accepted

## Context

The application holds tokens for GitHub, Discord, Notion, Hugging Face and optionally cloud model providers. The project is open source and its configuration files are meant to be shareable.

## Decision

- Store all secrets in Windows Credential Manager.
- Configuration and cycle files contain references such as `cred:discord/nomad`, never values.
- Redact secrets from logs and error reports.
- Recommend fine-grained GitHub tokens limited to the cycle repository.

## Consequences

- Secrets are bound to the Windows user account and do not travel with configuration or backups.
- A cycle repository can be public without exposing targets.

## Alternatives

- **Encrypted file with a master password:** portable, adds a password prompt.
- **Environment variables:** convenient for development, unsuitable for a desktop application.

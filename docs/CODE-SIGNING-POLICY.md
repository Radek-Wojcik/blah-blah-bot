# Code Signing Policy (Draft)

> **Status:** Draft. This policy applies if and when the project adopts code signing ([ADR 0024](decisions/0024-installer-signing.md)).

## Scope

When signing is enabled, the project signs only its own Windows binaries built from this repository:

- `app.exe` (the Blah Blah Bot desktop application)
- `Blah-Blah-Bot_<version>_x64-setup.exe` (the NSIS installer)
- `uninstall.exe` (the NSIS uninstaller generated during bundling)

Third-party binaries redistributed alongside the application — specifically the pinned `bin/uv.exe` from `astral-sh/uv` — are **excluded** from project signing and retain their upstream publisher signatures and SHA-256 verification at build time.

## SignPath Foundation Attribution

If the project signs releases through [SignPath Foundation](https://signpath.org/):

- Free code signing is provided by SignPath.io, using a certificate by SignPath Foundation.
- Users will see **SignPath Foundation** as the verified publisher in Windows SmartScreen and file properties.

## Roles and Responsibilities

| Role | Responsibility |
|---|---|
| **Author** | Submits code changes via pull requests in the public GitHub repository (`Radek-Wojcik/blah-blah-bot`). |
| **Reviewer** | Reviews pull requests for correctness, security, and adherence to release checks before merge to `main`. |
| **Approver** | Named project maintainer (`Radoslaw Wojcik`) who manually approves release builds and any signing requests. |

## Build and Signing Security

- All release builds run in GitHub Actions from tagged commits on the public repository with pinned dependencies and reproducible staging (`scripts/build-installer.ps1`).
- Signing credentials or commands (`BBB_SIGN_COMMAND`) are never committed to the repository; they are injected only via protected CI secrets or local release environment variables.
- Multi-factor authentication (MFA) is required for repository administration, CI/CD access, and any signing service account.
- Every release publishes `SHA256SUMS.txt` computed **after** signing so users can verify the exact distributed installer.
- Any suspected compromise or misuse of a signing key or release artifact triggers immediate key revocation and a security advisory.

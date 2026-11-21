# 0020. Installer signing

**Status:** Proposed. The default below ships unless the options are decided otherwise before the 1.0 release.

## Context

The Windows installer (NSIS, per user, built by Tauri in the release workflow) is the only artifact most users run. Windows treats a downloaded executable according to two signals, as Microsoft describes them in [SmartScreen reputation for Windows app developers](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation) (retrieved 2026-11-21): whether the file is signed by a known publisher, and whether this exact file has been downloaded without signs of malicious behavior.

What a user sees, per that page:

- **Unsigned installer:** SmartScreen shows "Windows protected your PC". The user must choose "More info" and then "Run anyway". Reputation starts at zero for every new version, because it cannot carry over between unsigned versions. An enterprise policy can remove the "Run anyway" choice.
- **Signed installer (OV or EV certificate, or a managed signing service):** the first downloads still show an "unrecognized app" warning, now with the verified publisher name. Reputation accumulates for the publisher identity, so later releases signed with the same identity can inherit it. Microsoft gives no threshold; it says it can take several weeks and hundreds of clean installs from a wide audience, and that there is no mechanism to submit a file for review on consumer machines.
- **EV certificates** no longer bypass SmartScreen; Microsoft's current guidance says paying for EV only for that purpose is not justified ([Code signing options](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options), retrieved 2026-11-21).
- **Windows 11 Smart App Control**, when switched on, can block unsigned executables that have no positive reputation (same SmartScreen page). The project targets Windows 10 first, with Windows 11 compatibility; this effect has not been tested here and is unverified for this installer.
- **A self-signed certificate** behaves like no signature for public users.

Signing therefore shortens the period of warnings only after reputation has built up, and costs money, identity checks or both. It does not remove the first-download warning.

## Decision

1. **Default for 1.0: the installer is unsigned.** The documentation states the SmartScreen warning plainly, with the steps to continue, and tells users how to verify the download.
2. **Every release publishes `SHA256SUMS.txt`** next to the installer. The README and the user guide show how to compare it (`Get-FileHash <file> -Algorithm SHA256`). The checksum file is computed after any signing step.
3. **The release workflow keeps a signing hook that is off by default.** A signing step runs only when the signing secrets of the chosen option exist in the repository settings. In the Tauri configuration it is `bundle.windows.signCommand`, a command with `%1` standing for the file path ([Tauri: Windows code signing](https://v2.tauri.app/distribute/sign/windows/), retrieved 2026-11-21). No signing material is ever committed.
4. Whether to adopt one of the options below is a decision for the project owner; this record stays Proposed until then. Changing the signing identity later resets publisher reputation, so the choice should be made once and kept.
5. Self-signed certificates are rejected.

## Options

Facts below were retrieved on 2026-11-21. Prices and eligibility change; confirm them at the source before committing.

### 1. Unsigned (default)

- **Cost:** none.
- **Steps:** none beyond documentation and `SHA256SUMS.txt`.
- **Effect:** the warning above on every new version, for every user, with no way to build reputation across versions.

### 2. Azure Artifact Signing (previously Trusted Signing)

- **Cost:** Basic plan USD 9.99 per month, up to 5,000 signatures, USD 0.005 per further signature; Premium USD 99.99 per month, up to 100,000 signatures ([product page](https://azure.microsoft.com/products/artifact-signing)). Billing starts when the account is created and is not pro-rated ([pricing page](https://azure.microsoft.com/pricing/details/artifact-signing/)). That is about USD 120 per year on the Basic plan.
- **Eligibility for public (publicly trusted) certificates:** organizations in the United States, Canada, the European Union, the United Kingdom, Australia, New Zealand, Japan, South Korea, Singapore, Switzerland, Norway and Israel. **Individual developers must be located in the United States or Canada.** The restriction does not apply to private-trust certificates, which are not useful for public downloads ([quickstart](https://learn.microsoft.com/azure/trusted-signing/quickstart), page updated 2026-11-20). A separate Microsoft page lists a narrower set of organization regions (United States, Canada, EU, UK); the quickstart is the more recent source. A developer outside the individual regions therefore needs a legal business entity in an eligible country.
- **Steps (individual):** an Azure subscription and a Microsoft Entra tenant; register the `Microsoft.CodeSigning` resource provider; a billing account of type Individual whose legal name and address match a government-issued ID; create an Artifact Signing account in a supported Azure region; complete identity validation in the Azure portal (email verification, then an ID check through a third-party verifier with a phone camera and Microsoft Authenticator); create a certificate profile. Processing takes 1 to 30 business days. Street address and email are not placed in the certificate; city, state and country are.
- **Steps (organization):** the same account steps, plus the legal entity name, website, a business identifier, a monitored email on a domain owned by the entity, and possibly registration documents.
- **Hardware token:** none; signing runs in CI. Tauri documents the `artifact-signing-cli` tool for `signCommand`, using `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` and `AZURE_TENANT_ID` as secrets.
- **Effect:** the publisher name appears in the prompt; reputation builds over releases. It does not give instant trust.

### 3. OV code-signing certificate from a certificate authority

- **Cost:** Microsoft quotes USD 150 to 300 per year for OV and USD 400 and more for EV. These are Microsoft's figures; current price lists of individual CAs were not checked and are unverified.
- **Hardware rule:** since 1 June 2023 the CA/Browser Forum requirements oblige CAs to make sure the private key is generated, stored and used in a hardware crypto module (a token, a server HSM or a cloud HSM), for OV as well as EV ([CA/Browser Forum code signing requirements](https://cabforum.org/working-groups/code-signing/requirements/), clause 6.2.7.4.2, as summarized in search results; retrieved 2026-11-21). A plain exported `.pfx` file is therefore no longer available for new certificates, and the Tauri guide for a certificate in the Windows store applies only to certificates issued before that date.
- **Steps:** buy from a CA; identity validation of the organization or, where the CA offers it, the individual (whether a given CA issues OV certificates to private individuals was not verified); receive a token or enable the CA's cloud signing service; sign in CI through the cloud service, or on a machine with the token attached (a USB token cannot be used by a hosted CI runner).
- **Effect:** the same SmartScreen behavior as option 2. Open to applicants anywhere, subject to the CA's rules.

### 4. SignPath Foundation (free signing for open-source projects)

- **Cost:** free for open-source projects ([signpath.org](https://signpath.org/), retrieved 2026-11-21).
- **Eligibility** ([terms](https://signpath.org/terms)): an OSI-approved license with no commercial dual licensing and no proprietary components; the project is actively maintained and already released in the form to be signed; no malware or potentially unwanted behavior; functionality documented on the download page; only the project's own binaries built from its own source. The project must also enable multi-factor authentication for SignPath and the source repository, define author, reviewer and approver roles, publish a code signing policy with the required attribution on its home and download pages, and enforce product name and version metadata.
- **Public repository:** the signing is described as verifying that the binary was built from the project's open-source repository. An explicit "public repository" rule was not found on the pages retrieved, and the application page (`https://signpath.org/apply`) could not be read in full; treat a public repository as required until confirmed with the foundation.
- **Publisher name:** the certificate belongs to the foundation, so users see "SignPath Foundation" as publisher, not the project. The foundation can revoke it if the rules are broken.
- **Steps:** make the repository public, publish the first release (unsigned), add the code signing policy page, apply, then connect the build so that signing requests are approved by a named approver. Application timing is unknown.
- **Effect:** the same SmartScreen behavior as options 2 and 3, with the foundation's identity accumulating reputation.

### 5. Self-signed certificate

Rejected. Windows treats it like an unsigned file for public users, and asking users to install a root certificate is worse than a visible warning.

### Other path: Microsoft Store

For MSIX packages published through the Microsoft Store, Microsoft re-signs the package and no SmartScreen prompt appears ([Code signing options](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)). The project ships an NSIS installer and a Store submission is a different release path with its own requirements; it is out of scope for 1.0 and not evaluated further.

## Consequences

- With the default, first-time users see "Windows protected your PC" and the user guide says so up front. Users who cannot or will not bypass it can build from source.
- Each unsigned release starts with zero reputation, so the warning does not fade between versions.
- The signing step and `SHA256SUMS.txt` are part of the workflow in either case, so adopting an option later is a secrets and configuration change, not a pipeline rewrite.
- Options 2 and 4 depend on facts that outside parties control (eligible regions, the foundation's acceptance). Option 3 depends on a hardware or cloud key and on the CA's validation rules.
- A public repository is a precondition for option 4 and is independent of the other options. The repository is currently private.

## Alternatives

The options above are the alternatives. The deciding inputs are the owner's country and legal status (option 2), the budget (options 2 and 3), the repository's visibility and willingness to display the foundation as publisher (option 4), and how much first-run friction the project accepts for 1.0.

//! The release check of the whole publishing flow: what the Publish button does, against the
//! real test repository and the real test webhook.
//!
//! The application's own parts run as they do in production: the command layer of cycles
//! (`create_cycle`, `save_cycle`), `Publishing::publish` (plan, snapshot, outbox) and the
//! dispatcher thread, the credential store for the token and the webhook, the issue refresh
//! and the context of the next briefing. Everything the test creates lives in a temporary
//! folder (a clone of the test repository, a database file, the recordings), so the data of
//! the user's own installation is not touched.
//!
//! After the demo meeting is published the check adds a correction to it, as the user does
//! after a meeting went out: the correction becomes a new file in the meeting's folder and a
//! comment on the issue of the item, and the next briefing carries it.
//!
//! What stays behind on the real services: three commits on `main` of the test repository
//! (the earlier meeting, the demo meeting and its correction, in folders of a cycle that only
//! exists for this run), closed issues labelled `cycle:RC<time>` and two messages in the test
//! channel. The issues are closed again at the end, also when a step fails.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cycle_store::issues::{ItemMarker, StatusMarker};
use cycle_store::time::now_unix_ms;
use cycle_store::{
    build_context, ContextRequest, Decision, Draft, ItemType, ProposedStatus, PublishMode, Repo,
    DEFAULT_MEETINGS_PATH,
};
use publishers::addr::web_base;
use publishers::github::{api_host, RepoRef, DEFAULT_API};
use publishers::http::{HttpClient, Request};
use publishers::testing::fixtures::{item, proposal, source, task};
use publishers::webhook::Webhook;
use queue::draft::DraftFiles;
use queue::transcript::{RawSegment, RawTranscript, Speaker};
use serde_json::Value;
use store::{Meeting, MeetingState, Screenshot, Store};

use super::{checked_target, require_release_check, var};
use crate::cycles::{self, CycleParticipantDto, DiscordTargetDto, GithubTargetDto};
use crate::issue_sync::refresh_cycle_issues;
use crate::publishing::{AppEnv, OutboxOpDto, Publishing};
use crate::secrets::{self, Credentials, SecretStore};
use crate::settings::{CycleRepoSetting, RepoKind};

/// A GitHub login that nobody should have: the participant who gets it shows what happens to
/// an assignee GitHub does not take.
const FICTIONAL_LOGIN: &str = "bbb-release-check-nobody-7f3a91c2e4";
const EARLIER: &str = "20261110-100000";
const DEMO: &str = "20261113-140501";
const SHOT: &str = "img/0001-00-12-41.png";

// ----- small helpers ----------------------------------------------------------------------

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in bytes {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

/// A small valid PNG (24 x 16, a gradient), written with stored deflate blocks.
fn synthetic_png() -> Vec<u8> {
    let (w, h) = (24usize, 16usize);
    let mut raw = Vec::new();
    for y in 0..h {
        raw.push(0u8);
        for x in 0..w {
            raw.extend([(x * 10) as u8, (y * 16) as u8, 160]);
        }
    }
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend((raw.len() as u16).to_le_bytes());
    z.extend((!(raw.len() as u16)).to_le_bytes());
    z.extend(&raw);
    z.extend(adler32(&raw).to_be_bytes());
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        png.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        png.extend(&body);
        png.extend(crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend((w as u32).to_be_bytes());
    ihdr.extend((h as u32).to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    png
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Git for the test repository: the token reaches Git only through its environment, as
/// the application does it, never through a file or an argument.
fn git_with_token(dir: Option<&Path>, base: &str, token: &str, args: &[&str]) -> String {
    let base = format!("{}/", base.trim_end_matches('/'));
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let out = cmd
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", format!("http.{base}.extraheader"))
        .env(
            "GIT_CONFIG_VALUE_0",
            format!(
                "AUTHORIZATION: basic {}",
                base64(format!("x-access-token:{token}").as_bytes())
            ),
        )
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed (output withheld, it may name the address)"
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Everything a log, a file or a row must not hold.
struct Secrets(Vec<String>);

impl Secrets {
    fn leaks(&self, text: &str) -> bool {
        self.0.iter().any(|s| text.contains(s.as_str()))
    }

    fn leaks_bytes(&self, bytes: &[u8]) -> bool {
        self.0
            .iter()
            .any(|s| bytes.windows(s.len()).any(|w| w == s.as_bytes()))
    }
}

/// What the tracing output of this process says.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

fn capture_log() -> Captured {
    let log = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
    log
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

// ----- the real services ------------------------------------------------------------------

/// Reads from the GitHub API with the stored token (nothing is written here).
struct Api {
    base: String,
    repo: RepoRef,
    token: String,
    client: HttpClient,
}

impl Api {
    fn new(world: &World) -> Api {
        Api {
            base: world.api.clone(),
            repo: world.repo.clone(),
            token: world.token.clone(),
            client: HttpClient::new().with_secret(&world.token),
        }
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/repos/{}{path}",
            self.base.trim_end_matches('/'),
            self.repo.full_name()
        )
    }

    fn get(&self, path: &str) -> Value {
        let request = Request::get(self.url(path))
            .header("Accept", "application/vnd.github+json")
            .bearer(&self.token);
        self.client
            .send(&request)
            .unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
            .json()
            .expect("JSON")
    }

    fn close(&self, number: u64) {
        let request = Request::patch_json(
            self.url(&format!("/issues/{number}")),
            &serde_json::json!({ "state": "closed", "state_reason": "completed" }),
        )
        .header("Accept", "application/vnd.github+json")
        .bearer(&self.token);
        if let Err(e) = self.client.send(&request) {
            eprintln!("could not close issue #{number}: {}", e.message);
        }
    }
}

/// Closes the issues the run created, when the test ends in any way.
struct CloseIssues {
    api: Api,
    numbers: Mutex<Vec<u64>>,
}

impl CloseIssues {
    fn note(&self, number: u64) {
        let mut numbers = self.numbers.lock().unwrap_or_else(|e| e.into_inner());
        if !numbers.contains(&number) {
            numbers.push(number);
        }
    }
}

impl Drop for CloseIssues {
    fn drop(&mut self) {
        let numbers = self
            .numbers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for number in numbers {
            self.api.close(number);
        }
        println!("closed the issues of the run again");
    }
}

// ----- the rig ----------------------------------------------------------------------------

/// The database and the clone live in `tmp`; the dispatcher thread is stopped with the rig.
struct Rig {
    root: PathBuf,
    clone: PathBuf,
    db: PathBuf,
    setting: CycleRepoSetting,
    repos: Arc<Mutex<Vec<CycleRepoSetting>>>,
    store: Arc<Mutex<Store>>,
    publishing: Arc<Publishing>,
    /// Last, so that it is removed after the database and the dispatcher let go of its files.
    tmp: test_tmp::TestDir,
}

impl Rig {
    /// A clone of the test repository in a temporary folder, a database file there, and a
    /// publishing service over them with the real credential store.
    fn new(world: &World) -> Rig {
        let tmp = test_tmp::TestDir::new("full-loop");
        let remote = world.remote.clone();
        let clone = tmp.path().join("clone");
        git_with_token(
            None,
            &world.web,
            &world.token,
            &["clone", "--quiet", "--", &remote, clone.to_str().unwrap()],
        );
        let setting = CycleRepoSetting {
            id: "release-check".into(),
            path: clone.to_string_lossy().into_owned(),
            remote: Some(remote),
            kind: RepoKind::Clone,
            github_repo: Some(world.repo.full_name()),
            github_api: (world.api != DEFAULT_API).then(|| world.api.clone()),
        };
        let root = tmp.path().join("recordings");
        std::fs::create_dir_all(&root).unwrap();
        let db = tmp.path().join("blah-blah-bot.db");
        let store = Arc::new(Mutex::new(Store::open(&db).unwrap()));
        let repos = Arc::new(Mutex::new(vec![setting.clone()]));
        let (r, root2) = (repos.clone(), root.clone());
        let env = Arc::new(AppEnv::new(
            move || r.lock().unwrap().clone(),
            move || root2.clone(),
            world.secrets.clone(),
        ));
        let publishing = Publishing::new(store.clone(), env);
        publishing.start();
        Rig {
            tmp,
            root,
            clone,
            db,
            setting,
            repos,
            store,
            publishing,
        }
    }

    fn repos(&self) -> Vec<CycleRepoSetting> {
        self.repos.lock().unwrap().clone()
    }

    /// The repository and the cycle with its participants and targets, through the command
    /// layer of cycles.
    fn make_cycle(&self, id: &str, owner_login: &str, webhook_ref: &str) {
        let repos = self.repos();
        cycles::create_cycle(&repos, &self.setting, id, "Release check rover").unwrap();
        let mut dto = cycles::get_cycle(&repos, id).unwrap().cycle;
        dto.participants = vec![
            CycleParticipantDto {
                id: "radek".into(),
                name: "Radek Tester".into(),
                github: Some(owner_login.to_string()),
                discord: None,
            },
            CycleParticipantDto {
                id: "casey".into(),
                name: "Casey Fictional".into(),
                github: Some(FICTIONAL_LOGIN.to_string()),
                discord: None,
            },
        ];
        dto.targets.github = Some(GithubTargetDto {
            path: DEFAULT_MEETINGS_PATH.to_string(),
            publish: PublishMode::OnApproval,
            issues: true,
        });
        dto.targets.discord = Some(DiscordTargetDto {
            webhook: Some(webhook_ref.to_string()),
            bot: None,
            publish: PublishMode::OnApproval,
            template: None,
        });
        let saved = cycles::save_cycle(&repos, &*self.store.lock().unwrap(), dto).unwrap();
        assert!(saved.saved, "the cycle is valid: {:?}", saved.issues);
    }

    /// A finished meeting: its row, the approved draft, the transcript and the screenshots.
    fn add_meeting(&self, draft: &Draft, screenshots: &[(&str, f64)]) {
        let id = draft.meeting_id.clone();
        let dir = self.root.join(&id);
        std::fs::create_dir_all(dir.join("work")).unwrap();
        std::fs::create_dir_all(dir.join("img")).unwrap();
        DraftFiles::new(&dir).save(draft).unwrap();
        let transcript = RawTranscript {
            version: 1,
            meeting_id: id.clone(),
            language: "en".into(),
            speakers: vec![Speaker {
                id: "radek".into(),
                label: "Radek Tester".into(),
                kind: "voice".into(),
                track: None,
            }],
            segments: vec![
                segment(
                    0,
                    745_000,
                    "I will mount the camera on the rover by Friday.",
                ),
                segment(1, 900_000, "The antenna arrived, so that one is done."),
            ],
        };
        std::fs::write(
            dir.join("work/transcript.json"),
            serde_json::to_vec(&transcript).unwrap(),
        )
        .unwrap();
        let store = self.store.lock().unwrap();
        store
            .create_meeting(&Meeting {
                id: id.clone(),
                title: "Release check meeting".into(),
                started_unix_ms: 1,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: id.clone(),
            })
            .unwrap();
        for (file, offset_ms) in screenshots {
            std::fs::write(dir.join(file), synthetic_png()).unwrap();
            store
                .add_screenshot(
                    &id,
                    &Screenshot {
                        file: (*file).into(),
                        offset_ms: *offset_ms,
                    },
                )
                .unwrap();
        }
    }

    /// The operations of a meeting.
    fn ops_of(&self, meeting: &str) -> Vec<OutboxOpDto> {
        self.publishing
            .state()
            .unwrap()
            .ops
            .into_iter()
            .filter(|o| o.meeting_id == meeting)
            .collect()
    }

    /// Waits until the dispatcher has finished every operation of the meeting. Fails, with
    /// what each operation says, when one gave up or the time is out.
    fn settle(&self, meeting: &str, close: &CloseIssues, within: Duration) -> Vec<OutboxOpDto> {
        let started = Instant::now();
        loop {
            let ops = self.ops_of(meeting);
            for op in &ops {
                if let Some(n) = op
                    .result
                    .as_ref()
                    .and_then(|r| r.get("number"))
                    .and_then(Value::as_u64)
                {
                    if op.kind == "create_issue" {
                        close.note(n);
                    }
                }
            }
            let states: Vec<String> = ops
                .iter()
                .map(|o| format!("{}={} ({} tries)", o.op_key, o.state, o.attempts))
                .collect();
            // An operation that gave up holds back the ones that follow it: say why at once.
            if let Some(op) = ops.iter().find(|o| o.state == "failed") {
                panic!(
                    "{} failed for good after {} tries: {:?} (the others: {states:?})",
                    op.op_key, op.attempts, op.last_error
                );
            }
            let finished = !ops.is_empty()
                && ops
                    .iter()
                    .all(|o| matches!(o.state, "done" | "failed" | "cancelled"));
            if finished {
                println!("{meeting}: {}", states.join(", "));
                let failed: Vec<String> = ops
                    .iter()
                    .filter(|o| o.state != "done")
                    .map(|o| format!("{}: {:?}", o.op_key, o.last_error))
                    .collect();
                assert!(failed.is_empty(), "operations did not finish: {failed:?}");
                return ops;
            }
            assert!(
                started.elapsed() < within,
                "the outbox did not finish in {within:?}: {states:?}"
            );
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.publishing.stop();
    }
}

fn segment(id: u32, start_ms: u64, text: &str) -> RawSegment {
    RawSegment {
        id,
        track: "app:discord".into(),
        speaker: "radek".into(),
        start_ms,
        end_ms: start_ms + 5_000,
        text: text.into(),
        words: Vec::new(),
    }
}

fn op<'a>(ops: &'a [OutboxOpDto], key: &str) -> &'a OutboxOpDto {
    ops.iter().find(|o| o.op_key == key).unwrap_or_else(|| {
        panic!(
            "no operation {key} in {:?}",
            ops.iter().map(|o| &o.op_key).collect::<Vec<_>>()
        )
    })
}

fn number_of(op: &OutboxOpDto) -> u64 {
    op.result.as_ref().unwrap()["number"].as_u64().unwrap()
}

/// The draft of a meeting of the cycle: tasks and the proposals are given.
fn draft_of(
    cycle: &str,
    meeting: &str,
    date: &str,
    items: Vec<cycle_store::Item>,
    proposals: Vec<cycle_store::StatusProposal>,
) -> Draft {
    let mut d = publishers::testing::fixtures::draft(meeting, items, proposals);
    d.cycle = cycle.to_string();
    d.date = date.to_string();
    for i in &mut d.items {
        if let Some(s) = i.source.as_mut() {
            s.speaker = "radek".into();
        }
    }
    d
}

// ----- the worlds the check runs in ------------------------------------------------------

/// What the check talks to: the real test repository and webhook, or the fakes of the tests.
struct World {
    /// The GitHub API address.
    api: String,
    repo: RepoRef,
    token: String,
    /// Where the clone comes from.
    remote: String,
    /// The address the token is attached to for Git.
    web: String,
    secrets: Arc<dyn SecretStore + Send + Sync>,
    webhook_ref: String,
    webhook: Webhook,
}

fn real_world() -> World {
    require_release_check();
    let (t, _) = checked_target();
    let webhook_ref = var("BBB_RC_DISCORD_REF");
    let value = secrets::resolve_cred(&Credentials, &webhook_ref).unwrap_or_else(|| {
        panic!("no webhook stored for {webhook_ref}: add it in the publishing settings first")
    });
    let web = web_base(&t.api);
    World {
        remote: format!("{web}/{}.git", t.repo),
        web,
        api: t.api,
        repo: t.repo,
        token: t.token,
        secrets: Arc::new(Credentials),
        webhook_ref,
        webhook: Webhook::parse(&value).expect("the stored webhook is valid"),
    }
}

#[test]
#[ignore = "release check against real services: set BBB_RELEASE_CHECK=yes, see docs/TESTING.md"]
fn full_loop_demo_meeting() {
    let world = real_world();
    run(&world, Some(capture_log()));
}

/// The same check on the fakes: a bare remote, a fake GitHub and a fake webhook, with the
/// secrets in memory. It runs with every test run, so the check itself stays correct.
#[test]
fn the_full_loop_check_passes_against_the_fakes() {
    use publishers::testing::{FakeDiscord, FakeGithub, Visibility};

    const TOKEN: &str = "ghp_FullLoopFakeToken0123456789";
    let tmp = test_tmp::TestDir::new("full-loop");
    let remote = tmp.path().join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    let git = |dir: &Path, args: &[&str]| {
        let out = Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=Test", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    };
    git(
        &remote,
        &["init", "--bare", "--quiet", "--initial-branch=main"],
    );
    let seed = tmp.path().join("seed");
    Repo::init_local(&seed).unwrap();
    std::fs::write(
        seed.join("README.md"),
        "# test
",
    )
    .unwrap();
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "--quiet", "-m", "init"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "--quiet", "origin", "main"]);

    let github = FakeGithub::start();
    github.add_repo("owner/rover", Visibility::Private);
    github.require_token(TOKEN);
    github.unknown_users("owner/rover", &[FICTIONAL_LOGIN]);
    let discord = FakeDiscord::start();
    let webhook_ref = "cred:discord/release-check";
    let memory = Arc::new(crate::secrets::MemorySecrets::default());
    memory
        .set(
            &secrets::target_entry(webhook_ref).unwrap(),
            &discord.webhook_url(),
        )
        .unwrap();
    memory
        .set(
            &secrets::endpoint_key(
                secrets::GITHUB_TOKEN,
                &api_host(&github.api_base()).unwrap(),
            ),
            TOKEN,
        )
        .unwrap();
    let world = World {
        api: github.api_base(),
        repo: RepoRef::parse("owner/rover").unwrap(),
        token: TOKEN.to_string(),
        remote: remote.to_string_lossy().into_owned(),
        web: github.api_base(),
        secrets: memory,
        webhook_ref: webhook_ref.to_string(),
        webhook: Webhook::parse(&discord.webhook_url()).unwrap(),
    };
    run(&world, None);
    assert_eq!(
        discord.messages().len(),
        2,
        "the summary and the screenshot"
    );
}

// ----- the check --------------------------------------------------------------------------

/// The whole flow, as the Publish button does it, against `world`; `log` is what the tracing
/// output goes to (the real check scans it for secrets).
fn run(world: &World, log: Option<Captured>) {
    let watch = Secrets(vec![
        world.token.clone(),
        world.webhook.url(),
        world.webhook.token().to_string(),
    ]);
    let close = CloseIssues {
        api: Api::new(world),
        numbers: Mutex::new(Vec::new()),
    };
    let api = Api::new(world);
    let cycle_id = format!("RC{}", now_unix_ms());
    let owner = world.repo.owner.clone();
    let rig = Rig::new(world);
    rig.make_cycle(&cycle_id, &owner, &world.webhook_ref);
    println!("cycle {cycle_id} in a temporary clone of {}", world.repo);
    let within = Duration::from_secs(300);

    // ----- an earlier meeting publishes the issue that the demo meeting will close ---------
    let earlier = draft_of(
        &cycle_id,
        EARLIER,
        "2026-11-10",
        vec![task("i1", "Order the rover antenna", Some("radek"), None)],
        vec![],
    );
    rig.add_meeting(&earlier, &[]);
    let result = rig
        .publishing
        .publish(EARLIER, &["github".to_string()])
        .expect("the earlier meeting is queued");
    assert_eq!(result.queued, 2, "the commit and one issue");
    let earlier_ops = rig.settle(EARLIER, &close, within);
    let antenna = number_of(op(&earlier_ops, "issue:i1"));

    // ----- the demo meeting ---------------------------------------------------------------
    let mut camera = task(
        "i1",
        "Mount the rover camera",
        Some("radek"),
        Some("2026-11-20"),
    );
    camera.source = Some(source(true, 745_000));
    let wheels = task("i2", "Order six wheels", Some("casey"), None);
    let topic = item("i3", ItemType::Topic, "Battery supplier for the rover");
    let mut done = proposal(
        "s1",
        &format!("#{antenna}"),
        ProposedStatus::Done,
        Decision::Approved,
    );
    done.comment = Some("The antenna arrived on Friday and is installed.".into());
    let mut demo = draft_of(
        &cycle_id,
        DEMO,
        "2026-11-13",
        vec![camera, wheels, topic],
        vec![done],
    );
    demo.summary = "We settled the camera, the wheels and the antenna.".into();
    rig.add_meeting(&demo, &[(SHOT, 761_000.0)]);

    let plan = rig
        .publishing
        .plan(DEMO, &["github".to_string(), "discord".to_string()])
        .expect("the plan");
    assert!(plan.blocks.is_empty(), "{:?}", plan.blocks);
    assert_eq!(plan.repo.visibility, "private");
    assert_eq!(plan.summary.issues.len(), 2);
    assert_eq!(plan.summary.statuses.len(), 1);
    assert!(plan.summary.discord);
    assert!(plan.ops.is_empty(), "planning queues nothing");

    let result = rig
        .publishing
        .publish(DEMO, &["github".to_string(), "discord".to_string()])
        .expect("the demo meeting is queued");
    assert_eq!(result.queued, 5, "commit, two issues, one status, Discord");
    let ops = rig.settle(DEMO, &close, within);

    // ----- the repository: one commit with the meeting folder -------------------------------
    let commit = &op(&ops, "commit").result.as_ref().unwrap().clone();
    assert_eq!(commit["pushed"], true, "{commit}");
    assert_eq!(
        commit["local"], "updated",
        "the clone caught up with the push: {commit}"
    );
    assert_eq!(
        op(&earlier_ops, "commit").result.as_ref().unwrap()["local"],
        "updated"
    );
    let sha = commit["commit"].as_str().unwrap().to_string();
    let folder = commit["folder"].as_str().unwrap().to_string();
    assert_eq!(folder, "2026-11-13");
    let base = world.web.clone();
    let verify = rig.tmp.path().join("verify");
    git_with_token(
        None,
        &base,
        &world.token,
        &[
            "clone",
            "--quiet",
            "--",
            &world.remote,
            verify.to_str().unwrap(),
        ],
    );
    let head = git_with_token(
        Some(&verify),
        &base,
        &world.token,
        &["rev-parse", "origin/main"],
    );
    assert_eq!(head, sha, "the commit of the meeting is the tip of main");
    let folder_in_repo = format!("meetings/{cycle_id}/{folder}");
    for file in ["report.md", "transcript.md", "meeting.json", SHOT] {
        assert!(
            verify.join(&folder_in_repo).join(file).is_file(),
            "{folder_in_repo}/{file} is in the repository"
        );
    }
    assert!(verify
        .join(format!("meetings/{cycle_id}/2026-11-10/report.md"))
        .is_file());
    let png = std::fs::read(verify.join(&folder_in_repo).join(SHOT)).unwrap();
    assert_eq!(png, synthetic_png(), "the screenshot arrived unchanged");
    let report = std::fs::read_to_string(verify.join(&folder_in_repo).join("report.md")).unwrap();
    assert!(report.contains("Mount the rover camera"), "{report}");
    let transcript =
        std::fs::read_to_string(verify.join(&folder_in_repo).join("transcript.md")).unwrap();
    assert!(transcript.contains("t-00-12-25"), "anchors of the quotes");
    let topics =
        std::fs::read_to_string(verify.join(format!("cycles/{cycle_id}/topics.yaml"))).unwrap();
    assert!(
        topics.contains("Battery supplier for the rover"),
        "{topics}"
    );
    let log_lines = git_with_token(
        Some(&verify),
        &base,
        &world.token,
        &["log", "--format=%H %s", "origin/main", "-3"],
    );
    assert!(
        log_lines.lines().count() >= 3,
        "the earlier commit, the demo commit and the seed: {log_lines}"
    );
    println!("the demo meeting is commit {sha} on main of {}", world.repo);

    // ----- the issues ---------------------------------------------------------------------------
    let label = format!("cycle:{cycle_id}");
    let camera_op = op(&ops, "issue:i1");
    let wheels_op = op(&ops, "issue:i2");
    let (camera_n, wheels_n) = (number_of(camera_op), number_of(wheels_op));
    assert_ne!(camera_n, wheels_n);
    let issue = api.get(&format!("/issues/{camera_n}"));
    assert_eq!(issue["state"], "open");
    let labels: Vec<&str> = issue["labels"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l["name"].as_str())
        .collect();
    assert!(labels.contains(&label.as_str()), "{labels:?}");
    let body = issue["body"].as_str().unwrap();
    assert!(body.contains("Due: 2026-11-20"), "the due line: {body}");
    assert_eq!(
        ItemMarker::parse(body).map(|m| (m.cycle, m.meeting, m.item)),
        Some((cycle_id.clone(), DEMO.to_string(), "i1".to_string())),
        "the marker"
    );
    assert!(
        body.contains(&format!("{folder_in_repo}/report.md")),
        "the report link: {body}"
    );
    assert!(
        body.contains("transcript.md#t-00-12-25"),
        "the transcript link: {body}"
    );
    let assigned: Vec<&str> = issue["assignees"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["login"].as_str())
        .collect();
    assert_eq!(
        assigned.len(),
        1,
        "the repository owner is assigned: {assigned:?}"
    );
    assert!(assigned[0].eq_ignore_ascii_case(&owner));
    assert_eq!(camera_op.result.as_ref().unwrap()["created"], true);

    // The participant whose login does not exist: the issue is made without an assignee and
    // the outcome says so.
    let issue = api.get(&format!("/issues/{wheels_n}"));
    assert_eq!(issue["state"], "open");
    assert!(issue["assignees"].as_array().unwrap().is_empty());
    assert_eq!(
        wheels_op.result.as_ref().unwrap()["assignee_dropped"],
        true,
        "{:?}",
        wheels_op.result
    );
    assert!(!issue["body"].as_str().unwrap().contains("Due:"));
    assert!(ItemMarker::parse(issue["body"].as_str().unwrap()).is_some());

    // The earlier issue is closed, with the comment of the approved status change.
    let old = api.get(&format!("/issues/{antenna}"));
    assert_eq!(old["state"], "closed");
    assert_eq!(old["state_reason"], "completed");
    let comments = api.get(&format!("/issues/{antenna}/comments"));
    let comments = comments.as_array().unwrap();
    assert_eq!(comments.len(), 1, "one comment");
    let text = comments[0]["body"].as_str().unwrap();
    assert!(text.contains("The antenna arrived on Friday"), "{text}");
    assert_eq!(
        StatusMarker::parse(text).map(|m| (m.meeting, m.proposal)),
        Some((DEMO.to_string(), "s1".to_string()))
    );
    let status = op(&ops, "status:s1").result.as_ref().unwrap();
    assert_eq!(
        (status["changed"].clone(), status["commented"].clone()),
        (true.into(), true.into())
    );

    // ----- Discord ------------------------------------------------------------------------------
    let post = op(&ops, "discord").result.as_ref().unwrap();
    let (sent, messages) = (
        post["sent_chunks"].as_u64().unwrap(),
        post["messages"].as_u64().unwrap(),
    );
    assert!(sent >= 1 && sent == messages, "{post}");
    assert_eq!(post["skipped_files"], 0);
    println!("Discord: {sent} of {messages} messages accepted");

    // ----- publishing again changes nothing -------------------------------------------------
    let again = rig
        .publishing
        .publish(DEMO, &["github".to_string(), "discord".to_string()])
        .expect("a second press of Publish");
    assert_eq!(again.queued, 0, "nothing is queued twice");
    assert!(rig.ops_of(DEMO).iter().all(|o| o.state == "done"));

    // ----- a correction after publication: a new file and a comment on the issue --------------
    let note = "The camera mount moved to the end of the month.";
    let view = rig
        .publishing
        .add_correction(DEMO, note, Some("i1"), None, Some("2026-11-16"))
        .expect("the correction is written");
    assert_eq!(view.corrections[0].state, "local", "on approval it waits");
    assert!(view.in_repository && view.locked);
    let queued = rig
        .publishing
        .publish_corrections(DEMO, &[])
        .expect("the correction is queued");
    assert_eq!(queued.queued, 2, "the file and the comment");
    let ops = rig.settle(DEMO, &close, within);
    let file = op(&ops, "correction:c1").result.as_ref().unwrap().clone();
    assert_eq!(file["file"], "corrections.md", "{file}");
    assert_eq!(file["pushed"], true);
    assert_eq!(file["local"], "updated", "{file}");
    let correction_path = format!("{folder_in_repo}/corrections.md");
    git_with_token(
        Some(&verify),
        &base,
        &world.token,
        &["fetch", "--quiet", "origin", "main"],
    );
    let published = git_with_token(
        Some(&verify),
        &base,
        &world.token,
        &["show", &format!("origin/main:{correction_path}")],
    );
    let read_back = cycle_store::corrections::parse_markdown(&published)
        .expect("the file is a correction note");
    assert_eq!(read_back.text, note);
    assert_eq!(
        (
            read_back.marker.cycle.as_str(),
            read_back.marker.meeting.as_str()
        ),
        (cycle_id.as_str(), DEMO)
    );
    assert_eq!(read_back.marker.about.as_deref(), Some("i1"));
    let still = git_with_token(
        Some(&verify),
        &base,
        &world.token,
        &[
            "show",
            &format!("origin/main:{folder_in_repo}/meeting.json"),
        ],
    );
    assert_eq!(
        Draft::from_json_str(&still).expect("meeting.json validates"),
        demo,
        "the published meeting itself is as it was"
    );
    let comments = api.get(&format!("/issues/{camera_n}/comments"));
    let comments = comments.as_array().unwrap();
    assert_eq!(comments.len(), 1, "one comment on the issue of the item");
    let text = comments[0]["body"].as_str().unwrap();
    assert!(text.contains(note), "{text}");
    assert!(
        text.contains(&correction_path),
        "the link to the file: {text}"
    );
    assert_eq!(
        cycle_store::CorrectionMarker::parse(text).map(|m| (m.meeting, m.id)),
        Some((DEMO.to_string(), "c1".to_string()))
    );
    assert_eq!(
        api.get(&format!("/issues/{camera_n}"))["state"],
        "open",
        "a correction does not close an issue"
    );
    let view = rig.publishing.list_corrections(DEMO).unwrap();
    assert_eq!(view.corrections[0].state, "published");
    assert_eq!(
        view.corrections[0].file_name.as_deref(),
        Some("corrections.md")
    );
    // Asked again, nothing is queued twice.
    let again = rig
        .publishing
        .publish_corrections(DEMO, &["c1".to_string()])
        .unwrap();
    assert_eq!(again.queued, 0);
    println!("correction c1 is {correction_path}, with a comment on #{camera_n}");

    // ----- the next briefing: the closed issue is done ----------------------------------------
    let repo = Repo::open(&rig.clone).unwrap();
    let wanted_open = [camera_n, wheels_n];
    let started = Instant::now();
    loop {
        let list = refresh_cycle_issues(
            world.secrets.as_ref(),
            &rig.setting,
            &repo,
            &cycle_id,
            HttpClient::new(),
        )
        .expect("the repository is linked to GitHub");
        assert!(list.error.is_none(), "{:?}", list.error);
        let open_seen = wanted_open
            .iter()
            .all(|n| list.issues.iter().any(|i| i.number == *n));
        if list.closed.contains(&antenna) && open_seen {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the issue list did not show the changes within 60 s"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    println!("the issue list had caught up after {:?}", started.elapsed());
    let data = cycles::load_cycle_data(&rig.repos(), &cycle_id, false)
        .unwrap()
        .expect("the cycle is registered");
    let mut request = ContextRequest::new(&data.cycle, "2026-11-20");
    request.topics = &data.topics;
    request.issues = &data.issues;
    request.closed_issue_ids = &data.closed_issue_ids;
    request.issue_links = &data.issue_links;
    request.previous_dirs = &data.previous_dirs;
    let context = build_context(&request).expect("the context of the next meeting");
    let previous: Vec<(String, Option<String>)> = context.value["previous_items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["text"].as_str().unwrap().to_string(),
                i["issue"].as_str().map(String::from),
            )
        })
        .collect();
    let texts: Vec<&str> = previous.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        !texts.contains(&"Order the rover antenna"),
        "its issue is closed, so it is done: {texts:?}"
    );
    for want in ["Mount the rover camera", "Order six wheels"] {
        let found = previous
            .iter()
            .find(|(t, _)| t == want)
            .unwrap_or_else(|| panic!("{want} is still open: {texts:?}"));
        assert!(
            found.1.as_deref().is_some_and(|i| i.starts_with('#')),
            "{found:?}"
        );
    }
    let corrections = context.value["corrections"]
        .as_array()
        .unwrap_or_else(|| panic!("the next context has the correction: {}", context.value));
    assert_eq!(corrections.len(), 1);
    assert_eq!(corrections[0]["text"], note);
    assert_eq!(corrections[0]["item"], "Mount the rover camera");
    assert_eq!(corrections[0]["issue"], format!("#{camera_n}"));
    assert_eq!(corrections[0]["meeting"], "2026-11-13");
    assert_eq!(corrections[0]["date"], "2026-11-16");
    let topics_now = context.value["open_topics"].to_string();
    assert!(
        topics_now.contains("Battery supplier for the rover"),
        "the topic of the meeting is open in the next context: {topics_now}"
    );
    // The task list of the cycle (what the briefing panel shows).
    let tasks = rig.publishing.cycle_tasks(&cycle_id).unwrap();
    assert_eq!(tasks.repo.as_deref(), Some(world.repo.full_name().as_str()));
    assert!(tasks.as_of.is_some());
    assert!(
        tasks
            .done
            .iter()
            .any(|d| d.number == antenna && d.title == "Order the rover antenna"),
        "{:?}",
        tasks.done
    );
    for n in wanted_open {
        assert!(tasks.open.iter().any(|o| o.number == n), "#{n} is open");
    }

    // ----- no secret anywhere ----------------------------------------------------------------------
    std::thread::sleep(Duration::from_millis(300));
    if let Some(log) = &log {
        let text =
            String::from_utf8_lossy(&log.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
        assert!(!watch.leaks(&text), "a secret is in the log");
    }
    let mut files = Vec::new();
    files_under(rig.tmp.path(), &mut files);
    assert!(files.iter().any(|f| f == &rig.db));
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        assert!(
            !watch.leaks_bytes(&bytes),
            "a secret is in {}",
            file.strip_prefix(rig.tmp.path()).unwrap_or(file).display()
        );
    }
    for dir in [&rig.clone, &verify] {
        let history = git_with_token(
            Some(dir),
            &base,
            &world.token,
            &["log", "--all", "-p", "--format=%H %an %ae %s"],
        );
        assert!(!watch.leaks(&history), "a secret is in the history");
        let config = std::fs::read_to_string(dir.join(".git/config")).unwrap_or_default();
        assert!(!watch.leaks(&config), "a secret is in a Git config");
    }
    let state = format!("{:?}", rig.publishing.state().unwrap());
    assert!(!watch.leaks(&state), "a secret is in the outbox state");
    let rows = format!("{:?}", rig.store.lock().unwrap().list_ops(None).unwrap());
    assert!(!watch.leaks(&rows), "a secret is in the outbox rows");

    let host = api_host(&world.api).unwrap_or_default();
    println!(
        "full loop verified on {host}: commit {sha}, issues #{antenna} (closed by status), #{camera_n}, #{wheels_n}, Discord {sent}/{messages}"
    );
    drop(close);
}

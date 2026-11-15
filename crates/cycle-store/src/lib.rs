//! The cycle repository: files that describe a cycle, the Git clone they live in, the
//! cached open issues, the context handed to the extraction stages and the deterministic
//! report layout. See `docs/CYCLES.md` for the format.
//!
//! * [`cycle`], [`topics`], [`template`]: parsing, validation and writing of the YAML and
//!   Markdown files.
//! * [`repo`]: the working tree, driven through the `git` command line.
//! * [`issues`]: open issues with an offline cache.
//! * [`publish`]: the commit of a meeting folder, built off the index and pushed.
//! * [`context`](mod@context): `work/context.json`.
//! * [`draft`], [`mod@render`]: the meeting draft types and the template engine.

pub mod context;
pub mod cycle;
pub mod draft;
pub mod error;
mod fsutil;
mod git;
pub mod issues;
pub mod publish;
pub mod render;
pub mod repo;
pub mod schema;
pub mod template;
pub mod time;
pub mod topics;

pub use context::{build_context, Context, ContextRequest, DEFAULT_MAX_PREVIOUS_ITEMS};
pub use cycle::{
    Cycle, DiscordRef, DiscordTarget, GithubTarget, GlossaryEntry, Languages, NotionTarget,
    Participant, PublishMode, Report, Targets, DEFAULT_TEMPLATE_NAME,
};
pub use draft::{
    Certainty, CertaintyLevel, Decision, Draft, Item, ItemType, Language, LanguageSetting,
    ProposedStatus, ReportMode, Section, Source, StatusProposal, Warning, WarningCode,
};
pub use error::{CycleError, Result, ValidationIssue};
pub use git::git_available;
pub use issues::{fetch_open_issues, IssueCache, IssueList, OpenIssue};
pub use publish::{
    publish_meeting, LocalSync, PublishError, PublishErrorKind, PublishFile, PublishRequest,
    Published,
};
pub use render::{
    defuse_discord_mentions, is_discord_id, item_line, item_line_with, neutralise_mentions, render,
    BuiltinSection, RenderOptions, Variant,
};
pub use repo::{
    check_remote_url, CycleEntry, PullOutcome, PullReport, Repo, RepoState, RepoStatus,
};
pub use schema::Schema;
pub use template::{
    create_default_template, list_templates, parse_template, read_template, validate_body,
    validate_template, write_template, Template, TemplateSection, DEFAULT_TEMPLATE_TEXT,
    PLACEHOLDERS, SHORT_TEMPLATE_TEXT,
};
pub use topics::{parse_topics, topics_to_yaml, Topic, TopicStatus};

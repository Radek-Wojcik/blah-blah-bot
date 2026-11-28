//! Builders for the tests of publishing: a draft of a meeting, status proposals and a cycle
//! with participants and targets.

use cycle_store::{
    Certainty, CertaintyLevel, Cycle, Decision, DiscordRef, DiscordTarget, Draft, GithubTarget,
    Item, ItemType, Language, Participant, ProposedStatus, PublishMode, ReportMode, Source,
    StatusProposal,
};

/// An item without assignee, due date or source.
pub fn item(id: &str, item_type: ItemType, text: &str) -> Item {
    Item {
        id: id.into(),
        item_type,
        text: text.into(),
        assignee: None,
        assignee_name: None,
        due: None,
        certainty: Certainty {
            assignee: CertaintyLevel::Unknown,
            due: CertaintyLevel::Unknown,
        },
        section: None,
        relates_to: None,
        source: None,
        edited: false,
    }
}

/// A quote from the transcript, found or not.
pub fn source(verified: bool, t_start_ms: u64) -> Source {
    Source {
        track: "app:discord".into(),
        speaker: "alex".into(),
        t_start_ms,
        t_end_ms: t_start_ms + 3000,
        quote: "We will do it by Friday".into(),
        verified,
        match_score: if verified { 0.97 } else { 0.4 },
    }
}

/// A task with a verified quote at 12:25.
pub fn task(id: &str, text: &str, assignee: Option<&str>, due: Option<&str>) -> Item {
    let mut i = item(id, ItemType::Task, text);
    i.assignee = assignee.map(String::from);
    i.due = due.map(String::from);
    i.source = Some(source(true, 745_000));
    i
}

pub fn proposal(
    id: &str,
    target: &str,
    status: ProposedStatus,
    decision: Decision,
) -> StatusProposal {
    StatusProposal {
        id: id.into(),
        target: target.into(),
        status,
        reason: "It was reported finished.".into(),
        comment: None,
        source: Some(source(true, 900_000)),
        decision,
        edited: false,
    }
}

/// The draft of the meeting `meeting_id` of the cycle NOMAD, on 2026-11-13.
pub fn draft(meeting_id: &str, items: Vec<Item>, proposals: Vec<StatusProposal>) -> Draft {
    Draft {
        version: 1,
        meeting_id: meeting_id.into(),
        cycle: "NOMAD".into(),
        date: "2026-11-13".into(),
        language: Language::En,
        mode: ReportMode::Fixed,
        template: None,
        meeting_type: None,
        sections: vec![],
        summary: "We agreed on the mount.".into(),
        items,
        status_proposals: proposals,
        warnings: vec![],
        key_screenshots: vec![],
    }
}

/// The cycle NOMAD: Alex (GitHub `alex-gh`, a Discord user), Sam (GitHub `sam-gh`), and the
/// given targets. A Discord target is `(reference, mode)`.
pub fn cycle(github: Option<PublishMode>, discord: Option<(&str, PublishMode)>) -> Cycle {
    let mut c = Cycle::new("NOMAD", "Nomad rover");
    c.participants = vec![
        Participant {
            id: "alex".into(),
            name: "Alex Example".into(),
            github: Some("alex-gh".into()),
            discord: Some(DiscordRef {
                user_id: Some("111111111111111111".into()),
                role_id: None,
            }),
        },
        Participant {
            id: "sam".into(),
            name: "Sam Sample".into(),
            github: Some("sam-gh".into()),
            discord: None,
        },
    ];
    c.targets.github = github.map(|publish| GithubTarget {
        publish,
        ..GithubTarget::default()
    });
    c.targets.discord = discord.map(|(webhook, publish)| DiscordTarget {
        webhook: Some(webhook.into()),
        bot: None,
        publish,
        template: None,
    });
    c
}

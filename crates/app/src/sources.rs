//! What the source picker offers from the browser: the tabs the extension has captured and
//! the whole-browser fallback with the reason it is on offer. Pure functions; the bridge and
//! the audio sessions are read elsewhere.
//!
//! The label of a tab in the picker shows its title and host. It is only shown on this
//! computer: what is stored with the recording and what can be published carries the host
//! alone (`browser::tab_label`).

use std::collections::BTreeMap;

use crate::recording_dto::{SourceInfo, SourceKind};

/// Longest title shown in the picker.
const TITLE_CHARS: usize = 60;

/// A tab the extension has captured, on offer or already recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabEntry {
    pub stream_id: String,
    pub title: String,
    pub host: String,
    pub group_id: Option<i64>,
}

/// How the browser extension is doing, as far as the picker needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionState {
    /// The bridge is turned on in the settings.
    pub enabled: bool,
    pub paired: bool,
    pub connected: bool,
    /// A tab of a tab group could not be captured.
    pub failed_tabs: bool,
}

/// `Tab: <title> (<host>)`, or whichever part there is.
pub fn picker_label(title: &str, host: &str) -> String {
    let title: String = title.trim().chars().take(TITLE_CHARS).collect();
    let host = host.trim();
    match (title.is_empty(), host.is_empty()) {
        (false, false) => format!("Tab: {title} ({host})"),
        (false, true) => format!("Tab: {title}"),
        (true, false) => format!("Tab: {host}"),
        (true, true) => "Browser tab".into(),
    }
}

/// `Group 1`, `Group 2`, ... for the tab groups present, in the order of their ids. A tab
/// without a group (no id, or the browser's "no group" id, which is negative) has none.
pub fn group_names(entries: &[TabEntry]) -> BTreeMap<i64, String> {
    let mut ids: Vec<i64> = entries
        .iter()
        .filter_map(|e| e.group_id)
        .filter(|g| *g >= 0)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .enumerate()
        .map(|(i, id)| (id, format!("Group {}", i + 1)))
        .collect()
}

/// The sources of the tabs, grouped tabs first.
pub fn tab_sources(entries: &[TabEntry]) -> Vec<SourceInfo> {
    let names = group_names(entries);
    let mut out: Vec<(Option<i64>, SourceInfo)> = entries
        .iter()
        .map(|e| {
            let group = e.group_id.filter(|g| *g >= 0);
            let mut info = SourceInfo::new(
                format!("tab:{}", e.stream_id),
                SourceKind::BrowserTab,
                picker_label(&e.title, &e.host),
                true,
            );
            info.group = group.and_then(|g| names.get(&g).cloned());
            (group, info)
        })
        .collect();
    out.sort_by(|a, b| {
        let key = |x: &(Option<i64>, SourceInfo)| (x.0.is_none(), x.0, x.1.label.clone());
        key(a).cmp(&key(b))
    });
    out.into_iter().map(|(_, s)| s).collect()
}

/// Why the whole browser is on offer, and what it costs.
pub fn fallback_hint(ext: &ExtensionState) -> String {
    if !ext.enabled {
        "The browser extension is turned off in the settings. This records all sound from Chrome, not single tabs."
    } else if !ext.paired {
        "The browser extension is not paired. This records all sound from Chrome, not single tabs."
    } else if !ext.connected {
        "The browser extension is not connected. This records all sound from Chrome instead of single tabs."
    } else if ext.failed_tabs {
        "A tab could not be captured. This records all sound from Chrome, including that tab."
    } else {
        "Records all sound from Chrome. Choose tabs instead to leave the others out."
    }
    .to_string()
}

/// Add the tabs to the sources of the system and tell the whole-browser source why it is
/// there.
pub fn apply_browser(sources: &mut Vec<SourceInfo>, entries: &[TabEntry], ext: &ExtensionState) {
    for s in sources.iter_mut().filter(|s| s.kind == SourceKind::Browser) {
        s.hint = Some(fallback_hint(ext));
    }
    sources.extend(tab_sources(entries));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str, title: &str, host: &str, group: Option<i64>) -> TabEntry {
        TabEntry {
            stream_id: id.into(),
            title: title.into(),
            host: host.into(),
            group_id: group,
        }
    }

    fn ext(enabled: bool, paired: bool, connected: bool, failed: bool) -> ExtensionState {
        ExtensionState {
            enabled,
            paired,
            connected,
            failed_tabs: failed,
        }
    }

    #[test]
    fn a_tab_label_shows_the_title_and_the_host() {
        assert_eq!(
            picker_label("ORBIT weekly sync", "meet.example.org"),
            "Tab: ORBIT weekly sync (meet.example.org)"
        );
        assert_eq!(
            picker_label("  ", "docs.example.org"),
            "Tab: docs.example.org"
        );
        assert_eq!(picker_label("Notes", ""), "Tab: Notes");
        assert_eq!(picker_label("", ""), "Browser tab");
        let long = picker_label(&"x".repeat(200), "a.example.org");
        assert_eq!(
            long.chars().count(),
            "Tab:  (a.example.org)".len() + TITLE_CHARS
        );
    }

    #[test]
    fn tab_groups_are_numbered_by_id_and_listed_first() {
        let entries = [
            tab("s1", "Loose", "c.example.org", None),
            tab("s2", "Late", "b.example.org", Some(90)),
            tab("s3", "Early", "a.example.org", Some(12)),
            tab("s4", "Ungrouped marker", "d.example.org", Some(-1)),
            tab("s5", "Early two", "e.example.org", Some(12)),
        ];
        let names = group_names(&entries);
        assert_eq!(names.get(&12).map(String::as_str), Some("Group 1"));
        assert_eq!(names.get(&90).map(String::as_str), Some("Group 2"));
        assert!(!names.contains_key(&-1));

        let out = tab_sources(&entries);
        let summary: Vec<(&str, Option<&str>)> = out
            .iter()
            .map(|s| (s.id.as_str(), s.group.as_deref()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("tab:s3", Some("Group 1")),
                ("tab:s5", Some("Group 1")),
                ("tab:s2", Some("Group 2")),
                ("tab:s1", None),
                ("tab:s4", None),
            ]
        );
        assert!(out
            .iter()
            .all(|s| s.kind == SourceKind::BrowserTab && s.active));
    }

    #[test]
    fn the_hint_names_the_reason_for_the_fallback() {
        assert!(fallback_hint(&ext(false, false, false, false)).contains("turned off"));
        assert!(fallback_hint(&ext(true, false, false, false)).contains("not paired"));
        assert!(fallback_hint(&ext(true, true, false, false)).contains("not connected"));
        assert!(fallback_hint(&ext(true, true, true, true)).contains("could not be captured"));
        assert!(fallback_hint(&ext(true, true, true, false)).contains("Choose tabs"));
        // A lost connection is named before a failed tab.
        assert!(fallback_hint(&ext(true, true, false, true)).contains("not connected"));
    }

    #[test]
    fn only_the_whole_browser_gets_a_hint_and_the_tabs_are_appended() {
        let mut sources = vec![
            SourceInfo::new(
                "mic:default".into(),
                SourceKind::Microphone,
                "Mic".into(),
                true,
            ),
            SourceInfo::new(
                "app:7".into(),
                SourceKind::Application,
                "Discord".into(),
                true,
            ),
            SourceInfo::new("app:100".into(), SourceKind::Browser, "Chrome".into(), true),
        ];
        let entries = [tab("s1", "Sync", "meet.example.org", None)];
        apply_browser(&mut sources, &entries, &ext(true, false, false, false));
        let hints: Vec<bool> = sources.iter().map(|s| s.hint.is_some()).collect();
        assert_eq!(hints, [false, false, true, false]);
        assert_eq!(sources[3].id, "tab:s1");
        assert_eq!(sources[3].label, "Tab: Sync (meet.example.org)");
    }
}

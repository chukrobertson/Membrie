// SPDX-License-Identifier: AGPL-3.0-or-later

use anyhow::{Context, Result};
use atspi::connection::set_session_accessibility;
use atspi::proxy::accessible::ObjectRefExt;
use atspi::{AccessibilityConnection, Interface, Role, State};
use futures_lite::future;
use std::collections::HashSet;
use std::time::Duration;

const MAX_NODES: usize = 500;
const MAX_DEPTH: usize = 14;
const MAX_PREVIEW_LINES: usize = 80;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WindowTarget {
    pub app_id: String,
    pub app_name: String,
    pub window_title: String,
}

#[derive(Debug, Default)]
pub struct ProbeSummary {
    pub application: String,
    pub window: String,
    pub nodes_seen: usize,
    pub visible_nodes: usize,
    pub text_nodes: usize,
    pub document_nodes: usize,
    pub preview: Vec<String>,
}

impl ProbeSummary {
    pub fn quality(&self) -> &'static str {
        if self.preview.len() >= 8 && (self.text_nodes >= 3 || self.document_nodes > 0) {
            "rich"
        } else if self.preview.len() >= 3
            || (!self.preview.is_empty() && (self.text_nodes > 0 || self.document_nodes > 0))
        {
            "partial"
        } else {
            "sparse"
        }
    }
}

/// Inspects only the accessibility tree which AT-SPI marks as the active window.
///
/// This is deliberately read-only: it queries `Accessible` metadata and never
/// constructs or invokes an `Action` or `EditableText` proxy. Nothing is stored.
pub fn inspect_active_window(show_text: bool) -> Result<ProbeSummary> {
    run_bounded_probe(None, show_text).context(
        "GNOME's accessibility interface could not be inspected; make sure Accessibility Context is enabled",
    )
}

/// Inspects the accessibility window which matches GNOME's focused-window identity.
///
/// A target match takes precedence over AT-SPI's active/focused state because some
/// applications retain stale state after losing focus. The function fails closed
/// when it cannot identify one best match.
pub fn inspect_target_window(target: WindowTarget, show_text: bool) -> Result<ProbeSummary> {
    if target.app_id.trim().is_empty()
        && target.app_name.trim().is_empty()
        && target.window_title.trim().is_empty()
    {
        anyhow::bail!("GNOME did not report a focused application or window");
    }
    run_bounded_probe(Some(target), show_text).context(
        "GNOME's focused window could not be matched to an accessibility tree; the application may need to be restarted",
    )
}

fn run_bounded_probe(target: Option<WindowTarget>, show_text: bool) -> Result<ProbeSummary> {
    async_io::block_on(future::race(
        inspect_window_async(target, show_text),
        async {
            async_io::Timer::after(Duration::from_secs(20)).await;
            Err(anyhow::anyhow!(
                "the accessibility inspection exceeded its 20-second safety limit"
            ))
        },
    ))
}

#[derive(Debug)]
struct WindowCandidate {
    object_ref: atspi::ObjectRefOwned,
    application: String,
    score: u16,
}

async fn inspect_window_async(
    target: Option<WindowTarget>,
    show_text: bool,
) -> Result<ProbeSummary> {
    set_session_accessibility(true).await?;
    let accessibility = AccessibilityConnection::new().await?;
    let root = accessibility.root_accessible_on_registry().await?;
    let connection = accessibility.connection();
    let applications = root.get_children().await?;
    let mut best_candidate: Option<WindowCandidate> = None;
    let mut best_is_ambiguous = false;

    for application_ref in applications {
        if application_ref.is_null() {
            continue;
        }
        let application = match application_ref.into_accessible_proxy(connection).await {
            Ok(application) => application,
            Err(_) => continue,
        };
        let application_name = application.name().await.unwrap_or_default();
        let windows = match application.get_children().await {
            Ok(windows) => windows,
            Err(_) => continue,
        };
        for window_ref in windows {
            if window_ref.is_null() {
                continue;
            }
            let window = match window_ref.clone().into_accessible_proxy(connection).await {
                Ok(window) => window,
                Err(_) => continue,
            };
            let states = window.get_state().await.unwrap_or_default();
            let active = states.contains(State::Active);
            let focused = states.contains(State::Focused);
            if let Some(target) = target.as_ref() {
                let window_name = window.name().await.unwrap_or_default();
                let Some(score) = target_candidate_score(
                    target,
                    &application_name,
                    &window_name,
                    active,
                    focused,
                ) else {
                    continue;
                };
                match best_candidate.as_ref() {
                    Some(best) if score < best.score => {}
                    Some(best) if score == best.score => best_is_ambiguous = true,
                    _ => {
                        best_candidate = Some(WindowCandidate {
                            object_ref: window_ref,
                            application: application_name.clone(),
                            score,
                        });
                        best_is_ambiguous = false;
                    }
                }
            } else if active || focused {
                return inspect_tree(connection, window_ref, application_name, show_text).await;
            }
        }
    }

    if let Some(candidate) = best_candidate {
        if best_is_ambiguous {
            anyhow::bail!(
                "more than one accessibility window matched GNOME's focused window equally well"
            );
        }
        return inspect_tree(
            connection,
            candidate.object_ref,
            candidate.application,
            show_text,
        )
        .await;
    }

    Err(anyhow::anyhow!(
        "no matching accessible window was found; the focused application may need to be restarted or may not expose AT-SPI data"
    ))
}

fn target_candidate_score(
    target: &WindowTarget,
    application: &str,
    window: &str,
    active: bool,
    focused: bool,
) -> Option<u16> {
    let app_strength = identity_strength(application, &target.app_name)
        .max(identity_strength(application, &target.app_id));
    let title_strength = title_strength(window, &target.window_title);
    if app_strength == 0 && title_strength < 2 {
        return None;
    }

    let mut score = app_strength * 100 + title_strength * 40;
    if active {
        score += 20;
    }
    if focused {
        score += 10;
    }
    Some(score)
}

fn identity_strength(left: &str, right: &str) -> u16 {
    let left = identity_words(left);
    let right = identity_words(right);
    if left.is_empty() || right.is_empty() {
        return 0;
    }
    if left == right {
        return 3;
    }
    if left.contains(&right) || right.contains(&left) {
        return 2;
    }
    let left_words: HashSet<&str> = left
        .split_whitespace()
        .filter(meaningful_identity_word)
        .collect();
    if right
        .split_whitespace()
        .filter(meaningful_identity_word)
        .any(|word| left_words.contains(word))
    {
        1
    } else {
        0
    }
}

fn title_strength(left: &str, right: &str) -> u16 {
    let left = identity_words(left);
    let right = identity_words(right);
    if left.is_empty() || right.is_empty() {
        return 0;
    }
    if left == right {
        3
    } else if left.len().min(right.len()) >= 6 && (left.contains(&right) || right.contains(&left)) {
        2
    } else {
        0
    }
}

fn identity_words(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn meaningful_identity_word(word: &&str) -> bool {
    word.len() >= 4 && !matches!(*word, "application" | "desktop" | "client")
}

async fn inspect_tree(
    connection: &atspi::zbus::Connection,
    root_ref: atspi::ObjectRefOwned,
    application: String,
    show_text: bool,
) -> Result<ProbeSummary> {
    let mut summary = ProbeSummary {
        application,
        ..ProbeSummary::default()
    };
    let mut stack = vec![(root_ref, 0_usize)];
    let mut preview_seen = HashSet::new();

    while let Some((object_ref, depth)) = stack.pop() {
        if summary.nodes_seen >= MAX_NODES || depth > MAX_DEPTH || object_ref.is_null() {
            continue;
        }
        let accessible = match object_ref.into_accessible_proxy(connection).await {
            Ok(accessible) => accessible,
            Err(_) => continue,
        };
        summary.nodes_seen += 1;
        let role = accessible.get_role().await.unwrap_or(Role::Unknown);
        if role == Role::PasswordText {
            continue;
        }
        let states = accessible.get_state().await.unwrap_or_default();
        let is_root = depth == 0;
        let is_visible =
            is_root || (states.contains(State::Visible) && states.contains(State::Showing));
        if !is_visible {
            continue;
        }
        summary.visible_nodes += 1;
        let interfaces = accessible.get_interfaces().await.unwrap_or_default();
        if interfaces.contains(Interface::Text) {
            summary.text_nodes += 1;
        }
        if interfaces.contains(Interface::Document) {
            summary.document_nodes += 1;
        }
        let name = accessible.name().await.unwrap_or_default();
        if is_root {
            summary.window = name.clone();
        }
        if show_text && summary.preview.len() < MAX_PREVIEW_LINES {
            let description = accessible.description().await.unwrap_or_default();
            let line = semantic_label(role, &name, &description);
            if !line.is_empty() && preview_seen.insert(line.clone()) {
                summary.preview.push(line);
            }
        }
        if depth == MAX_DEPTH || states.contains(State::ManagesDescendants) {
            continue;
        }
        if let Ok(children) = accessible.get_children().await {
            for child in children.into_iter().rev() {
                stack.push((child, depth + 1));
            }
        }
    }
    Ok(summary)
}

fn semantic_label(role: Role, name: &str, description: &str) -> String {
    let name = normalize(name);
    let description = normalize(description);
    match (name.is_empty(), description.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!("{role}: {name}"),
        (true, false) => format!("{role}: {description}"),
        (false, false) if name == description => format!("{role}: {name}"),
        (false, false) => format!("{role}: {name} — {description}"),
    }
}

fn normalize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_labels_are_normalized_and_deduplicatable() {
        assert_eq!(
            semantic_label(Role::Heading, "  Case   notes ", ""),
            "heading: Case notes"
        );
        assert_eq!(semantic_label(Role::Static, "Sent", "Sent"), "static: Sent");
    }

    #[test]
    fn quality_is_conservative() {
        assert_eq!(ProbeSummary::default().quality(), "sparse");
        assert_eq!(
            ProbeSummary {
                text_nodes: 1,
                preview: vec!["text: Inbox".to_owned()],
                ..ProbeSummary::default()
            }
            .quality(),
            "partial"
        );
        assert_eq!(
            ProbeSummary {
                document_nodes: 1,
                preview: (0..8).map(|index| format!("item: {index}")).collect(),
                ..ProbeSummary::default()
            }
            .quality(),
            "rich"
        );
    }

    #[test]
    fn trusted_spotify_identity_rejects_a_stale_keyboard_window() {
        let target = WindowTarget {
            app_id: "spotify_spotify.desktop".to_owned(),
            app_name: "Spotify".to_owned(),
            window_title: "Spotify Premium".to_owned(),
        };
        assert_eq!(
            target_candidate_score(&target, "Das Keyboard Q", "Das Keyboard", true, true),
            None
        );
        assert!(target_candidate_score(&target, "spotify", "Spotify", false, false).is_some());
    }

    #[test]
    fn desktop_id_can_match_an_accessibility_application_name() {
        let target = WindowTarget {
            app_id: "google-chrome.desktop".to_owned(),
            app_name: "Google Chrome".to_owned(),
            window_title: "Inbox - Gmail - Google Chrome".to_owned(),
        };
        assert!(
            target_candidate_score(
                &target,
                "Google Chrome",
                "Inbox - Gmail - Google Chrome",
                true,
                false,
            )
            .is_some()
        );
    }
}

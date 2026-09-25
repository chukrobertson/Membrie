// SPDX-License-Identifier: AGPL-3.0-or-later

use anyhow::{Context, Result};
use atspi::connection::set_session_accessibility;
use atspi::proxy::accessible::ObjectRefExt;
use atspi::{AccessibilityConnection, Interface, Role, State};
use std::collections::HashSet;

const MAX_NODES: usize = 500;
const MAX_DEPTH: usize = 14;
const MAX_PREVIEW_LINES: usize = 80;

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
        if self.text_nodes >= 8 || self.document_nodes > 0 || self.preview.len() >= 8 {
            "rich"
        } else if self.text_nodes > 0 || self.preview.len() >= 3 {
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
    async_io::block_on(inspect_active_window_async(show_text)).context(
        "GNOME's accessibility interface could not be inspected; make sure Accessibility Context is enabled",
    )
}

async fn inspect_active_window_async(show_text: bool) -> Result<ProbeSummary> {
    set_session_accessibility(true).await?;
    let accessibility = AccessibilityConnection::new().await?;
    let root = accessibility.root_accessible_on_registry().await?;
    let connection = accessibility.connection();
    let applications = root.get_children().await?;

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
            if !states.contains(State::Active) && !states.contains(State::Focused) {
                continue;
            }
            return inspect_tree(connection, window_ref, application_name, show_text).await;
        }
    }

    Err(anyhow::anyhow!(
        "no active accessible window was found; the active application may need to be restarted or may not expose AT-SPI data"
    ))
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
                ..ProbeSummary::default()
            }
            .quality(),
            "partial"
        );
        assert_eq!(
            ProbeSummary {
                document_nodes: 1,
                ..ProbeSummary::default()
            }
            .quality(),
            "rich"
        );
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later

use anyhow::{Context, Result};
use atspi::connection::set_session_accessibility;
use atspi::proxy::accessible::ObjectRefExt;
use atspi::proxy::proxy_ext::ProxyExt;
use atspi::{AccessibilityConnection, Interface, Role, State};
use futures_lite::future;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

const MAX_NODES: usize = 500;
const MAX_DEPTH: usize = 14;
const MAX_PREVIEW_LINES: usize = 80;
const MAX_DOCUMENT_TEXT_NODES: usize = 24;
const MAX_DOCUMENT_TEXT_CHARACTERS: usize = 6_000;
const MAX_DOCUMENT_TEXT_PER_NODE: usize = 2_000;
const MAX_DOCUMENT_PREVIEW_LINE_CHARACTERS: usize = 600;
const MAX_SELECTED_DESCENDANTS: usize = 16;
const INTEGRATION_CONTEXT_RESERVED_LINES: usize = 16;

// AT-SPI uses background D-Bus workers. Opening a fresh connection for every
// sample leaves those workers alive in a long-running capture process, even
// after the short-lived inspection future has completed. Reuse one connection
// and replace it only when the bus has actually closed.
static ACCESSIBILITY_CONNECTION: Mutex<Option<AccessibilityConnection>> = Mutex::new(None);

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
    pub integration: Option<String>,
    pub document: Option<String>,
    pub nodes_seen: usize,
    pub visible_nodes: usize,
    pub text_nodes: usize,
    pub document_nodes: usize,
    pub document_text_nodes: usize,
    pub document_text_characters: usize,
    pub preview: Vec<String>,
}

impl ProbeSummary {
    pub fn quality(&self) -> &'static str {
        if self.document_text_characters >= 80
            || (self.preview.len() >= 8 && (self.text_nodes >= 3 || self.document_nodes > 0))
        {
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
        inspect_window_with_shared_connection(target, show_text),
        async {
            async_io::Timer::after(Duration::from_secs(20)).await;
            Err(anyhow::anyhow!(
                "the accessibility inspection exceeded its 20-second safety limit"
            ))
        },
    ))
}

async fn inspect_window_with_shared_connection(
    target: Option<WindowTarget>,
    show_text: bool,
) -> Result<ProbeSummary> {
    let accessibility = shared_accessibility_connection().await?;
    let result = inspect_window_async(&accessibility, target, show_text).await;
    if accessibility.connection().is_closed()
        && let Ok(mut cached) = ACCESSIBILITY_CONNECTION.lock()
    {
        *cached = None;
    }
    result
}

async fn shared_accessibility_connection() -> Result<AccessibilityConnection> {
    if let Some(connection) = ACCESSIBILITY_CONNECTION
        .lock()
        .map_err(|_| anyhow::anyhow!("the accessibility connection state became unavailable"))?
        .as_ref()
        .filter(|connection| !connection.connection().is_closed())
        .cloned()
    {
        return Ok(connection);
    }

    set_session_accessibility(true).await?;
    let connection = AccessibilityConnection::new().await?;
    let mut cached = ACCESSIBILITY_CONNECTION
        .lock()
        .map_err(|_| anyhow::anyhow!("the accessibility connection state became unavailable"))?;
    *cached = Some(connection.clone());
    Ok(connection)
}

#[derive(Debug)]
struct WindowCandidate {
    object_ref: atspi::ObjectRefOwned,
    application: String,
    score: u16,
}

async fn inspect_window_async(
    accessibility: &AccessibilityConnection,
    target: Option<WindowTarget>,
    show_text: bool,
) -> Result<ProbeSummary> {
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
                return inspect_tree(
                    connection,
                    window_ref,
                    application_name,
                    target.as_ref(),
                    show_text,
                )
                .await;
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
            target.as_ref(),
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
    target: Option<&WindowTarget>,
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
            let target_identity = target
                .map(|target| {
                    format!(
                        "{} {} {}",
                        target.app_id, target.app_name, target.window_title
                    )
                })
                .unwrap_or_default();
            if let Some(component) =
                libreoffice_component(&summary.application, &summary.window, &target_identity)
            {
                summary.integration = Some(component.to_owned());
                summary.document = libreoffice_document_title(&summary.window);
                if show_text {
                    for line in libreoffice_identity_lines(component, summary.document.as_deref()) {
                        if summary.preview.len() >= MAX_PREVIEW_LINES {
                            break;
                        }
                        if preview_seen.insert(line.clone()) {
                            summary.preview.push(line);
                        }
                    }
                }
            } else if is_thunderbird(&summary.application, &summary.window, &target_identity) {
                summary.integration = Some("Thunderbird".to_owned());
                let focused_window = target
                    .map(|target| target.window_title.trim())
                    .filter(|window| !window.is_empty())
                    .unwrap_or(summary.window.as_str());
                summary.document = thunderbird_focused_context(focused_window);
                if show_text {
                    for line in thunderbird_identity_lines(summary.document.as_deref()) {
                        if summary.preview.len() >= MAX_PREVIEW_LINES {
                            break;
                        }
                        if preview_seen.insert(line.clone()) {
                            summary.preview.push(line);
                        }
                    }
                }
            }
        }
        if show_text
            && summary.integration.as_deref() == Some("Thunderbird")
            && (states.contains(State::Selected) || states.contains(State::Focused))
            && matches!(role, Role::TreeItem | Role::ListItem | Role::TableRow)
        {
            let selected_name = normalize(&name);
            if !selected_name.is_empty() {
                let selected_line = format!("selected mail item ({role}): {selected_name}");
                if summary.preview.len() < MAX_PREVIEW_LINES
                    && preview_seen.insert(selected_line.clone())
                {
                    summary.preview.push(selected_line);
                }
                if role == Role::TreeItem
                    && let Some(folder) = thunderbird_folder(&selected_name)
                {
                    if summary
                        .document
                        .as_deref()
                        .and_then(thunderbird_folder)
                        .is_none()
                    {
                        summary.document = Some(format!("{folder} folder"));
                    }
                    for line in thunderbird_folder_lines(folder) {
                        if summary.preview.len() >= MAX_PREVIEW_LINES {
                            break;
                        }
                        if preview_seen.insert(line.clone()) {
                            summary.preview.push(line);
                        }
                    }
                }
            }
        }
        let semantic_label_limit = if summary.integration.is_some() {
            MAX_PREVIEW_LINES - INTEGRATION_CONTEXT_RESERVED_LINES
        } else {
            MAX_PREVIEW_LINES
        };
        if show_text && summary.preview.len() < semantic_label_limit {
            let description = accessible.description().await.unwrap_or_default();
            let line = semantic_label(role, &name, &description);
            if !line.is_empty() && preview_seen.insert(line.clone()) {
                summary.preview.push(line);
            }
        }
        if show_text
            && summary.integration.is_some()
            && summary.document_text_nodes < MAX_DOCUMENT_TEXT_NODES
            && summary.document_text_characters < MAX_DOCUMENT_TEXT_CHARACTERS
            && should_read_integration_text(
                summary.integration.as_deref().unwrap_or_default(),
                role,
            )
            && interfaces.contains(Interface::Text)
            && let Ok(proxies) = accessible.proxies().await
            && let Ok(text) = proxies.text().await
            && let Ok(character_count) = text.character_count().await
        {
            let remaining = MAX_DOCUMENT_TEXT_CHARACTERS - summary.document_text_characters;
            let caret = text.caret_offset().await.ok();
            let (start, end) = document_text_range(character_count, caret, remaining);
            if end > start
                && let Ok(excerpt) = text.get_text(start, end).await
            {
                let excerpt = truncate_characters(&normalize(&excerpt), remaining);
                if !excerpt.is_empty() {
                    summary.document_text_nodes += 1;
                    summary.document_text_characters += excerpt.chars().count();
                    let label = integration_content_label(
                        summary.integration.as_deref().unwrap_or_default(),
                        summary.document.as_deref(),
                    );
                    for line in chunk_document_text(label, &excerpt) {
                        if summary.preview.len() >= MAX_PREVIEW_LINES {
                            break;
                        }
                        if preview_seen.insert(line.clone()) {
                            summary.preview.push(line);
                        }
                    }
                }
            }
        }
        if depth == MAX_DEPTH {
            continue;
        }
        if states.contains(State::ManagesDescendants) {
            if summary.integration.is_some()
                && interfaces.contains(Interface::Selection)
                && let Ok(proxies) = accessible.proxies().await
                && let Ok(selection) = proxies.selection().await
                && let Ok(selected_count) = selection.n_selected_children().await
            {
                for index in (0..selected_count.clamp(0, MAX_SELECTED_DESCENDANTS as i32)).rev() {
                    if let Ok(child) = selection.get_selected_child(index).await
                        && !child.is_null()
                    {
                        stack.push((child, depth + 1));
                    }
                }
            }
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

fn libreoffice_component(
    application: &str,
    window: &str,
    target_identity: &str,
) -> Option<&'static str> {
    let identity = format!("{application} {window} {target_identity}").to_ascii_lowercase();
    if !identity.contains("libreoffice") && !identity.contains("soffice") {
        return None;
    }
    for (needle, component) in [
        ("writer", "LibreOffice Writer"),
        ("calc", "LibreOffice Calc"),
        ("impress", "LibreOffice Impress"),
        ("draw", "LibreOffice Draw"),
        ("base", "LibreOffice Base"),
        ("math", "LibreOffice Math"),
    ] {
        if identity.contains(needle) {
            return Some(component);
        }
    }
    Some("LibreOffice")
}

fn libreoffice_document_title(window: &str) -> Option<String> {
    let normalized = normalize(window);
    let lower = normalized.to_ascii_lowercase();
    let mut end = normalized.len();
    for separator in [" - libreoffice", " — libreoffice", " – libreoffice"] {
        if let Some(index) = lower.rfind(separator) {
            end = end.min(index);
        }
    }
    let title = normalized[..end]
        .trim()
        .trim_start_matches(['*', '•'])
        .trim()
        .trim_end_matches('*')
        .trim();
    if title.is_empty()
        || title.eq_ignore_ascii_case("libreoffice")
        || title.eq_ignore_ascii_case("start center")
        || title.eq_ignore_ascii_case("libreoffice start center")
    {
        None
    } else {
        Some(title.to_owned())
    }
}

fn libreoffice_identity_lines(component: &str, document: Option<&str>) -> Vec<String> {
    let mut lines = vec![format!("office application: {component}")];
    if let Some(document) = document {
        lines.push(format!("focused document: {document}"));
    }
    lines
}

fn is_thunderbird(application: &str, window: &str, target_identity: &str) -> bool {
    format!("{application} {window} {target_identity}")
        .to_ascii_lowercase()
        .contains("thunderbird")
}

fn thunderbird_focused_context(window: &str) -> Option<String> {
    let normalized = normalize(window);
    let lower = normalized.to_ascii_lowercase();
    let mut end = normalized.len();
    for suffix in [
        " - mozilla thunderbird",
        " — mozilla thunderbird",
        " – mozilla thunderbird",
        " - thunderbird",
        " — thunderbird",
        " – thunderbird",
    ] {
        if let Some(index) = lower.rfind(suffix) {
            end = end.min(index);
        }
    }
    let context = normalized[..end].trim();
    if context.is_empty()
        || context.eq_ignore_ascii_case("thunderbird")
        || context.eq_ignore_ascii_case("mozilla thunderbird")
    {
        None
    } else {
        Some(context.to_owned())
    }
}

fn thunderbird_folder(context: &str) -> Option<&'static str> {
    for segment in context
        .to_ascii_lowercase()
        .split(['-', '—', '–', '|'])
        .map(str::trim)
    {
        match segment {
            "inbox" => return Some("Inbox"),
            "sent" | "sent mail" => return Some("Sent"),
            "drafts" => return Some("Drafts"),
            "outbox" => return Some("Outbox"),
            "archive" | "archives" => return Some("Archive"),
            "trash" | "deleted" | "deleted items" => return Some("Trash"),
            "junk" | "spam" => return Some("Junk"),
            "all mail" => return Some("All Mail"),
            _ => {}
        }
    }
    None
}

fn thunderbird_is_compose(context: &str) -> bool {
    let context = context.to_ascii_lowercase();
    context.contains("write:") || context.contains("compose") || context.contains("new message")
}

fn thunderbird_identity_lines(context: Option<&str>) -> Vec<String> {
    let context = context.unwrap_or_default();
    let mut lines = vec!["mail application: Thunderbird".to_owned()];
    if thunderbird_is_compose(context) {
        lines.push("mail view: Compose window".to_owned());
        lines.push(
            "evidence boundary: compose or draft context is not proof that the message was sent"
                .to_owned(),
        );
    } else if let Some(folder) = thunderbird_folder(context) {
        lines.extend(thunderbird_folder_lines(folder));
    } else {
        lines.push("mail view: Message or mailbox".to_owned());
        lines.push(
            "evidence boundary: visible mail context does not by itself prove a reply, send, delivery, or reading"
                .to_owned(),
        );
    }
    if !context.is_empty() {
        lines.insert(2, format!("focused mail context: {context}"));
    }
    lines
}

fn thunderbird_folder_lines(folder: &str) -> Vec<String> {
    let boundary = match folder {
        "Sent" => {
            "visible Sent-folder context can support that Thunderbird submitted or stored a sent copy; it does not prove delivery or reading"
        }
        "Drafts" => "Drafts context is not proof that the message was sent",
        "Outbox" => {
            "Outbox context may indicate queued mail; it is not proof of sending or delivery"
        }
        "Inbox" => {
            "visible Inbox context supports that Thunderbird displayed locally synced mail; it does not prove that a response was sent"
        }
        _ => {
            "visible mail-folder context does not by itself prove a reply, send, delivery, or reading"
        }
    };
    vec![
        format!("mail view: {folder} folder"),
        format!("evidence boundary: {boundary}"),
    ]
}

fn integration_content_label(integration: &str, context: Option<&str>) -> &'static str {
    if integration == "Thunderbird" {
        let context = context.unwrap_or_default();
        if thunderbird_is_compose(context) {
            "draft email content"
        } else if thunderbird_folder(context) == Some("Sent") {
            "sent-folder content"
        } else {
            "email content"
        }
    } else if integration.ends_with("Writer") {
        "writer text"
    } else if integration.ends_with("Calc") {
        "sheet content"
    } else if integration.ends_with("Impress") {
        "slide content"
    } else if integration.ends_with("Draw") {
        "drawing content"
    } else if integration.ends_with("Base") {
        "database content"
    } else if integration.ends_with("Math") {
        "formula content"
    } else {
        "application content"
    }
}

fn should_read_integration_text(integration: &str, role: Role) -> bool {
    if integration == "Thunderbird" {
        matches!(
            role,
            Role::DocumentEmail
                | Role::DocumentWeb
                | Role::DocumentFrame
                | Role::Paragraph
                | Role::Text
                | Role::TableCell
                | Role::ListItem
                | Role::Heading
                | Role::Comment
                | Role::Entry
        )
    } else {
        matches!(
            role,
            Role::DocumentText
                | Role::DocumentSpreadsheet
                | Role::DocumentPresentation
                | Role::DocumentFrame
                | Role::Paragraph
                | Role::Text
                | Role::TableCell
                | Role::Heading
                | Role::Comment
        )
    }
}

fn document_text_range(
    character_count: i32,
    caret_offset: Option<i32>,
    remaining: usize,
) -> (i32, i32) {
    let character_count = character_count.max(0);
    let maximum = remaining
        .min(MAX_DOCUMENT_TEXT_PER_NODE)
        .min(i32::MAX as usize) as i32;
    if character_count == 0 || maximum == 0 {
        return (0, 0);
    }
    if character_count <= maximum {
        return (0, character_count);
    }
    let caret = caret_offset.unwrap_or(0).clamp(0, character_count);
    let start = (caret - maximum / 2).clamp(0, character_count - maximum);
    (start, start + maximum)
}

fn chunk_document_text(label: &str, text: &str) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();
    characters
        .chunks(MAX_DOCUMENT_PREVIEW_LINE_CHARACTERS)
        .map(|chunk| format!("{label}: {}", chunk.iter().collect::<String>()))
        .collect()
}

fn truncate_characters(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
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
        assert_eq!(
            ProbeSummary {
                document_text_characters: 80,
                preview: vec!["writer text: bounded document context".to_owned()],
                ..ProbeSummary::default()
            }
            .quality(),
            "rich"
        );
    }

    #[test]
    fn libreoffice_components_and_document_titles_are_explicit() {
        assert_eq!(
            libreoffice_component("soffice", "Case Notes.odt — LibreOffice Writer", ""),
            Some("LibreOffice Writer")
        );
        assert_eq!(
            libreoffice_component("libreoffice-calc", "Budget.ods - LibreOffice Calc", ""),
            Some("LibreOffice Calc")
        );
        assert_eq!(libreoffice_component("Firefox", "Case Notes", ""), None);
        assert_eq!(
            libreoffice_component("soffice", "Budget.ods", "libreoffice-calc.desktop"),
            Some("LibreOffice Calc")
        );
        assert_eq!(
            libreoffice_document_title("* Case Notes.odt — LibreOffice Writer").as_deref(),
            Some("Case Notes.odt")
        );
        assert_eq!(libreoffice_document_title("LibreOffice Start Center"), None);
    }

    #[test]
    fn document_text_ranges_are_bounded_around_the_caret() {
        assert_eq!(document_text_range(120, Some(60), 6_000), (0, 120));
        assert_eq!(
            document_text_range(10_000, Some(5_000), 6_000),
            (4_000, 6_000)
        );
        assert_eq!(document_text_range(10_000, Some(10), 400), (0, 400));
        assert_eq!(document_text_range(0, None, 6_000), (0, 0));
    }

    #[test]
    fn document_text_chunks_preserve_unicode_and_component_labels() {
        let text = "é".repeat(MAX_DOCUMENT_PREVIEW_LINE_CHARACTERS + 1);
        let chunks = chunk_document_text("writer text", &text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            chunks[0]
                .chars()
                .filter(|character| *character == 'é')
                .count(),
            600
        );
        assert_eq!(chunks[1], "writer text: é");
        assert_eq!(truncate_characters("éclair", 2), "éc");
    }

    #[test]
    fn thunderbird_windows_have_truthful_mail_evidence_boundaries() {
        assert!(is_thunderbird(
            "Thunderbird",
            "Inbox - Gmail — Mozilla Thunderbird",
            "thunderbird_thunderbird.desktop"
        ));
        assert_eq!(
            thunderbird_focused_context("Sent - Gmail — Mozilla Thunderbird").as_deref(),
            Some("Sent - Gmail")
        );
        let sent = thunderbird_identity_lines(Some("Sent - Gmail"));
        assert!(sent.iter().any(|line| line == "mail view: Sent folder"));
        assert!(
            sent.iter()
                .any(|line| line.contains("does not prove delivery"))
        );
        let compose = thunderbird_identity_lines(Some("Write: Care plan update"));
        assert!(
            compose
                .iter()
                .any(|line| line == "mail view: Compose window")
        );
        assert!(compose.iter().any(|line| line.contains("not proof")));
        assert_eq!(
            integration_content_label("Thunderbird", Some("Write: Care plan update")),
            "draft email content"
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

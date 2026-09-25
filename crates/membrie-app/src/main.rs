use adw::prelude::*;
use gtk::{Align, Orientation};
use membrie_core::{
    CaptureRule, CaptureStatus, DaemonClient, NewRemembrie, PauseMode, Remembrie, SearchHit,
    socket_path,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "com.chuk.Membrie";
const CLIPBOARD_DBUS_NAME: &str = "com.chuk.Membrie.Clipboard";
const CLIPBOARD_DBUS_PATH: &str = "/com/chuk/Membrie/Clipboard";
const CLIPBOARD_DBUS_INTERFACE: &str = "com.chuk.Membrie.Clipboard";

fn main() -> gtk::glib::ExitCode {
    let application = adw::Application::builder().application_id(APP_ID).build();
    application.connect_startup(|_| install_css());
    application.connect_activate(build_ui);
    application.run()
}

#[derive(Clone)]
struct UiState {
    client: DaemonClient,
    timeline: gtk::ListBox,
    search_results: gtk::ListBox,
    capture_rules: gtk::ListBox,
    privacy_stats: gtk::Label,
    clipboard_button: gtk::Button,
    clipboard_bridge_label: gtk::Label,
    clipboard_agent_label: gtk::Label,
    clipboard_bridge_available: Cell<bool>,
    backup_status: gtk::Label,
    backup_button: gtk::Button,
    backup_in_progress: Cell<bool>,
    timeline_ids: RefCell<Option<Vec<String>>>,
    status_label: gtk::Label,
    pause_button: gtk::MenuButton,
    toast_overlay: adw::ToastOverlay,
}

fn build_ui(application: &adw::Application) {
    let client = DaemonClient::new(socket_path());
    let timeline = memory_list();
    let search_results = memory_list();
    let capture_rules = memory_list();
    let privacy_stats = gtk::Label::new(None);
    privacy_stats.set_xalign(0.0);
    privacy_stats.set_wrap(true);
    let clipboard_button = gtk::Button::with_label("Enable clipboard capture");
    clipboard_button.set_halign(Align::Start);
    let clipboard_bridge_label = gtk::Label::new(None);
    clipboard_bridge_label.set_xalign(0.0);
    clipboard_bridge_label.set_wrap(true);
    let clipboard_agent_label = gtk::Label::new(Some("Clipboard capture service · Checking…"));
    clipboard_agent_label.set_xalign(0.0);
    clipboard_agent_label.set_wrap(true);
    let backup_status = gtk::Label::new(Some("Checking local backups…"));
    backup_status.set_xalign(0.0);
    backup_status.set_wrap(true);
    let backup_button = gtk::Button::with_label("Create backup now");
    backup_button.set_halign(Align::Start);
    let status_label = gtk::Label::new(Some("Connecting…"));
    status_label.add_css_class("dim-label");
    let pause_button = gtk::MenuButton::builder()
        .icon_name("media-playback-pause-symbolic")
        .tooltip_text("Capture controls")
        .build();
    let toast_overlay = adw::ToastOverlay::new();

    let state = Rc::new(UiState {
        client,
        timeline,
        search_results,
        capture_rules,
        privacy_stats,
        clipboard_button,
        clipboard_bridge_label,
        clipboard_agent_label,
        clipboard_bridge_available: Cell::new(false),
        backup_status,
        backup_button,
        backup_in_progress: Cell::new(false),
        timeline_ids: RefCell::new(None),
        status_label,
        pause_button,
        toast_overlay: toast_overlay.clone(),
    });

    let root = gtk::Box::new(Orientation::Vertical, 0);
    let header = build_header(&state);
    root.append(&header);

    let content = gtk::Box::new(Orientation::Horizontal, 0);
    content.add_css_class("content-root");
    let stack = gtk::Stack::builder()
        .hexpand(true)
        .vexpand(true)
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    stack.add_named(&build_timeline_page(&state), Some("timeline"));
    stack.add_named(&build_search_page(&state), Some("search"));
    stack.add_named(&build_brie_page(), Some("brie"));
    stack.add_named(&build_constellation_page(), Some("constellation"));
    stack.add_named(&build_privacy_page(&state), Some("privacy"));
    content.append(&build_sidebar(&stack));
    content.append(&stack);
    root.append(&content);

    toast_overlay.set_child(Some(&root));
    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Membrie")
        .default_width(1120)
        .default_height(760)
        .content(&toast_overlay)
        .build();

    refresh_clipboard_bridge(&state);
    refresh_status(&state);
    refresh_backups(&state);
    refresh_timeline(&state);
    refresh_capture_rules(&state);
    let state_for_live_capture = Rc::clone(&state);
    gtk::glib::timeout_add_seconds_local(2, move || {
        refresh_timeline(&state_for_live_capture);
        refresh_status(&state_for_live_capture);
        gtk::glib::ControlFlow::Continue
    });
    let state_for_timer = Rc::clone(&state);
    gtk::glib::timeout_add_seconds_local(30, move || {
        refresh_clipboard_bridge(&state_for_timer);
        refresh_status(&state_for_timer);
        refresh_backups(&state_for_timer);
        gtk::glib::ControlFlow::Continue
    });
    window.present();
}

fn build_header(state: &Rc<UiState>) -> adw::HeaderBar {
    let header = adw::HeaderBar::new();
    let title = gtk::Box::new(Orientation::Vertical, 0);
    let name = gtk::Label::new(Some("Membrie"));
    name.add_css_class("title");
    let local = gtk::Label::new(Some("Private · Local only"));
    local.add_css_class("caption");
    local.add_css_class("dim-label");
    title.append(&name);
    title.append(&local);
    header.set_title_widget(Some(&title));

    header.pack_start(&state.status_label);
    header.pack_end(&state.pause_button);
    state
        .pause_button
        .set_popover(Some(&build_pause_popover(state)));
    header
}

fn build_pause_popover(state: &Rc<UiState>) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let controls = gtk::Box::new(Orientation::Vertical, 4);
    controls.set_margin_top(8);
    controls.set_margin_bottom(8);
    controls.set_margin_start(8);
    controls.set_margin_end(8);

    let heading = gtk::Label::new(Some("Automatic capture"));
    heading.add_css_class("heading");
    heading.set_xalign(0.0);
    heading.set_margin_bottom(4);
    controls.append(&heading);

    let options: [(&str, Option<i64>); 4] = [
        ("Resume", Some(0)),
        ("Pause for 15 minutes", Some(15 * 60 * 1000)),
        ("Pause for 1 hour", Some(60 * 60 * 1000)),
        ("Pause until resumed", None),
    ];
    for (label, duration_ms) in options {
        let button = gtk::Button::with_label(label);
        button.add_css_class("flat");
        button.set_halign(Align::Fill);
        let state = Rc::clone(state);
        let popover_for_click = popover.clone();
        button.connect_clicked(move |_| {
            let mode = match duration_ms {
                Some(0) => PauseMode::Resume,
                Some(duration) => PauseMode::Until {
                    timestamp_ms: current_time_ms() + duration,
                },
                None => PauseMode::Indefinite,
            };
            match state.client.set_pause(mode) {
                Ok(status) => {
                    apply_status_ui(&state, &status);
                    toast(
                        &state,
                        if status.paused {
                            "Automatic capture paused"
                        } else {
                            "Automatic capture resumed"
                        },
                    );
                }
                Err(error) => toast(&state, &format!("Could not update capture: {error}")),
            }
            popover_for_click.popdown();
        });
        controls.append(&button);
    }
    popover.set_child(Some(&controls));
    popover
}

fn build_sidebar(stack: &gtk::Stack) -> gtk::Box {
    let sidebar = gtk::Box::new(Orientation::Vertical, 6);
    sidebar.add_css_class("sidebar");
    sidebar.set_size_request(210, -1);

    for (label, icon, page) in [
        ("Timeline", "view-list-symbolic", "timeline"),
        ("Search", "system-search-symbolic", "search"),
        ("Brie", "avatar-default-symbolic", "brie"),
        (
            "Constellation",
            "weather-clear-night-symbolic",
            "constellation",
        ),
    ] {
        let button = gtk::Button::builder()
            .label(label)
            .icon_name(icon)
            .halign(Align::Fill)
            .build();
        button.add_css_class("flat");
        button.add_css_class("nav-button");
        let stack = stack.clone();
        button.connect_clicked(move |_| stack.set_visible_child_name(page));
        sidebar.append(&button);
    }

    let spacer = gtk::Box::new(Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    sidebar.append(&spacer);
    let privacy_button = gtk::Button::builder()
        .label("Privacy & Capture")
        .icon_name("security-high-symbolic")
        .halign(Align::Fill)
        .build();
    privacy_button.add_css_class("flat");
    privacy_button.add_css_class("nav-button");
    let privacy_stack = stack.clone();
    privacy_button.connect_clicked(move |_| privacy_stack.set_visible_child_name("privacy"));
    sidebar.append(&privacy_button);
    let privacy = gtk::Label::new(Some("Nothing leaves this PC"));
    privacy.add_css_class("caption");
    privacy.add_css_class("dim-label");
    privacy.set_wrap(true);
    sidebar.append(&privacy);
    sidebar
}

fn build_timeline_page(state: &Rc<UiState>) -> gtk::Widget {
    let page = page_shell("Timeline", "The moments Membrie has kept for you.");
    let capture = gtk::Box::new(Orientation::Vertical, 10);
    capture.add_css_class("card");
    capture.add_css_class("capture-card");

    let capture_heading = gtk::Label::new(Some("Create a Remembrie"));
    capture_heading.add_css_class("heading");
    capture_heading.set_xalign(0.0);
    let title_entry = gtk::Entry::builder()
        .placeholder_text("Title (optional)")
        .hexpand(true)
        .build();
    let body_view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .top_margin(10)
        .bottom_margin(10)
        .left_margin(10)
        .right_margin(10)
        .build();
    body_view.add_css_class("memory-editor");
    body_view.buffer().set_text("");
    let body_frame = gtk::Frame::new(None);
    body_frame.set_child(Some(&body_view));
    body_frame.set_size_request(-1, 100);

    let save = gtk::Button::with_label("Remember this");
    save.add_css_class("suggested-action");
    save.set_halign(Align::End);
    let state_for_save = Rc::clone(state);
    let title_for_save = title_entry.clone();
    let body_for_save = body_view.clone();
    save.connect_clicked(move |_| {
        let buffer = body_for_save.buffer();
        let body = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        let title = title_for_save.text();
        if title.trim().is_empty() && body.trim().is_empty() {
            toast(&state_for_save, "Write something to remember first");
            return;
        }
        match state_for_save
            .client
            .create(NewRemembrie::manual(title.as_str(), body.as_str()))
        {
            Ok(_) => {
                title_for_save.set_text("");
                buffer.set_text("");
                refresh_timeline(&state_for_save);
                refresh_status(&state_for_save);
                toast(&state_for_save, "Remembrie saved locally");
            }
            Err(error) => toast(&state_for_save, &format!("Could not save: {error}")),
        }
    });

    capture.append(&capture_heading);
    capture.append(&title_entry);
    capture.append(&body_frame);
    capture.append(&save);
    page.append(&capture);

    let recent = gtk::Label::new(Some("Recent Remembries"));
    recent.add_css_class("heading");
    recent.set_xalign(0.0);
    page.append(&recent);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&state.timeline)
        .build();
    page.append(&scroll);
    page.upcast()
}

fn build_search_page(state: &Rc<UiState>) -> gtk::Widget {
    let page = page_shell("Search", "Exact local search across every Remembrie.");
    let search_bar = gtk::Box::new(Orientation::Horizontal, 8);
    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Search names, phrases, commands…")
        .hexpand(true)
        .build();
    let button = gtk::Button::with_label("Search");
    button.add_css_class("suggested-action");
    search_bar.append(&entry);
    search_bar.append(&button);
    page.append(&search_bar);

    let perform_search: Rc<dyn Fn()> = {
        let state = Rc::clone(state);
        let entry = entry.clone();
        Rc::new(move || {
            let query = entry.text();
            if query.trim().is_empty() {
                clear_list(&state.search_results);
                return;
            }
            match state.client.search(query.as_str(), 50) {
                Ok(hits) => render_search_hits(&state.search_results, &hits),
                Err(error) => toast(&state, &format!("Search failed: {error}")),
            }
        })
    };
    let search_from_button = Rc::clone(&perform_search);
    button.connect_clicked(move |_| search_from_button());
    let search_from_entry = Rc::clone(&perform_search);
    entry.connect_activate(move |_| search_from_entry());

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&state.search_results)
        .build();
    page.append(&scroll);
    page.upcast()
}

fn build_brie_page() -> gtk::Widget {
    let page = page_shell(
        "Brie",
        "Ask your history—and see exactly which Remembries answer.",
    );
    let empty = adw::StatusPage::builder()
        .icon_name("avatar-default-symbolic")
        .title("Brie is waking up")
        .description("Local retrieval and citation foundations come first. Ollama-backed conversation is the next milestone.")
        .vexpand(true)
        .build();
    page.append(&empty);
    page.upcast()
}

fn build_constellation_page() -> gtk::Widget {
    let page = page_shell(
        "Constellation",
        "Explore the people, topics, and moments connected through your memory.",
    );
    let empty = adw::StatusPage::builder()
        .icon_name("weather-clear-night-symbolic")
        .title("Your sky is still forming")
        .description("Relationships and embeddings will turn Remembries into an explorable, evidence-linked constellation.")
        .vexpand(true)
        .build();
    page.append(&empty);
    page.upcast()
}

fn build_privacy_page(state: &Rc<UiState>) -> gtk::Widget {
    let page = page_shell(
        "Privacy & Capture",
        "Membrie decides what is safe before anything is written to disk.",
    );

    let status_card = gtk::Box::new(Orientation::Vertical, 8);
    status_card.add_css_class("card");
    status_card.add_css_class("capture-card");
    let status_heading = gtk::Label::new(Some("Capture ledger"));
    status_heading.add_css_class("heading");
    status_heading.set_xalign(0.0);
    let status_detail = gtk::Label::new(Some(
        "Skipped events contain only a category and reason. Their original content is never stored.",
    ));
    status_detail.add_css_class("dim-label");
    status_detail.set_xalign(0.0);
    status_detail.set_wrap(true);
    status_card.append(&status_heading);
    status_card.append(&state.privacy_stats);
    status_card.append(&status_detail);
    let clipboard_heading = gtk::Label::new(Some("Clipboard capture"));
    clipboard_heading.add_css_class("heading");
    clipboard_heading.set_xalign(0.0);
    clipboard_heading.set_margin_top(8);
    let clipboard_detail = gtk::Label::new(Some(
        "Off by default. Wayland does not reliably identify which application placed text on the clipboard. Membrie blocks recognizable secrets, but an ordinary-looking password may be indistinguishable from normal text.",
    ));
    clipboard_detail.add_css_class("dim-label");
    clipboard_detail.set_xalign(0.0);
    clipboard_detail.set_wrap(true);
    status_card.append(&clipboard_heading);
    status_card.append(&state.clipboard_bridge_label);
    status_card.append(&state.clipboard_agent_label);
    status_card.append(&clipboard_detail);
    status_card.append(&state.clipboard_button);
    let state_for_clipboard = Rc::clone(state);
    state.clipboard_button.connect_clicked(move |_| {
        let enabled = state_for_clipboard
            .client
            .status()
            .map(|status| !status.clipboard_enabled)
            .unwrap_or(false);
        match state_for_clipboard.client.set_clipboard_enabled(enabled) {
            Ok(status) => {
                apply_status_ui(&state_for_clipboard, &status);
                toast(
                    &state_for_clipboard,
                    if enabled {
                        "Automatic clipboard capture enabled"
                    } else {
                        "Automatic clipboard capture disabled"
                    },
                );
            }
            Err(error) => toast(
                &state_for_clipboard,
                &format!("Could not update clipboard capture: {error}"),
            ),
        }
    });
    page.append(&status_card);

    let backup_card = gtk::Box::new(Orientation::Vertical, 8);
    backup_card.add_css_class("card");
    backup_card.add_css_class("capture-card");
    let backup_heading = gtk::Label::new(Some("Local backups"));
    backup_heading.add_css_class("heading");
    backup_heading.set_xalign(0.0);
    let backup_detail = gtk::Label::new(Some(
        "Membrie creates one verified database snapshot per day and keeps the newest 14. Backups stay alongside your local Membrie data on this computer.",
    ));
    backup_detail.add_css_class("dim-label");
    backup_detail.set_xalign(0.0);
    backup_detail.set_wrap(true);
    backup_card.append(&backup_heading);
    backup_card.append(&state.backup_status);
    backup_card.append(&backup_detail);
    backup_card.append(&state.backup_button);
    let state_for_backup = Rc::clone(state);
    state.backup_button.connect_clicked(move |button| {
        if state_for_backup.backup_in_progress.replace(true) {
            return;
        }
        button.set_sensitive(false);
        button.set_label("Creating backup…");
        let client = state_for_backup.client.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(client.create_backup());
        });
        let state = Rc::clone(&state_for_backup);
        gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
            match receiver.try_recv() {
                Ok(Ok(backup)) => {
                    state.backup_in_progress.set(false);
                    state.backup_button.set_sensitive(true);
                    state.backup_button.set_label("Create backup now");
                    state.backup_status.set_text(&format!(
                        "Latest backup: {} · {}",
                        format_timestamp(backup.created_at_ms),
                        format_file_size(backup.size_bytes)
                    ));
                    toast(&state, "Verified local backup created");
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    state.backup_in_progress.set(false);
                    state.backup_button.set_sensitive(true);
                    state.backup_button.set_label("Create backup now");
                    toast(&state, &format!("Backup failed: {error}"));
                    gtk::glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    state.backup_in_progress.set(false);
                    state.backup_button.set_sensitive(true);
                    state.backup_button.set_label("Create backup now");
                    toast(&state, "Backup worker stopped unexpectedly");
                    gtk::glib::ControlFlow::Break
                }
            }
        });
    });
    page.append(&backup_card);

    let rules_card = gtk::Box::new(Orientation::Vertical, 10);
    rules_card.add_css_class("card");
    rules_card.add_css_class("capture-card");
    let rules_heading = gtk::Label::new(Some("Application exclusions"));
    rules_heading.add_css_class("heading");
    rules_heading.set_xalign(0.0);
    let rules_detail = gtk::Label::new(Some(
        "Built-in password-manager and private-window rules are always protected. Add an application name or identifier; * matches any text.",
    ));
    rules_detail.add_css_class("dim-label");
    rules_detail.set_xalign(0.0);
    rules_detail.set_wrap(true);
    let add_row = gtk::Box::new(Orientation::Horizontal, 8);
    let pattern = gtk::Entry::builder()
        .placeholder_text("For example: *Signal* or org.gnome.Terminal")
        .hexpand(true)
        .build();
    let add = gtk::Button::with_label("Exclude");
    add.add_css_class("suggested-action");
    add_row.append(&pattern);
    add_row.append(&add);
    let state_for_add = Rc::clone(state);
    let pattern_for_add = pattern.clone();
    add.connect_clicked(move |_| {
        let value = pattern_for_add.text();
        if value.trim().is_empty() {
            toast(&state_for_add, "Enter an application pattern first");
            return;
        }
        match state_for_add.client.add_capture_rule(
            "app",
            value.as_str(),
            Some(value.trim().to_owned()),
        ) {
            Ok(_) => {
                pattern_for_add.set_text("");
                refresh_capture_rules(&state_for_add);
                toast(&state_for_add, "Application exclusion added");
            }
            Err(error) => toast(&state_for_add, &format!("Could not add exclusion: {error}")),
        }
    });
    let state_for_entry = Rc::clone(state);
    let add_for_entry = add.clone();
    pattern.connect_activate(move |_| {
        if add_for_entry.is_sensitive() {
            add_for_entry.emit_clicked();
        } else {
            toast(&state_for_entry, "Exclusion controls are unavailable");
        }
    });
    let rules_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(170)
        .max_content_height(260)
        .propagate_natural_height(true)
        .child(&state.capture_rules)
        .build();
    rules_card.append(&rules_heading);
    rules_card.append(&rules_detail);
    rules_card.append(&add_row);
    rules_card.append(&rules_scroll);
    page.append(&rules_card);

    let delete_card = gtk::Box::new(Orientation::Vertical, 8);
    delete_card.add_css_class("card");
    delete_card.add_css_class("capture-card");
    let delete_heading = gtk::Label::new(Some("Delete recent memory"));
    delete_heading.add_css_class("heading");
    delete_heading.set_xalign(0.0);
    let delete_detail = gtk::Label::new(Some(
        "Deletion removes matching Remembries and their search, relationship, and derived records.",
    ));
    delete_detail.add_css_class("dim-label");
    delete_detail.set_xalign(0.0);
    delete_detail.set_wrap(true);
    delete_card.append(&delete_heading);
    delete_card.append(&delete_detail);

    for (label, duration_ms) in [
        ("Delete the last 15 minutes", Some(15 * 60 * 1000_i64)),
        ("Delete the last hour", Some(60 * 60 * 1000_i64)),
        ("Delete the last 24 hours", Some(24 * 60 * 60 * 1000_i64)),
        ("Delete every Remembrie", None),
    ] {
        let button = gtk::Button::with_label(label);
        button.add_css_class("destructive-action");
        button.set_halign(Align::Start);
        let parent = page.clone();
        let state = Rc::clone(state);
        button.connect_clicked(move |_| {
            confirm_delete(&parent, &state, label, duration_ms);
        });
        delete_card.append(&button);
    }
    page.append(&delete_card);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&page)
        .build();
    scroll.upcast()
}

fn confirm_delete(
    parent: &impl IsA<gtk::Widget>,
    state: &Rc<UiState>,
    label: &str,
    duration_ms: Option<i64>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Delete these Remembries?")
        .body("This removes the matching local evidence and derived records. This action cannot be undone.")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", label);
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("cancel"));
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    let state_for_delete = Rc::clone(state);
    dialog.connect_response(Some("delete"), move |_, _| {
        let timestamp_ms = duration_ms
            .map(|duration| current_time_ms().saturating_sub(duration))
            .unwrap_or(0);
        match state_for_delete.client.delete_since(timestamp_ms) {
            Ok(count) => {
                refresh_timeline(&state_for_delete);
                refresh_status(&state_for_delete);
                clear_list(&state_for_delete.search_results);
                toast(
                    &state_for_delete,
                    &format!("Deleted {count} Remembries locally"),
                );
            }
            Err(error) => toast(&state_for_delete, &format!("Deletion failed: {error}")),
        }
    });
    dialog.present(Some(parent));
}

fn page_shell(title: &str, subtitle: &str) -> gtk::Box {
    let page = gtk::Box::new(Orientation::Vertical, 16);
    page.add_css_class("page");
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("page-title");
    heading.set_xalign(0.0);
    let subheading = gtk::Label::new(Some(subtitle));
    subheading.add_css_class("dim-label");
    subheading.set_xalign(0.0);
    subheading.set_wrap(true);
    page.append(&heading);
    page.append(&subheading);
    page
}

fn memory_list() -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list
}

fn refresh_timeline(state: &Rc<UiState>) {
    match state.client.list_recent(100) {
        Ok(remembries) => {
            let ids: Vec<String> = remembries
                .iter()
                .map(|remembrie| remembrie.id.clone())
                .collect();
            if state.timeline_ids.borrow().as_ref() != Some(&ids) {
                render_remembries(&state.timeline, &remembries);
                *state.timeline_ids.borrow_mut() = Some(ids);
            }
        }
        Err(error) => {
            *state.timeline_ids.borrow_mut() = None;
            render_error(
                &state.timeline,
                "Membrie daemon is not available",
                &error.to_string(),
            );
        }
    }
}

fn refresh_clipboard_bridge(state: &UiState) {
    let available = gtk::gio::DBusProxy::for_bus_sync(
        gtk::gio::BusType::Session,
        gtk::gio::DBusProxyFlags::DO_NOT_AUTO_START,
        None,
        CLIPBOARD_DBUS_NAME,
        CLIPBOARD_DBUS_PATH,
        CLIPBOARD_DBUS_INTERFACE,
        None::<&gtk::gio::Cancellable>,
    )
    .ok()
    .is_some_and(|proxy| proxy.name_owner().is_some());
    state.clipboard_bridge_available.set(available);
    if available {
        state
            .clipboard_bridge_label
            .set_text("GNOME Clipboard Bridge · Connected");
        state.clipboard_bridge_label.remove_css_class("error");
        state.clipboard_bridge_label.add_css_class("success");
    } else {
        state.clipboard_bridge_label.set_text(
            "GNOME Clipboard Bridge · Not installed or disabled\nRun ./scripts/install-gnome-extension.sh once. A new installation may require one log out and back in; then restart Membrie.",
        );
        state.clipboard_bridge_label.remove_css_class("success");
        state.clipboard_bridge_label.add_css_class("error");
    }
}

fn refresh_status(state: &Rc<UiState>) {
    match state.client.status() {
        Ok(status) => apply_status_ui(state, &status),
        Err(_) => {
            state.status_label.set_text("Daemon offline");
            state.pause_button.set_sensitive(false);
            state.clipboard_button.set_sensitive(false);
            state
                .clipboard_agent_label
                .set_text("Clipboard capture service · Daemon offline");
            state.clipboard_agent_label.remove_css_class("success");
            state.clipboard_agent_label.add_css_class("error");
            state
                .privacy_stats
                .set_text("Capture statistics are unavailable while the daemon is offline.");
        }
    }
}

fn apply_status_ui(state: &UiState, status: &CaptureStatus) {
    state.pause_button.set_sensitive(true);
    state
        .clipboard_button
        .set_sensitive(state.clipboard_bridge_available.get() || status.clipboard_enabled);
    state.pause_button.set_icon_name(if status.paused {
        "media-playback-start-symbolic"
    } else {
        "media-playback-pause-symbolic"
    });
    let capture_state = if status.paused {
        status
            .paused_until_ms
            .map(|until| {
                let minutes = ((until - current_time_ms()).max(0) + 59_999) / 60_000;
                format!("Paused · {minutes}m remaining")
            })
            .unwrap_or_else(|| "Paused until resumed".to_owned())
    } else {
        "Capture active".to_owned()
    };
    state.status_label.set_text(&format!(
        "{} · {} remembered",
        capture_state, status.remembrie_count
    ));
    state
        .pause_button
        .set_tooltip_text(Some("Capture controls"));
    state
        .clipboard_button
        .set_label(if status.clipboard_enabled {
            "Disable clipboard capture"
        } else {
            "Enable clipboard capture"
        });
    let capture_agent_running = status
        .clipboard_agent_last_seen_ms
        .is_some_and(|last_seen| current_time_ms().saturating_sub(last_seen) <= 30_000);
    if capture_agent_running {
        state
            .clipboard_agent_label
            .set_text("Clipboard capture service · Running");
        state.clipboard_agent_label.remove_css_class("error");
        state.clipboard_agent_label.add_css_class("success");
    } else {
        state
            .clipboard_agent_label
            .set_text("Clipboard capture service · Not responding");
        state.clipboard_agent_label.remove_css_class("success");
        state.clipboard_agent_label.add_css_class("error");
    }
    state.privacy_stats.set_text(&format!(
        "{} Remembries stored · {} automatic events safely skipped\n{} probable secrets blocked · {} duplicates ignored",
        status.remembrie_count,
        status.skipped_total,
        status.skipped_sensitive,
        status.skipped_duplicate
    ));
}

fn refresh_backups(state: &Rc<UiState>) {
    match state.client.list_backups() {
        Ok(backups) => {
            state
                .backup_button
                .set_sensitive(!state.backup_in_progress.get());
            if let Some(backup) = backups.first() {
                state.backup_status.set_text(&format!(
                    "Latest backup: {} · {} · {} stored",
                    format_timestamp(backup.created_at_ms),
                    format_file_size(backup.size_bytes),
                    backups.len()
                ));
            } else {
                state
                    .backup_status
                    .set_text("No verified local backups yet");
            }
        }
        Err(_) => {
            state.backup_button.set_sensitive(false);
            state
                .backup_status
                .set_text("Backups are unavailable while the daemon is offline");
        }
    }
}

fn refresh_capture_rules(state: &Rc<UiState>) {
    match state.client.list_capture_rules() {
        Ok(rules) => render_capture_rules(&state.capture_rules, state, &rules),
        Err(error) => render_error(
            &state.capture_rules,
            "Exclusions unavailable",
            &error.to_string(),
        ),
    }
}

fn render_capture_rules(list: &gtk::ListBox, state: &Rc<UiState>, rules: &[CaptureRule]) {
    clear_list(list);
    for rule in rules {
        let row = gtk::ListBoxRow::new();
        let content = gtk::Box::new(Orientation::Horizontal, 10);
        content.set_margin_top(9);
        content.set_margin_bottom(9);
        content.set_margin_start(12);
        content.set_margin_end(12);
        let labels = gtk::Box::new(Orientation::Vertical, 2);
        labels.set_hexpand(true);
        let title = gtk::Label::new(Some(rule.label.as_deref().unwrap_or(rule.pattern.as_str())));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        let pattern = gtk::Label::new(Some(&format!("{} · {}", rule.rule_type, rule.pattern)));
        pattern.add_css_class("caption");
        pattern.add_css_class("dim-label");
        pattern.set_xalign(0.0);
        labels.append(&title);
        labels.append(&pattern);
        content.append(&labels);

        if rule.is_default {
            let protected = gtk::Image::from_icon_name("changes-prevent-symbolic");
            protected.set_tooltip_text(Some("Built-in safety rule"));
            content.append(&protected);
        } else {
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove exclusion")
                .build();
            remove.add_css_class("flat");
            let state = Rc::clone(state);
            let id = rule.id.clone();
            remove.connect_clicked(move |_| match state.client.delete_capture_rule(&id) {
                Ok(()) => {
                    refresh_capture_rules(&state);
                    toast(&state, "Application exclusion removed");
                }
                Err(error) => toast(&state, &format!("Could not remove exclusion: {error}")),
            });
            content.append(&remove);
        }
        row.set_child(Some(&content));
        list.append(&row);
    }
}

fn render_remembries(list: &gtk::ListBox, remembries: &[Remembrie]) {
    clear_list(list);
    if remembries.is_empty() {
        render_error(
            list,
            "No Remembries yet",
            "Create one above. It will stay entirely on this computer.",
        );
        return;
    }
    for remembrie in remembries {
        list.append(&remembrie_row(remembrie, None));
    }
}

fn render_search_hits(list: &gtk::ListBox, hits: &[SearchHit]) {
    clear_list(list);
    if hits.is_empty() {
        render_error(
            list,
            "No exact matches",
            "Semantic recall will arrive with the hybrid-search milestone.",
        );
        return;
    }
    for hit in hits {
        list.append(&remembrie_row(&hit.remembrie, Some(&hit.snippet)));
    }
}

fn remembrie_row(remembrie: &Remembrie, snippet: Option<&str>) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let content = gtk::Box::new(Orientation::Vertical, 5);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(14);
    content.set_margin_end(14);

    let top = gtk::Box::new(Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some(&remembrie.title));
    title.add_css_class("heading");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let time = gtk::Label::new(Some(&format_timestamp(remembrie.occurred_at_ms)));
    time.add_css_class("caption");
    time.add_css_class("dim-label");
    top.append(&title);
    top.append(&time);
    content.append(&top);

    let text = snippet.unwrap_or(&remembrie.body);
    if !text.is_empty() {
        let preview = gtk::Label::new(Some(text));
        preview.set_xalign(0.0);
        preview.set_wrap(true);
        preview.set_lines(3);
        preview.set_ellipsize(gtk::pango::EllipsizeMode::End);
        preview.add_css_class("memory-preview");
        content.append(&preview);
    }

    let source = gtk::Label::new(Some(&format!(
        "{} · {}",
        remembrie.kind,
        remembrie.source_app.as_deref().unwrap_or("Unknown source")
    )));
    source.add_css_class("caption");
    source.add_css_class("dim-label");
    source.set_xalign(0.0);
    content.append(&source);
    row.set_child(Some(&content));
    row
}

fn render_error(list: &gtk::ListBox, title: &str, detail: &str) {
    clear_list(list);
    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    let content = gtk::Box::new(Orientation::Vertical, 5);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(18);
    content.set_margin_end(18);
    let title = gtk::Label::new(Some(title));
    title.add_css_class("heading");
    let detail = gtk::Label::new(Some(detail));
    detail.add_css_class("dim-label");
    detail.set_wrap(true);
    content.append(&title);
    content.append(&detail);
    row.set_child(Some(&content));
    list.append(&row);
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn format_timestamp(timestamp_ms: i64) -> String {
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .and_then(|date| date.format("%b %-d, %-I:%M %p"))
        .map(|text| text.to_string())
        .unwrap_or_else(|_| "Unknown time".to_owned())
}

fn format_file_size(size_bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * KIB;
    if size_bytes >= MIB as u64 {
        format!("{:.1} MiB", size_bytes as f64 / MIB)
    } else if size_bytes >= KIB as u64 {
        format!("{:.1} KiB", size_bytes as f64 / KIB)
    } else {
        format!("{size_bytes} bytes")
    }
}

fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn toast(state: &UiState, message: &str) {
    state.toast_overlay.add_toast(adw::Toast::new(message));
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        ".content-root { background: @window_bg_color; }
         .sidebar {
             background: color-mix(in srgb, @headerbar_bg_color 92%, @accent_bg_color 8%);
             border-right: 1px solid @borders;
             padding: 18px 12px;
         }
         .nav-button { padding: 10px 12px; }
         .page { padding: 28px 34px; }
         .page-title { font-size: 26px; font-weight: 700; }
         .capture-card { padding: 18px; }
         .memory-editor { background: @view_bg_color; }
         .memory-preview { opacity: 0.88; }
         .title { font-weight: 700; }
         .heading { font-weight: 600; }",
    );
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("a graphical display is required"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

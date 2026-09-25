use adw::prelude::*;
use gtk::{Align, Orientation};
use membrie_core::{
    BrieAnswer, BrieCitation, CaptureRule, CaptureStatus, DaemonClient, IntelligenceSettings,
    IntelligenceStatus, LocalModel, NewRemembrie, PauseMode, Remembrie, SearchHit, socket_path,
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
    activity_status: gtk::Label,
    activity_button: gtk::Button,
    activity_finish_button: gtk::Button,
    activity_idle_combo: gtk::ComboBoxText,
    activity_settings_updating: Cell<bool>,
    backup_status: gtk::Label,
    backup_button: gtk::Button,
    backup_in_progress: Cell<bool>,
    brie_status: gtk::Label,
    brie_retry_button: gtk::Button,
    brie_models_button: gtk::Button,
    brie_messages: gtk::ListBox,
    brie_entry: gtk::Entry,
    brie_send_button: gtk::Button,
    brie_busy: Cell<bool>,
    intelligence_refreshing: Cell<bool>,
    intelligence_settings: RefCell<Option<IntelligenceSettings>>,
    available_models: RefCell<Vec<LocalModel>>,
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
    let clipboard_agent_label = gtk::Label::new(Some("Desktop capture service · Checking…"));
    clipboard_agent_label.set_xalign(0.0);
    clipboard_agent_label.set_wrap(true);
    let activity_status = gtk::Label::new(Some("Activity context · Checking…"));
    activity_status.set_xalign(0.0);
    activity_status.set_wrap(true);
    let activity_button = gtk::Button::with_label("Enable activity context");
    activity_button.set_halign(Align::Start);
    let activity_finish_button = gtk::Button::with_label("Finish current session now");
    activity_finish_button.set_halign(Align::Start);
    activity_finish_button.set_visible(false);
    let activity_idle_combo = gtk::ComboBoxText::new();
    for (id, label) in [
        ("300000", "5 minutes"),
        ("900000", "15 minutes"),
        ("1800000", "30 minutes"),
        ("3600000", "1 hour"),
    ] {
        activity_idle_combo.append(Some(id), label);
    }
    activity_idle_combo.set_active_id(Some("900000"));
    let backup_status = gtk::Label::new(Some("Checking local backups…"));
    backup_status.set_xalign(0.0);
    backup_status.set_wrap(true);
    let backup_button = gtk::Button::with_label("Create backup now");
    backup_button.set_halign(Align::Start);
    let brie_status = gtk::Label::new(Some("Checking local intelligence…"));
    brie_status.set_xalign(0.0);
    brie_status.set_wrap(true);
    let brie_retry_button = gtk::Button::with_label("Retry failed processing");
    brie_retry_button.set_halign(Align::Start);
    brie_retry_button.set_visible(false);
    let brie_models_button = gtk::Button::with_label("Choose local models");
    brie_models_button.set_halign(Align::Start);
    brie_models_button.set_sensitive(false);
    let brie_messages = memory_list();
    let brie_entry = gtk::Entry::builder()
        .placeholder_text("Ask Brie about your Remembries…")
        .hexpand(true)
        .build();
    let brie_send_button = gtk::Button::with_label("Ask Brie");
    brie_send_button.add_css_class("suggested-action");
    brie_send_button.set_sensitive(false);
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
        activity_status,
        activity_button,
        activity_finish_button,
        activity_idle_combo,
        activity_settings_updating: Cell::new(false),
        backup_status,
        backup_button,
        backup_in_progress: Cell::new(false),
        brie_status,
        brie_retry_button,
        brie_models_button,
        brie_messages,
        brie_entry,
        brie_send_button,
        brie_busy: Cell::new(false),
        intelligence_refreshing: Cell::new(false),
        intelligence_settings: RefCell::new(None),
        available_models: RefCell::new(Vec::new()),
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
    stack.add_named(&build_brie_page(&state), Some("brie"));
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
    refresh_intelligence(&state);
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
    let state_for_intelligence = Rc::clone(&state);
    gtk::glib::timeout_add_seconds_local(5, move || {
        refresh_intelligence(&state_for_intelligence);
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
    let page = page_shell(
        "Search",
        "Hybrid local search combines exact language with related meaning.",
    );
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

    let search_busy = Rc::new(Cell::new(false));
    let perform_search: Rc<dyn Fn()> = {
        let state = Rc::clone(state);
        let entry = entry.clone();
        let button = button.clone();
        let search_busy = Rc::clone(&search_busy);
        Rc::new(move || {
            let query = entry.text();
            if query.trim().is_empty() {
                clear_list(&state.search_results);
                return;
            }
            if search_busy.replace(true) {
                return;
            }
            button.set_sensitive(false);
            render_error(
                &state.search_results,
                "Searching locally…",
                "Checking exact text and semantic meaning.",
            );
            let client = state.client.clone();
            let query = query.to_string();
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(client.search(query, 50));
            });
            let state = Rc::clone(&state);
            let button = button.clone();
            let search_busy = Rc::clone(&search_busy);
            gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
                match receiver.try_recv() {
                    Ok(Ok(hits)) => {
                        search_busy.set(false);
                        button.set_sensitive(true);
                        render_search_hits(&state.search_results, &hits);
                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Err(error)) => {
                        search_busy.set(false);
                        button.set_sensitive(true);
                        render_error(
                            &state.search_results,
                            "Search could not finish",
                            &error.to_string(),
                        );
                        gtk::glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        search_busy.set(false);
                        button.set_sensitive(true);
                        toast(&state, "The local search worker stopped unexpectedly");
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
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

fn build_brie_page(state: &Rc<UiState>) -> gtk::Widget {
    let page = page_shell(
        "Brie",
        "Ask your history—and see exactly which Remembries answer.",
    );

    let status_card = gtk::Box::new(Orientation::Vertical, 8);
    status_card.add_css_class("card");
    status_card.add_css_class("capture-card");
    let status_heading = gtk::Label::new(Some("Local intelligence"));
    status_heading.add_css_class("heading");
    status_heading.set_xalign(0.0);
    let privacy = gtk::Label::new(Some(
        "Brie uses only Ollama on this PC. Questions, answers, summaries, and embeddings never leave it.",
    ));
    privacy.add_css_class("dim-label");
    privacy.set_xalign(0.0);
    privacy.set_wrap(true);
    status_card.append(&status_heading);
    status_card.append(&state.brie_status);
    status_card.append(&privacy);
    status_card.append(&state.brie_retry_button);
    status_card.append(&state.brie_models_button);
    page.append(&status_card);

    append_brie_message(
        &state.brie_messages,
        "Brie",
        "I'm here. Ask me about anything Membrie has remembered, and I'll show the Remembries supporting my answer.",
        &[],
    );
    let conversation = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .min_content_height(260)
        .child(&state.brie_messages)
        .build();
    page.append(&conversation);

    let ask_row = gtk::Box::new(Orientation::Horizontal, 8);
    ask_row.append(&state.brie_entry);
    ask_row.append(&state.brie_send_button);
    page.append(&ask_row);

    let ask: Rc<dyn Fn()> = {
        let state = Rc::clone(state);
        Rc::new(move || {
            let question = state.brie_entry.text().trim().to_owned();
            if question.is_empty() || state.brie_busy.replace(true) {
                return;
            }
            state.brie_entry.set_text("");
            state.brie_entry.set_sensitive(false);
            state.brie_send_button.set_sensitive(false);
            state.brie_send_button.set_label("Brie is thinking…");
            append_brie_message(&state.brie_messages, "You", &question, &[]);
            scroll_list_to_bottom(&conversation);

            let client = state.client.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(client.ask_brie(question));
            });
            let state = Rc::clone(&state);
            let conversation = conversation.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
                match receiver.try_recv() {
                    Ok(Ok(answer)) => {
                        finish_brie_request(&state);
                        append_brie_answer(&state.brie_messages, &answer);
                        scroll_list_to_bottom(&conversation);
                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Err(error)) => {
                        finish_brie_request(&state);
                        append_brie_message(
                            &state.brie_messages,
                            "Brie",
                            &format!("I couldn't complete that locally: {error}"),
                            &[],
                        );
                        scroll_list_to_bottom(&conversation);
                        gtk::glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        finish_brie_request(&state);
                        toast(&state, "Brie's local worker stopped unexpectedly");
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
        })
    };
    let ask_from_button = Rc::clone(&ask);
    state
        .brie_send_button
        .connect_clicked(move |_| ask_from_button());
    let ask_from_entry = Rc::clone(&ask);
    state.brie_entry.connect_activate(move |_| ask_from_entry());

    let state_for_retry = Rc::clone(state);
    state.brie_retry_button.connect_clicked(move |_| {
        match state_for_retry.client.retry_intelligence() {
            Ok(count) => {
                toast(
                    &state_for_retry,
                    &format!("Queued {count} Remembries for another local attempt"),
                );
                refresh_intelligence(&state_for_retry);
            }
            Err(error) => toast(&state_for_retry, &format!("Could not retry: {error}")),
        }
    });
    let state_for_models = Rc::clone(state);
    state
        .brie_models_button
        .connect_clicked(move |button| show_model_dialog(button, &state_for_models));

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

    let activity_card = gtk::Box::new(Orientation::Vertical, 8);
    activity_card.add_css_class("card");
    activity_card.add_css_class("capture-card");
    let activity_heading = gtk::Label::new(Some("Activity context"));
    activity_heading.add_css_class("heading");
    activity_heading.set_xalign(0.0);
    let activity_detail = gtk::Label::new(Some(
        "Off by default. When enabled, Membrie records focused application and window-title changes, then closes a session after inactivity. This phase does not take screenshots or record keyboard input.",
    ));
    activity_detail.add_css_class("dim-label");
    activity_detail.set_xalign(0.0);
    activity_detail.set_wrap(true);
    let idle_row = gtk::Box::new(Orientation::Horizontal, 10);
    let idle_label = gtk::Label::new(Some("Start a new session after"));
    idle_label.set_xalign(0.0);
    idle_label.set_hexpand(true);
    idle_row.append(&idle_label);
    idle_row.append(&state.activity_idle_combo);
    let screen_note = gtk::Label::new(Some(
        "Screen content · Not captured yet. Keyframes, OCR, retention, and visual-model controls come after this activity foundation is proven.",
    ));
    screen_note.add_css_class("dim-label");
    screen_note.set_xalign(0.0);
    screen_note.set_wrap(true);
    activity_card.append(&activity_heading);
    activity_card.append(&state.activity_status);
    activity_card.append(&activity_detail);
    activity_card.append(&idle_row);
    activity_card.append(&state.activity_button);
    activity_card.append(&state.activity_finish_button);
    activity_card.append(&screen_note);
    let state_for_activity = Rc::clone(state);
    state.activity_button.connect_clicked(move |_| {
        let enabled = state_for_activity
            .client
            .status()
            .map(|status| !status.activity_enabled)
            .unwrap_or(false);
        match state_for_activity.client.set_activity_enabled(enabled) {
            Ok(status) => {
                apply_status_ui(&state_for_activity, &status);
                refresh_timeline(&state_for_activity);
                toast(
                    &state_for_activity,
                    if enabled {
                        "Activity context enabled—no screenshots are being taken"
                    } else {
                        "Activity context disabled"
                    },
                );
            }
            Err(error) => toast(
                &state_for_activity,
                &format!("Could not update activity context: {error}"),
            ),
        }
    });
    let state_for_idle = Rc::clone(state);
    state.activity_idle_combo.connect_changed(move |combo| {
        if state_for_idle.activity_settings_updating.get() {
            return;
        }
        let Some(value) = combo
            .active_id()
            .and_then(|value| value.parse::<u64>().ok())
        else {
            return;
        };
        match state_for_idle.client.set_activity_idle_threshold(value) {
            Ok(status) => {
                apply_status_ui(&state_for_idle, &status);
                toast(&state_for_idle, "Activity-session boundary updated");
            }
            Err(error) => {
                toast(
                    &state_for_idle,
                    &format!("Could not update session timing: {error}"),
                );
                refresh_status(&state_for_idle);
            }
        }
    });
    let state_for_finish = Rc::clone(state);
    state.activity_finish_button.connect_clicked(move |_| {
        match state_for_finish.client.end_activity_session("manual") {
            Ok(()) => {
                refresh_status(&state_for_finish);
                refresh_timeline(&state_for_finish);
                refresh_intelligence(&state_for_finish);
                toast(&state_for_finish, "Activity session saved as a Remembrie");
            }
            Err(error) => toast(
                &state_for_finish,
                &format!("Could not finish the activity session: {error}"),
            ),
        }
    });
    page.append(&activity_card);

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
        "Deletion removes matching Remembries, overlapping activity sessions, and their search, relationship, and derived records.",
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
        .body("This removes the matching local evidence, overlapping activity sessions, and derived records. This action cannot be undone.")
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
            .set_text("GNOME Desktop Bridge · Connected");
        state.clipboard_bridge_label.remove_css_class("error");
        state.clipboard_bridge_label.add_css_class("success");
    } else {
        state.clipboard_bridge_label.set_text(
            "GNOME Desktop Bridge · Not installed or disabled\nRun ./scripts/install-gnome-extension.sh once. A new installation may require one log out and back in; then restart Membrie.",
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
            state.activity_button.set_sensitive(false);
            state.activity_finish_button.set_sensitive(false);
            state.activity_idle_combo.set_sensitive(false);
            state
                .clipboard_agent_label
                .set_text("Desktop capture service · Daemon offline");
            state.clipboard_agent_label.remove_css_class("success");
            state.clipboard_agent_label.add_css_class("error");
            state
                .privacy_stats
                .set_text("Capture statistics are unavailable while the daemon is offline.");
            state
                .activity_status
                .set_text("Activity context · Daemon offline");
        }
    }
}

fn show_model_dialog(parent: &gtk::Button, state: &Rc<UiState>) {
    let Some(settings) = state.intelligence_settings.borrow().clone() else {
        toast(state, "Local model information is still loading");
        return;
    };
    let models = state.available_models.borrow().clone();
    if models.is_empty() {
        toast(state, "No local Ollama models are available");
        return;
    }

    let controls = gtk::Box::new(Orientation::Vertical, 10);
    let chat_label = gtk::Label::new(Some("Brie and summary model"));
    chat_label.add_css_class("heading");
    chat_label.set_xalign(0.0);
    let chat_models = gtk::ComboBoxText::new();
    for model in models.iter().filter(|model| {
        let name = model.name.to_ascii_lowercase();
        !name.contains("embed") && !name.contains("whisper")
    }) {
        chat_models.append(Some(&model.name), &model.name);
    }
    if !chat_models.set_active_id(Some(&settings.chat_model)) {
        chat_models.append(Some(&settings.chat_model), &settings.chat_model);
        chat_models.set_active_id(Some(&settings.chat_model));
    }

    let embedding_label = gtk::Label::new(Some("Semantic search model"));
    embedding_label.add_css_class("heading");
    embedding_label.set_xalign(0.0);
    let embedding_models = gtk::ComboBoxText::new();
    for model in models.iter().filter(|model| {
        let name = model.name.to_ascii_lowercase();
        name.contains("embed")
    }) {
        embedding_models.append(Some(&model.name), &model.name);
    }
    if !embedding_models.set_active_id(Some(&settings.embedding_model)) {
        embedding_models.append(Some(&settings.embedding_model), &settings.embedding_model);
        embedding_models.set_active_id(Some(&settings.embedding_model));
    }

    let context_label = gtk::Label::new(Some("Working context (tokens)"));
    context_label.add_css_class("heading");
    context_label.set_xalign(0.0);
    let context = gtk::SpinButton::with_range(2048.0, 32768.0, 1024.0);
    context.set_value(f64::from(settings.context_tokens));
    context.set_tooltip_text(Some("8192 is recommended for Gemma 4 12B on this computer"));

    controls.append(&chat_label);
    controls.append(&chat_models);
    controls.append(&embedding_label);
    controls.append(&embedding_models);
    controls.append(&context_label);
    controls.append(&context);

    let dialog = adw::AlertDialog::builder()
        .heading("Local intelligence models")
        .body("Changing either model safely re-queues derived summaries and embeddings. Original Remembries are not changed.")
        .extra_child(&controls)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("save", "Save and reprocess");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("save"));
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    let state_for_save = Rc::clone(state);
    dialog.connect_response(Some("save"), move |_, _| {
        let Some(chat_model) = chat_models.active_id() else {
            toast(&state_for_save, "Choose a model for Brie");
            return;
        };
        let Some(embedding_model) = embedding_models.active_id() else {
            toast(&state_for_save, "Choose a semantic search model");
            return;
        };
        let settings = IntelligenceSettings {
            chat_model: chat_model.to_string(),
            embedding_model: embedding_model.to_string(),
            context_tokens: context.value_as_int() as u32,
        };
        state_for_save.brie_models_button.set_sensitive(false);
        state_for_save
            .brie_status
            .set_text("Saving local models and re-queuing derived data…");
        let client = state_for_save.client.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(client.set_intelligence_settings(settings));
        });
        let state = Rc::clone(&state_for_save);
        gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
            match receiver.try_recv() {
                Ok(Ok(status)) => {
                    apply_intelligence_status(&state, &status);
                    toast(&state, "Local intelligence models updated");
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    state.brie_models_button.set_sensitive(true);
                    toast(&state, &format!("Could not update models: {error}"));
                    refresh_intelligence(&state);
                    gtk::glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    state.brie_models_button.set_sensitive(true);
                    toast(&state, "The local model update stopped unexpectedly");
                    gtk::glib::ControlFlow::Break
                }
            }
        });
    });
    dialog.present(Some(parent));
}

fn refresh_intelligence(state: &Rc<UiState>) {
    if state.intelligence_refreshing.replace(true) {
        return;
    }
    let client = state.client.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(client.intelligence_status());
    });
    let state = Rc::clone(state);
    gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
        match receiver.try_recv() {
            Ok(Ok(status)) => {
                state.intelligence_refreshing.set(false);
                apply_intelligence_status(&state, &status);
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(error)) => {
                state.intelligence_refreshing.set(false);
                state
                    .brie_status
                    .set_text(&format!("Local intelligence unavailable · {error}"));
                state.brie_status.remove_css_class("success");
                state.brie_status.add_css_class("error");
                state.brie_send_button.set_sensitive(false);
                gtk::glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                state.intelligence_refreshing.set(false);
                state
                    .brie_status
                    .set_text("Local intelligence status stopped unexpectedly");
                state.brie_send_button.set_sensitive(false);
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn apply_intelligence_status(state: &UiState, status: &IntelligenceStatus) {
    if !status.ollama_available {
        state.brie_status.set_text(
            "Ollama is not responding locally. Start Ollama and Brie will resume automatically.",
        );
        state.brie_status.remove_css_class("success");
        state.brie_status.add_css_class("error");
        state.brie_send_button.set_sensitive(false);
        state.brie_models_button.set_sensitive(false);
        return;
    }

    *state.intelligence_settings.borrow_mut() = Some(status.settings.clone());
    *state.available_models.borrow_mut() = status.available_models.clone();

    let processing = if status.running_jobs > 0 {
        format!(" · processing {} now", status.running_jobs)
    } else if status.pending_jobs > 0 {
        format!(" · {} queued", status.pending_jobs)
    } else {
        String::new()
    };
    state.brie_status.set_text(&format!(
        "Ollama connected · {} · {} of {} Remembries indexed{}\nEmbeddings: {} · Context: {} tokens",
        status.settings.chat_model,
        status.indexed_remembries,
        status.total_remembries,
        processing,
        status.settings.embedding_model,
        status.settings.context_tokens
    ));
    state.brie_status.remove_css_class("error");
    state.brie_status.add_css_class("success");
    state.brie_retry_button.set_visible(status.failed_jobs > 0);
    state
        .brie_retry_button
        .set_label(&format!("Retry {} failed Remembries", status.failed_jobs));
    state
        .brie_send_button
        .set_sensitive(status.total_remembries > 0 && !state.brie_busy.get());
    state.brie_models_button.set_sensitive(true);
}

fn finish_brie_request(state: &UiState) {
    state.brie_busy.set(false);
    state.brie_entry.set_sensitive(true);
    state.brie_send_button.set_sensitive(true);
    state.brie_send_button.set_label("Ask Brie");
    state.brie_entry.grab_focus();
}

fn append_brie_answer(list: &gtk::ListBox, answer: &BrieAnswer) {
    append_brie_message(list, "Brie", &answer.answer, &answer.citations);
}

fn append_brie_message(
    list: &gtk::ListBox,
    speaker: &str,
    message: &str,
    citations: &[BrieCitation],
) {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    let content = gtk::Box::new(Orientation::Vertical, 6);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(14);
    content.set_margin_end(14);
    content.add_css_class(if speaker == "You" {
        "chat-user"
    } else {
        "chat-assistant"
    });

    let speaker_label = gtk::Label::new(Some(speaker));
    speaker_label.add_css_class("heading");
    speaker_label.set_xalign(0.0);
    let message_label = gtk::Label::new(Some(message));
    message_label.set_xalign(0.0);
    message_label.set_wrap(true);
    message_label.set_selectable(true);
    content.append(&speaker_label);
    content.append(&message_label);

    if !citations.is_empty() {
        let source_heading = gtk::Label::new(Some("Supporting Remembries"));
        source_heading.add_css_class("caption");
        source_heading.add_css_class("dim-label");
        source_heading.set_xalign(0.0);
        source_heading.set_margin_top(4);
        content.append(&source_heading);
        for citation in citations {
            let label = format!("[{}] {}", citation.number, citation.remembrie.title);
            let button = gtk::Button::with_label(&label);
            button.add_css_class("flat");
            button.set_halign(Align::Start);
            button.set_tooltip_text(Some("Open the exact supporting Remembrie"));
            let citation = citation.clone();
            button.connect_clicked(move |button| {
                let source = citation
                    .remembrie
                    .source_app
                    .as_deref()
                    .unwrap_or("Unknown source");
                let body = if citation.remembrie.body.trim().is_empty() {
                    citation.excerpt.as_str()
                } else {
                    citation.remembrie.body.as_str()
                };
                let dialog = adw::AlertDialog::builder()
                    .heading(&citation.remembrie.title)
                    .body(format!(
                        "{} · {}\n\n{}",
                        format_timestamp(citation.remembrie.occurred_at_ms),
                        source,
                        truncate_display_text(body, 5000)
                    ))
                    .build();
                dialog.add_response("close", "Close");
                dialog.set_default_response(Some("close"));
                dialog.present(Some(button));
            });
            content.append(&button);
        }
    }
    row.set_child(Some(&content));
    list.append(&row);
}

fn scroll_list_to_bottom(scroll: &gtk::ScrolledWindow) {
    let adjustment = scroll.vadjustment();
    gtk::glib::idle_add_local_once(move || {
        adjustment.set_value(adjustment.upper() - adjustment.page_size());
    });
}

fn truncate_display_text(text: &str, maximum: usize) -> String {
    let mut result: String = text.chars().take(maximum).collect();
    if text.chars().count() > maximum {
        result.push('…');
    }
    result
}

fn apply_status_ui(state: &UiState, status: &CaptureStatus) {
    state.pause_button.set_sensitive(true);
    state
        .clipboard_button
        .set_sensitive(state.clipboard_bridge_available.get() || status.clipboard_enabled);
    state
        .activity_button
        .set_sensitive(state.clipboard_bridge_available.get() || status.activity_enabled);
    state
        .activity_idle_combo
        .set_sensitive(status.activity_enabled);
    state
        .activity_finish_button
        .set_visible(status.activity_active_since_ms.is_some());
    state
        .activity_finish_button
        .set_sensitive(status.activity_active_since_ms.is_some());
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
    state.activity_button.set_label(if status.activity_enabled {
        "Disable activity context"
    } else {
        "Enable activity context"
    });
    state.activity_settings_updating.set(true);
    state
        .activity_idle_combo
        .set_active_id(Some(&status.activity_idle_threshold_ms.to_string()));
    state.activity_settings_updating.set(false);
    let capture_agent_running = status
        .clipboard_agent_last_seen_ms
        .is_some_and(|last_seen| current_time_ms().saturating_sub(last_seen) <= 30_000);
    if capture_agent_running {
        state
            .clipboard_agent_label
            .set_text("Desktop capture service · Running");
        state.clipboard_agent_label.remove_css_class("error");
        state.clipboard_agent_label.add_css_class("success");
    } else {
        state
            .clipboard_agent_label
            .set_text("Desktop capture service · Not responding");
        state.clipboard_agent_label.remove_css_class("success");
        state.clipboard_agent_label.add_css_class("error");
    }
    let activity_text = if !status.activity_enabled {
        format!(
            "Off · {} completed activity sessions stored",
            status.activity_session_count
        )
    } else if !capture_agent_running {
        "Enabled, but the desktop capture service is not responding".to_owned()
    } else if let Some(started_at) = status.activity_active_since_ms {
        let app = status.activity_current_app.as_deref().unwrap_or("Desktop");
        let window = status
            .activity_current_window
            .as_deref()
            .filter(|title| !title.is_empty())
            .map(|title| format!(" · {title}"))
            .unwrap_or_default();
        format!(
            "Recording local context since {} · {app}{window}\n{} completed sessions stored",
            format_timestamp(started_at),
            status.activity_session_count
        )
    } else {
        format!(
            "Enabled · waiting for active desktop context\n{} completed sessions stored",
            status.activity_session_count
        )
    };
    state.activity_status.set_text(&activity_text);
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
            "No matching Remembries",
            "Try a different phrase or let local indexing finish.",
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
         .chat-user { border-left: 3px solid @accent_bg_color; padding-left: 10px; }
         .chat-assistant { border-left: 3px solid @success_color; padding-left: 10px; }
         .title { font-weight: 700; }
         .heading { font-weight: 600; }",
    );
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("a graphical display is required"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

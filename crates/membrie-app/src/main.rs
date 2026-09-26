use adw::prelude::*;
use gtk::{Align, Orientation};
use membrie_a11y::{ProbeSummary, WindowTarget};
use membrie_core::{
    ActivitySnapshot, BrieAnswer, BrieCitation, CaptureRule, CaptureStatus, DaemonClient,
    IntelligenceSettings, IntelligenceStatus, LocalModel, NewRemembrie, PauseMode, Remembrie,
    ScreenCaptureCandidate, ScreenCaptureResult, SearchHit, TimelineActivityObservation,
    TimelineEntry, TimelineMapSlice, socket_path,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "com.chuk.Membrie";
const CLIPBOARD_DBUS_NAME: &str = "com.chuk.Membrie.Clipboard";
const CLIPBOARD_DBUS_PATH: &str = "/com/chuk/Membrie/Clipboard";
const CLIPBOARD_DBUS_INTERFACE: &str = "com.chuk.Membrie.Clipboard";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimelineMapMode {
    Day,
    Week,
    Month,
}

fn main() -> gtk::glib::ExitCode {
    let application = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gtk::gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    application.add_main_option(
        "quick",
        b'\0'.into(),
        gtk::glib::OptionFlags::NONE,
        gtk::glib::OptionArg::None,
        "Open Quick Brie",
        None,
    );
    application.connect_startup(|_| install_css());
    application.connect_activate(activate_main_ui);
    application.connect_command_line(|application, command_line| {
        if command_line.options_dict().contains("quick") {
            toggle_quick_brie(application);
        } else {
            application.activate();
        }
        0
    });
    application.run()
}

fn activate_main_ui(application: &adw::Application) {
    if let Some(window) = application
        .windows()
        .into_iter()
        .find(|window| window.widget_name() == "membrie-main-window")
    {
        window.present();
        return;
    }
    build_ui(application);
}

#[derive(Clone)]
struct UiState {
    client: DaemonClient,
    timeline: gtk::ListBox,
    timeline_history_grid: gtk::Grid,
    timeline_ribbon_grid: gtk::Grid,
    timeline_ribbon_axis: gtk::Box,
    timeline_app_legend: gtk::FlowBox,
    timeline_day_label: gtk::Label,
    timeline_next_button: gtk::Button,
    timeline_today_button: gtk::Button,
    timeline_insight: gtk::Box,
    timeline_insight_label: gtk::Label,
    timeline_day_start_ms: Cell<i64>,
    timeline_map_mode: Cell<TimelineMapMode>,
    timeline_target_id: RefCell<Option<String>>,
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
    semantic_status: gtk::Label,
    semantic_capture_button: gtk::Button,
    semantic_button: gtk::Button,
    semantic_probe_busy: Cell<bool>,
    semantic_interval_combo: gtk::ComboBoxText,
    semantic_settings_updating: Cell<bool>,
    screen_status: gtk::Label,
    screen_button: gtk::Button,
    screen_capture_now_button: gtk::Button,
    screen_capture_now_busy: Cell<bool>,
    screen_interval_combo: gtk::ComboBoxText,
    screen_model_combo: gtk::ComboBoxText,
    screen_settings_updating: Cell<bool>,
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
    timeline_render_key: RefCell<Option<String>>,
    status_label: gtk::Label,
    pause_button: gtk::MenuButton,
    toast_overlay: adw::ToastOverlay,
}

fn build_ui(application: &adw::Application) {
    let client = DaemonClient::new(socket_path());
    let timeline = memory_list();
    let timeline_history_grid = gtk::Grid::builder()
        .column_spacing(4)
        .row_spacing(4)
        .column_homogeneous(true)
        .build();
    timeline_history_grid.add_css_class("timeline-history-grid");
    let timeline_ribbon_grid = gtk::Grid::builder().column_homogeneous(true).build();
    timeline_ribbon_grid.add_css_class("timeline-ribbon-grid");
    let timeline_ribbon_axis = gtk::Box::new(Orientation::Horizontal, 0);
    timeline_ribbon_axis.set_homogeneous(true);
    let timeline_app_legend = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(8)
        .row_spacing(6)
        .max_children_per_line(6)
        .build();
    let timeline_day_label = gtk::Label::new(None);
    timeline_day_label.add_css_class("page-title");
    timeline_day_label.set_xalign(0.0);
    timeline_day_label.set_hexpand(true);
    let timeline_next_button = gtk::Button::builder()
        .icon_name("go-next-symbolic")
        .tooltip_text("Next day")
        .build();
    let timeline_today_button = gtk::Button::with_label("Today");
    let timeline_insight = gtk::Box::new(Orientation::Vertical, 5);
    timeline_insight.add_css_class("card");
    timeline_insight.add_css_class("timeline-insight");
    timeline_insight.set_visible(false);
    let timeline_insight_label = gtk::Label::new(None);
    timeline_insight_label.set_xalign(0.0);
    timeline_insight_label.set_wrap(true);
    let timeline_day_start_ms = local_today_start_ms();
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
    let semantic_status = gtk::Label::new(Some("Semantic Context · Checking…"));
    semantic_status.set_xalign(0.0);
    semantic_status.set_wrap(true);
    let semantic_button = gtk::Button::with_label("Enable and test semantic context");
    semantic_button.set_halign(Align::Start);
    let semantic_capture_button = gtk::Button::with_label("Enable automatic semantic context");
    semantic_capture_button.set_halign(Align::Start);
    let semantic_interval_combo = gtk::ComboBoxText::new();
    for (id, label) in [
        ("30000", "30 seconds"),
        ("60000", "1 minute"),
        ("120000", "2 minutes"),
        ("300000", "5 minutes"),
    ] {
        semantic_interval_combo.append(Some(id), label);
    }
    semantic_interval_combo.set_active_id(Some("60000"));
    let screen_status = gtk::Label::new(Some("Screen memory · Checking…"));
    screen_status.set_xalign(0.0);
    screen_status.set_wrap(true);
    let screen_button = gtk::Button::with_label("Enable screen memory");
    screen_button.set_halign(Align::Start);
    let screen_capture_now_button = gtk::Button::with_label("Remember this screen in 5 seconds");
    screen_capture_now_button.set_halign(Align::Start);
    screen_capture_now_button.set_sensitive(false);
    let screen_interval_combo = gtk::ComboBoxText::new();
    for (id, label) in [
        ("30000", "30 seconds"),
        ("60000", "1 minute"),
        ("120000", "2 minutes"),
        ("300000", "5 minutes"),
    ] {
        screen_interval_combo.append(Some(id), label);
    }
    screen_interval_combo.set_active_id(Some("120000"));
    let screen_model_combo = gtk::ComboBoxText::new();
    screen_model_combo.append(Some("gemma4:e2b"), "gemma4:e2b");
    screen_model_combo.set_active_id(Some("gemma4:e2b"));
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
        timeline_history_grid,
        timeline_ribbon_grid,
        timeline_ribbon_axis,
        timeline_app_legend,
        timeline_day_label,
        timeline_next_button,
        timeline_today_button,
        timeline_insight,
        timeline_insight_label,
        timeline_day_start_ms: Cell::new(timeline_day_start_ms),
        timeline_map_mode: Cell::new(TimelineMapMode::Week),
        timeline_target_id: RefCell::new(None),
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
        semantic_status,
        semantic_capture_button,
        semantic_button,
        semantic_probe_busy: Cell::new(false),
        semantic_interval_combo,
        semantic_settings_updating: Cell::new(false),
        screen_status,
        screen_button,
        screen_capture_now_button,
        screen_capture_now_busy: Cell::new(false),
        screen_interval_combo,
        screen_model_combo,
        screen_settings_updating: Cell::new(false),
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
        timeline_render_key: RefCell::new(None),
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
    stack.add_named(&build_brie_page(&state, &stack), Some("brie"));
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
    window.set_widget_name("membrie-main-window");

    refresh_clipboard_bridge(&state);
    refresh_semantic_status(&state);
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
        refresh_semantic_status(&state_for_timer);
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

fn toggle_quick_brie(application: &adw::Application) {
    if let Some(window) = application
        .windows()
        .into_iter()
        .find(|window| window.widget_name() == "membrie-quick-window")
    {
        if window.is_active() {
            window.close();
        } else {
            window.present();
        }
        return;
    }

    let client = DaemonClient::new(socket_path());
    let desktop_target = focused_semantic_target().ok();
    let content = gtk::Box::new(Orientation::Vertical, 10);
    content.add_css_class("quick-brie");
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(18);
    content.set_margin_end(18);

    let heading = gtk::Box::new(Orientation::Horizontal, 8);
    let titles = gtk::Box::new(Orientation::Vertical, 1);
    titles.set_hexpand(true);
    let title = gtk::Label::new(Some("Quick Brie"));
    title.add_css_class("page-title");
    title.set_xalign(0.0);
    let privacy = gtk::Label::new(Some("Private · Ollama on this PC"));
    privacy.add_css_class("caption");
    privacy.add_css_class("dim-label");
    privacy.set_xalign(0.0);
    titles.append(&title);
    titles.append(&privacy);
    let open_membrie = gtk::Button::builder()
        .label("Open Membrie")
        .icon_name("go-next-symbolic")
        .build();
    open_membrie.add_css_class("flat");
    let close_button = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .tooltip_text("Close Quick Brie")
        .build();
    close_button.add_css_class("flat");
    heading.append(&titles);
    heading.append(&open_membrie);
    heading.append(&close_button);
    let window_handle = gtk::WindowHandle::new();
    window_handle.set_child(Some(&heading));
    content.append(&window_handle);

    let context = gtk::Label::new(Some(&quick_context_description(desktop_target.as_ref())));
    context.add_css_class("quick-context");
    context.set_xalign(0.0);
    context.set_wrap(true);
    content.append(&context);

    let messages = memory_list();
    append_brie_message(
        &messages,
        "Brie",
        "Ask me without leaving your work. I will still cite the Remembries behind my answer.",
        &[],
        None,
    );
    let conversation = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .min_content_height(220)
        .max_content_height(420)
        .child(&messages)
        .build();
    conversation.add_css_class("quick-conversation");
    content.append(&conversation);

    let status = gtk::Label::new(Some("Checking local intelligence…"));
    status.add_css_class("caption");
    status.add_css_class("dim-label");
    status.set_xalign(0.0);
    status.set_wrap(true);
    content.append(&status);

    let input_row = gtk::Box::new(Orientation::Horizontal, 8);
    let entry = gtk::Entry::builder()
        .placeholder_text("Ask Brie or write something to remember…")
        .hexpand(true)
        .build();
    let ask_button = gtk::Button::with_label("Ask Brie");
    ask_button.add_css_class("suggested-action");
    input_row.append(&entry);
    input_row.append(&ask_button);
    content.append(&input_row);

    let actions = gtk::Box::new(Orientation::Horizontal, 8);
    let remember_button = gtk::Button::with_label("Remember as a note");
    remember_button.add_css_class("flat");
    let hint = gtk::Label::new(Some("Esc closes Quick Brie"));
    hint.add_css_class("caption");
    hint.add_css_class("dim-label");
    hint.set_hexpand(true);
    hint.set_xalign(1.0);
    actions.append(&remember_button);
    actions.append(&hint);
    content.append(&actions);

    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Quick Brie")
        .default_width(680)
        .default_height(520)
        .resizable(true)
        .content(&content)
        .build();
    window.set_widget_name("membrie-quick-window");

    let window_for_close = window.clone();
    close_button.connect_clicked(move |_| window_for_close.close());

    let key_controller = gtk::EventControllerKey::new();
    let window_for_escape = window.clone();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            window_for_escape.close();
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    window.add_controller(key_controller);

    let application_for_open = application.clone();
    let window_for_open = window.clone();
    open_membrie.connect_clicked(move |_| {
        application_for_open.activate();
        window_for_open.close();
    });

    let ready = client
        .intelligence_status()
        .map(|intelligence| intelligence.ollama_available && intelligence.total_remembries > 0)
        .unwrap_or(false);
    ask_button.set_sensitive(ready);
    if ready {
        status.set_text("Brie is ready · answers and evidence stay on this PC");
    } else {
        status.set_text("Brie is not ready yet · open Membrie to check local intelligence");
    }

    let busy = Rc::new(Cell::new(false));
    let ask: Rc<dyn Fn()> = {
        let client = client.clone();
        let desktop_target = desktop_target.clone();
        let messages = messages.clone();
        let conversation = conversation.clone();
        let entry = entry.clone();
        let ask_button = ask_button.clone();
        let remember_button = remember_button.clone();
        let status = status.clone();
        let busy = Rc::clone(&busy);
        Rc::new(move || {
            let question = entry.text().trim().to_owned();
            if question.is_empty() || !ask_button.is_sensitive() || busy.replace(true) {
                return;
            }
            entry.set_text("");
            entry.set_sensitive(false);
            ask_button.set_sensitive(false);
            remember_button.set_sensitive(false);
            ask_button.set_label("Thinking…");
            status.set_text("Brie is checking your local Remembries…");
            append_brie_message(&messages, "You", &question, &[], None);
            scroll_list_to_bottom(&conversation);

            let question = quick_contextual_question(&question, desktop_target.as_ref());
            let client_for_worker = client.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(client_for_worker.ask_brie(question));
            });

            let messages = messages.clone();
            let conversation = conversation.clone();
            let entry = entry.clone();
            let ask_button = ask_button.clone();
            let remember_button = remember_button.clone();
            let status = status.clone();
            let busy = Rc::clone(&busy);
            gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
                match receiver.try_recv() {
                    Ok(Ok(answer)) => {
                        append_brie_message(
                            &messages,
                            "Brie",
                            &answer.answer,
                            &answer.citations,
                            None,
                        );
                        status.set_text("Answered locally · select a citation to inspect it");
                        finish_quick_brie_request(&entry, &ask_button, &remember_button, &busy);
                        scroll_list_to_bottom(&conversation);
                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Err(error)) => {
                        append_brie_message(
                            &messages,
                            "Brie",
                            &format!("I couldn't complete that locally: {error}"),
                            &[],
                            None,
                        );
                        status.set_text("The local request could not finish");
                        finish_quick_brie_request(&entry, &ask_button, &remember_button, &busy);
                        scroll_list_to_bottom(&conversation);
                        gtk::glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        status.set_text("Brie's local worker stopped unexpectedly");
                        finish_quick_brie_request(&entry, &ask_button, &remember_button, &busy);
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
        })
    };
    let ask_from_button = Rc::clone(&ask);
    ask_button.connect_clicked(move |_| ask_from_button());
    let ask_from_entry = Rc::clone(&ask);
    entry.connect_activate(move |_| ask_from_entry());

    let client_for_note = client.clone();
    let entry_for_note = entry.clone();
    let status_for_note = status.clone();
    let desktop_target_for_note = desktop_target.clone();
    remember_button.connect_clicked(move |_| {
        let note = entry_for_note.text().trim().to_owned();
        if note.is_empty() {
            status_for_note.set_text("Write something first, then remember it as a note");
            return;
        }
        let title = desktop_target_for_note
            .as_ref()
            .map(|target| format!("Quick note · {}", quick_app_name(target)))
            .unwrap_or_else(|| "Quick note".to_owned());
        match client_for_note.create(NewRemembrie::manual(&title, &note)) {
            Ok(_) => {
                entry_for_note.set_text("");
                status_for_note.set_text("Quick Remembrie saved locally");
            }
            Err(error) => status_for_note.set_text(&format!("Could not save locally: {error}")),
        }
    });

    window.present();
    entry.grab_focus();
}

fn finish_quick_brie_request(
    entry: &gtk::Entry,
    ask_button: &gtk::Button,
    remember_button: &gtk::Button,
    busy: &Cell<bool>,
) {
    busy.set(false);
    entry.set_sensitive(true);
    ask_button.set_sensitive(true);
    ask_button.set_label("Ask Brie");
    remember_button.set_sensitive(true);
    entry.grab_focus();
}

fn quick_app_name(target: &WindowTarget) -> &str {
    if target.app_name.trim().is_empty() {
        target.app_id.trim()
    } else {
        target.app_name.trim()
    }
}

fn quick_context_description(target: Option<&WindowTarget>) -> String {
    let Some(target) = target else {
        return "No previous window context was available; Brie will search your Remembries normally."
            .to_owned();
    };
    let app = quick_app_name(target);
    if target.window_title.trim().is_empty() {
        format!("Opened from {app} · only this application identity is added as search context")
    } else {
        format!(
            "Opened from {app} · {} · only this title is added as search context",
            target.window_title.trim()
        )
    }
}

fn quick_contextual_question(question: &str, target: Option<&WindowTarget>) -> String {
    let Some(target) = target else {
        return question.to_owned();
    };
    format!(
        "{question}\n\nCurrent desktop context, use only when relevant: application '{}', window title '{}'.",
        quick_app_name(target),
        target.window_title.trim()
    )
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
    let page = page_shell(
        "Timeline",
        "A chronological view of what Membrie observed, with exact evidence one step away.",
    );

    let day_navigation = gtk::Box::new(Orientation::Horizontal, 8);
    day_navigation.add_css_class("timeline-day-navigation");
    let previous = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text("Previous day")
        .build();
    let state_for_previous = Rc::clone(state);
    previous.connect_clicked(move |_| {
        state_for_previous.timeline_target_id.borrow_mut().take();
        let day = shift_timeline_period(
            state_for_previous.timeline_day_start_ms.get(),
            state_for_previous.timeline_map_mode.get(),
            -1,
        );
        state_for_previous.timeline_day_start_ms.set(day);
        *state_for_previous.timeline_render_key.borrow_mut() = None;
        refresh_timeline(&state_for_previous);
    });
    let state_for_next = Rc::clone(state);
    state.timeline_next_button.connect_clicked(move |_| {
        let selected = state_for_next.timeline_day_start_ms.get();
        let today = local_today_start_ms();
        if selected < today {
            state_for_next.timeline_target_id.borrow_mut().take();
            state_for_next.timeline_day_start_ms.set(
                shift_timeline_period(selected, state_for_next.timeline_map_mode.get(), 1)
                    .min(today),
            );
            *state_for_next.timeline_render_key.borrow_mut() = None;
            refresh_timeline(&state_for_next);
        }
    });
    let state_for_today = Rc::clone(state);
    state.timeline_today_button.connect_clicked(move |_| {
        state_for_today.timeline_target_id.borrow_mut().take();
        state_for_today
            .timeline_day_start_ms
            .set(local_today_start_ms());
        *state_for_today.timeline_render_key.borrow_mut() = None;
        refresh_timeline(&state_for_today);
    });
    day_navigation.append(&previous);
    day_navigation.append(&state.timeline_day_label);
    day_navigation.append(&state.timeline_today_button);
    day_navigation.append(&state.timeline_next_button);
    page.append(&day_navigation);

    let history_section = gtk::Box::new(Orientation::Vertical, 8);
    history_section.add_css_class("timeline-section");
    let history_top = gtk::Box::new(Orientation::Horizontal, 8);
    let history_heading = gtk::Label::new(Some("Activity map"));
    history_heading.add_css_class("heading");
    history_heading.set_xalign(0.0);
    history_heading.set_hexpand(true);
    let day_mode = gtk::ToggleButton::with_label("Day");
    let week_mode = gtk::ToggleButton::with_label("Week");
    let month_mode = gtk::ToggleButton::with_label("Month");
    week_mode.set_group(Some(&day_mode));
    month_mode.set_group(Some(&day_mode));
    week_mode.set_active(true);
    for (button, mode) in [
        (day_mode.clone(), TimelineMapMode::Day),
        (week_mode.clone(), TimelineMapMode::Week),
        (month_mode.clone(), TimelineMapMode::Month),
    ] {
        let state_for_mode = Rc::clone(state);
        button.connect_toggled(move |button| {
            if !button.is_active() || state_for_mode.timeline_map_mode.get() == mode {
                return;
            }
            state_for_mode.timeline_map_mode.set(mode);
            *state_for_mode.timeline_render_key.borrow_mut() = None;
            refresh_timeline(&state_for_mode);
        });
    }
    history_top.append(&history_heading);
    history_top.append(&day_mode);
    history_top.append(&week_mode);
    history_top.append(&month_mode);
    let history_detail = gtk::Label::new(Some(
        "Color identifies the dominant application; intensity means remembered active time—not productivity or importance.",
    ));
    history_detail.add_css_class("dim-label");
    history_detail.set_xalign(0.0);
    history_detail.set_wrap(true);
    history_section.append(&history_top);
    history_section.append(&history_detail);
    history_section.append(&state.timeline_history_grid);
    page.append(&history_section);

    let ribbon_section = gtk::Box::new(Orientation::Vertical, 7);
    ribbon_section.add_css_class("timeline-section");
    let ribbon_heading = gtk::Label::new(Some("Shape of the day"));
    ribbon_heading.add_css_class("heading");
    ribbon_heading.set_xalign(0.0);
    let ribbon_detail = gtk::Label::new(Some(
        "Application colors stay consistent. Durations are observed active time, with idle gaps left visible.",
    ));
    ribbon_detail.add_css_class("dim-label");
    ribbon_detail.set_xalign(0.0);
    ribbon_detail.set_wrap(true);
    ribbon_section.append(&ribbon_heading);
    ribbon_section.append(&ribbon_detail);
    ribbon_section.append(&state.timeline_ribbon_axis);
    ribbon_section.append(&state.timeline_ribbon_grid);
    ribbon_section.append(&state.timeline_app_legend);
    page.append(&ribbon_section);

    let insight_heading = gtk::Label::new(Some("Pattern worth noticing"));
    insight_heading.add_css_class("heading");
    insight_heading.set_xalign(0.0);
    state.timeline_insight.append(&insight_heading);
    state.timeline_insight.append(&state.timeline_insight_label);
    page.append(&state.timeline_insight);

    let capture = gtk::Box::new(Orientation::Vertical, 10);
    capture.add_css_class("manual-capture-content");

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

    let manual_capture = gtk::Expander::builder()
        .label("Create a Remembrie manually")
        .child(&capture)
        .build();
    manual_capture.add_css_class("card");
    manual_capture.add_css_class("manual-capture");
    page.append(&manual_capture);

    let recent = gtk::Label::new(Some("Chronology"));
    recent.add_css_class("heading");
    recent.set_xalign(0.0);
    page.append(&recent);
    page.append(&state.timeline);

    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&page)
        .build()
        .upcast()
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

fn build_brie_page(state: &Rc<UiState>, stack: &gtk::Stack) -> gtk::Widget {
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
        None,
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
        let stack = stack.clone();
        Rc::new(move || {
            let question = state.brie_entry.text().trim().to_owned();
            if question.is_empty() || state.brie_busy.replace(true) {
                return;
            }
            state.brie_entry.set_text("");
            state.brie_entry.set_sensitive(false);
            state.brie_send_button.set_sensitive(false);
            state.brie_send_button.set_label("Brie is thinking…");
            append_brie_message(&state.brie_messages, "You", &question, &[], None);
            scroll_list_to_bottom(&conversation);

            let client = state.client.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(client.ask_brie(question));
            });
            let state = Rc::clone(&state);
            let conversation = conversation.clone();
            let stack = stack.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
                match receiver.try_recv() {
                    Ok(Ok(answer)) => {
                        finish_brie_request(&state);
                        append_brie_answer(&state.brie_messages, &answer, &state, &stack);
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
                            None,
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
        "Off by default. When enabled, Membrie records focused application and window-title changes, then closes a session after inactivity. It never records keyboard or pointer input.",
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
        "Screen content remains off unless you separately enable Screen Memory below.",
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

    let semantic_card = gtk::Box::new(Orientation::Vertical, 8);
    semantic_card.add_css_class("card");
    semantic_card.add_css_class("capture-card");
    let semantic_heading = gtk::Label::new(Some("Semantic Context · Early access"));
    semantic_heading.add_css_class("heading");
    semantic_heading.set_xalign(0.0);
    let semantic_detail = gtk::Label::new(Some(
        "Off by default. When enabled, Membrie reads bounded names, labels, and visible text supplied by the active application to GNOME accessibility tools. Password fields are skipped, no application actions are performed, and exclusions, secret filtering, and duplicate filtering apply before anything is stored.",
    ));
    semantic_detail.add_css_class("dim-label");
    semantic_detail.set_xalign(0.0);
    semantic_detail.set_wrap(true);
    let semantic_interval_row = gtk::Box::new(Orientation::Horizontal, 10);
    let semantic_interval_label =
        gtk::Label::new(Some("Check changed application context at most every"));
    semantic_interval_label.set_xalign(0.0);
    semantic_interval_label.set_hexpand(true);
    semantic_interval_row.append(&semantic_interval_label);
    semantic_interval_row.append(&state.semantic_interval_combo);
    let semantic_fallback = gtk::Label::new(Some(
        "Rich semantic context avoids local vision work. Partial or unavailable context falls back to Screen Memory when Screen Memory is enabled.",
    ));
    semantic_fallback.add_css_class("dim-label");
    semantic_fallback.set_xalign(0.0);
    semantic_fallback.set_wrap(true);
    let semantic_test_heading = gtk::Label::new(Some("Compatibility test"));
    semantic_test_heading.add_css_class("heading");
    semantic_test_heading.set_xalign(0.0);
    semantic_test_heading.set_margin_top(8);
    let semantic_hint = gtk::Label::new(Some(
        "The five-second test stores nothing. It lets you check a window before enabling automatic capture; some applications may need to be restarted after GNOME accessibility is enabled.",
    ));
    semantic_hint.add_css_class("dim-label");
    semantic_hint.set_xalign(0.0);
    semantic_hint.set_wrap(true);
    semantic_card.append(&semantic_heading);
    semantic_card.append(&state.semantic_status);
    semantic_card.append(&semantic_detail);
    semantic_card.append(&semantic_interval_row);
    semantic_card.append(&state.semantic_capture_button);
    semantic_card.append(&semantic_fallback);
    semantic_card.append(&semantic_test_heading);
    semantic_card.append(&semantic_hint);
    semantic_card.append(&state.semantic_button);
    let state_for_semantic_capture = Rc::clone(state);
    state
        .semantic_capture_button
        .connect_clicked(move |button| {
            request_semantic_capture_toggle(button, &state_for_semantic_capture)
        });
    let state_for_semantic_interval = Rc::clone(state);
    state.semantic_interval_combo.connect_changed(move |combo| {
        if state_for_semantic_interval.semantic_settings_updating.get() {
            return;
        }
        let Some(value) = combo
            .active_id()
            .and_then(|value| value.parse::<u64>().ok())
        else {
            return;
        };
        match state_for_semantic_interval
            .client
            .set_semantic_sample_interval(value)
        {
            Ok(status) => {
                apply_status_ui(&state_for_semantic_interval, &status);
                toast(
                    &state_for_semantic_interval,
                    "Semantic Context timing updated",
                );
            }
            Err(error) => {
                toast(
                    &state_for_semantic_interval,
                    &format!("Could not update Semantic Context timing: {error}"),
                );
                refresh_status(&state_for_semantic_interval);
            }
        }
    });
    let state_for_semantic = Rc::clone(state);
    state.semantic_button.connect_clicked(move |button| {
        request_semantic_probe(button, &state_for_semantic);
    });
    page.append(&semantic_card);

    let screen_card = gtk::Box::new(Orientation::Vertical, 8);
    screen_card.add_css_class("card");
    screen_card.add_css_class("capture-card");
    let screen_heading = gtk::Label::new(Some("Screen Memory · Early access"));
    screen_heading.add_css_class("heading");
    screen_heading.set_xalign(0.0);
    let screen_detail = gtk::Label::new(Some(
        "Off by default. When enabled, Membrie privately samples only the active window and ignores unchanged frames. Each temporary PNG is removed from disk before its pixels are passed in memory to the selected local Ollama vision model; only clearly labeled machine-described text is kept. No video is recorded.",
    ));
    screen_detail.add_css_class("dim-label");
    screen_detail.set_xalign(0.0);
    screen_detail.set_wrap(true);
    let screen_caution = gtk::Label::new(Some(
        "Machine descriptions can be incomplete or wrong. Brie treats them as supporting context—not proof that an action was completed.",
    ));
    screen_caution.add_css_class("dim-label");
    screen_caution.set_xalign(0.0);
    screen_caution.set_wrap(true);
    let screen_interval_row = gtk::Box::new(Orientation::Horizontal, 10);
    let screen_interval_label = gtk::Label::new(Some("Check a changed screen at most every"));
    screen_interval_label.set_xalign(0.0);
    screen_interval_label.set_hexpand(true);
    screen_interval_row.append(&screen_interval_label);
    screen_interval_row.append(&state.screen_interval_combo);
    let screen_model_row = gtk::Box::new(Orientation::Horizontal, 10);
    let screen_model_label = gtk::Label::new(Some("Local screen model"));
    screen_model_label.set_xalign(0.0);
    screen_model_label.set_hexpand(true);
    screen_model_row.append(&screen_model_label);
    screen_model_row.append(&state.screen_model_combo);
    screen_card.append(&screen_heading);
    screen_card.append(&state.screen_status);
    screen_card.append(&screen_detail);
    screen_card.append(&screen_caution);
    screen_card.append(&screen_interval_row);
    screen_card.append(&screen_model_row);
    screen_card.append(&state.screen_button);
    screen_card.append(&state.screen_capture_now_button);

    let state_for_screen = Rc::clone(state);
    state.screen_button.connect_clicked(move |_| {
        let enabled = state_for_screen
            .client
            .status()
            .map(|status| !status.screen_enabled)
            .unwrap_or(false);
        match state_for_screen.client.set_screen_enabled(enabled) {
            Ok(status) => {
                apply_status_ui(&state_for_screen, &status);
                toast(
                    &state_for_screen,
                    if enabled {
                        "Screen Memory enabled—temporary images stay local and are never retained"
                    } else {
                        "Screen Memory disabled"
                    },
                );
            }
            Err(error) => toast(
                &state_for_screen,
                &format!("Could not update Screen Memory: {error}"),
            ),
        }
    });
    let state_for_screen_capture = Rc::clone(state);
    state
        .screen_capture_now_button
        .connect_clicked(move |_| begin_manual_screen_capture(&state_for_screen_capture));
    let state_for_screen_interval = Rc::clone(state);
    state.screen_interval_combo.connect_changed(move |combo| {
        if state_for_screen_interval.screen_settings_updating.get() {
            return;
        }
        let Some(value) = combo
            .active_id()
            .and_then(|value| value.parse::<u64>().ok())
        else {
            return;
        };
        match state_for_screen_interval
            .client
            .set_screen_sample_interval(value)
        {
            Ok(status) => {
                apply_status_ui(&state_for_screen_interval, &status);
                toast(&state_for_screen_interval, "Screen Memory timing updated");
            }
            Err(error) => {
                toast(
                    &state_for_screen_interval,
                    &format!("Could not update screen timing: {error}"),
                );
                refresh_status(&state_for_screen_interval);
            }
        }
    });
    let state_for_screen_model = Rc::clone(state);
    state.screen_model_combo.connect_changed(move |combo| {
        if state_for_screen_model.screen_settings_updating.get() {
            return;
        }
        let Some(model) = combo.active_id() else {
            return;
        };
        match state_for_screen_model
            .client
            .set_screen_model(model.as_str())
        {
            Ok(status) => {
                apply_status_ui(&state_for_screen_model, &status);
                toast(&state_for_screen_model, "Local screen model updated");
            }
            Err(error) => {
                toast(
                    &state_for_screen_model,
                    &format!("Could not update the local screen model: {error}"),
                );
                refresh_status(&state_for_screen_model);
            }
        }
    });
    page.append(&screen_card);

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
    let today = local_today_start_ms();
    let day_start = state.timeline_day_start_ms.get().min(today);
    state.timeline_day_start_ms.set(day_start);
    let day_end = add_local_days(day_start, 1);
    let map_mode = state.timeline_map_mode.get();
    let (map_start, map_end) = timeline_map_range(day_start, map_mode);
    let result = state
        .client
        .timeline_map(map_start, map_end)
        .and_then(|slices| {
            state
                .client
                .timeline_day(day_start, day_end)
                .map(|entries| (slices, entries))
        });
    match result {
        Ok((slices, entries)) => {
            let key = timeline_render_key(day_start, map_mode, &slices, &entries);
            if state.timeline_render_key.borrow().as_deref() == Some(&key) {
                return;
            }
            render_timeline_map(state, &slices, day_start, map_mode, map_start, map_end);
            render_timeline_ribbon(state, &entries, day_start, day_end);
            render_timeline_entries(&state.timeline, state, &entries);
            render_timeline_insight(state, &entries);
            state
                .timeline_day_label
                .set_text(&format_timeline_day(day_start, today));
            state.timeline_next_button.set_sensitive(day_start < today);
            state
                .timeline_today_button
                .set_sensitive(day_start != today);
            *state.timeline_render_key.borrow_mut() = Some(key);
        }
        Err(error) => {
            *state.timeline_render_key.borrow_mut() = None;
            render_error(
                &state.timeline,
                "Membrie daemon is not available",
                &error.to_string(),
            );
        }
    }
}

fn timeline_render_key(
    day_start: i64,
    map_mode: TimelineMapMode,
    slices: &[TimelineMapSlice],
    entries: &[TimelineEntry],
) -> String {
    let mut key = format!(
        "{day_start}:{map_mode:?}:{}:{}",
        slices.len(),
        entries.len()
    );
    for slice in slices {
        key.push_str(&format!(
            ":{}:{}:{}",
            slice.app_id, slice.started_at_ms, slice.ended_at_ms
        ));
    }
    for entry in entries {
        key.push_str(&format!(
            ":{}:{}:{}",
            entry.id, entry.started_at_ms, entry.ended_at_ms
        ));
        if let Some(activity) = &entry.activity {
            key.push_str(&format!(
                ":{}:{}:{}",
                activity.observation_count,
                activity.semantic_observation_count,
                activity.screen_observation_count
            ));
        }
    }
    key
}

fn render_timeline_map(
    state: &Rc<UiState>,
    slices: &[TimelineMapSlice],
    selected_day: i64,
    mode: TimelineMapMode,
    map_start: i64,
    map_end: i64,
) {
    clear_grid(&state.timeline_history_grid);
    match mode {
        TimelineMapMode::Day | TimelineMapMode::Week => render_hour_map(
            state,
            slices,
            selected_day,
            map_start,
            if mode == TimelineMapMode::Day { 1 } else { 7 },
        ),
        TimelineMapMode::Month => render_month_map(state, slices, selected_day, map_start, map_end),
    }
}

fn render_hour_map(
    state: &Rc<UiState>,
    slices: &[TimelineMapSlice],
    selected_day: i64,
    map_start: i64,
    day_count: i32,
) {
    for hour in 0..24_i32 {
        let text = match hour {
            0 => "12a".to_owned(),
            6 => "6a".to_owned(),
            12 => "12p".to_owned(),
            18 => "6p".to_owned(),
            _ => String::new(),
        };
        let label = gtk::Label::new(Some(&text));
        label.add_css_class("caption");
        label.add_css_class("dim-label");
        state
            .timeline_history_grid
            .attach(&label, hour + 1, 0, 1, 1);
    }

    for day_offset in 0..day_count {
        let day_start = add_local_days(map_start, day_offset);
        let day_label = gtk::Label::new(Some(&format_map_row_day(day_start)));
        day_label.add_css_class("caption");
        day_label.add_css_class("dim-label");
        day_label.set_xalign(0.0);
        state
            .timeline_history_grid
            .attach(&day_label, 0, day_offset + 1, 1, 1);

        for hour in 0..24_i32 {
            let cell_start = add_local_hours(day_start, hour);
            let cell_end = add_local_hours(day_start, hour + 1);
            let summary = map_cell_summary(slices, cell_start, cell_end);
            let button = timeline_map_button(
                state,
                summary.as_ref(),
                cell_start,
                cell_end,
                day_start,
                selected_day,
                false,
            );
            state
                .timeline_history_grid
                .attach(&button, hour + 1, day_offset + 1, 1, 1);
        }
    }
}

fn render_month_map(
    state: &Rc<UiState>,
    slices: &[TimelineMapSlice],
    selected_day: i64,
    map_start: i64,
    map_end: i64,
) {
    for (column, text) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        .iter()
        .enumerate()
    {
        let label = gtk::Label::new(Some(text));
        label.add_css_class("caption");
        label.add_css_class("dim-label");
        state
            .timeline_history_grid
            .attach(&label, column as i32, 0, 1, 1);
    }

    let first_weekday = local_day_of_week(map_start);
    let mut day_start = map_start;
    let mut offset = 0_i32;
    while day_start < map_end {
        let day_end = add_local_days(day_start, 1).min(map_end);
        let position = first_weekday - 1 + offset;
        let column = position % 7;
        let row = position / 7 + 1;
        let summary = map_cell_summary(slices, day_start, day_end);
        let button = timeline_map_button(
            state,
            summary.as_ref(),
            day_start,
            day_end,
            day_start,
            selected_day,
            true,
        );
        button.set_label(&local_day_number(day_start));
        state
            .timeline_history_grid
            .attach(&button, column, row, 1, 1);
        day_start = day_end;
        offset += 1;
    }
}

#[derive(Clone)]
struct MapCellSummary {
    app_id: String,
    app_name: String,
    active_ms: i64,
}

fn map_cell_summary(
    slices: &[TimelineMapSlice],
    cell_start: i64,
    cell_end: i64,
) -> Option<MapCellSummary> {
    let mut apps: HashMap<String, (String, String, i64)> = HashMap::new();
    let mut active_ms = 0_i64;
    for slice in slices {
        let duration = overlap_ms(slice.started_at_ms, slice.ended_at_ms, cell_start, cell_end);
        if duration <= 0 {
            continue;
        }
        active_ms += duration;
        let identity = app_identity(&slice.app_id, &slice.app_name);
        let total = apps.entry(identity).or_insert_with(|| {
            (
                slice.app_id.clone(),
                display_map_app_name(&slice.app_id, &slice.app_name),
                0,
            )
        });
        total.2 += duration;
    }
    apps.into_values()
        .max_by_key(|(_, _, duration)| *duration)
        .map(|(app_id, app_name, _)| MapCellSummary {
            app_id,
            app_name,
            active_ms,
        })
}

fn timeline_map_button(
    state: &Rc<UiState>,
    summary: Option<&MapCellSummary>,
    cell_start: i64,
    cell_end: i64,
    day_start: i64,
    selected_day: i64,
    is_day_cell: bool,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class(if is_day_cell {
        "timeline-map-day-cell"
    } else {
        "timeline-map-hour-cell"
    });
    if day_start == selected_day {
        button.add_css_class("timeline-map-selected");
    }
    if let Some(summary) = summary {
        let cell_duration = cell_end.saturating_sub(cell_start);
        let level = map_intensity_level(summary.active_ms, cell_duration);
        let color = app_color_index(&summary.app_id, &summary.app_name);
        button.add_css_class(&format!("app-map-{color}"));
        button.add_css_class(&format!("timeline-map-level-{level}"));
        button.set_tooltip_text(Some(&format!(
            "{} · {} dominant · {} observed active time",
            format_map_cell_time(cell_start, is_day_cell),
            summary.app_name,
            format_duration(summary.active_ms)
        )));
    } else {
        button.add_css_class("timeline-map-empty");
        button.set_tooltip_text(Some(&format!(
            "{} · no remembered activity",
            format_map_cell_time(cell_start, is_day_cell)
        )));
    }
    let state_for_day = Rc::clone(state);
    button.connect_clicked(move |_| {
        state_for_day.timeline_target_id.borrow_mut().take();
        state_for_day.timeline_day_start_ms.set(day_start);
        *state_for_day.timeline_render_key.borrow_mut() = None;
        refresh_timeline(&state_for_day);
    });
    button
}

fn map_intensity_level(active_ms: i64, cell_duration_ms: i64) -> u8 {
    let share = active_ms.saturating_mul(100) / cell_duration_ms.max(1);
    match share {
        0 => 0,
        1..=24 => 1,
        25..=49 => 2,
        50..=74 => 3,
        _ => 4,
    }
}

#[derive(Clone)]
struct TimelineSlice {
    started_at_ms: i64,
    ended_at_ms: i64,
    app_id: String,
    app_name: String,
    window_title: String,
}

fn timeline_slices(
    entries: &[TimelineEntry],
    range_start: i64,
    range_end: i64,
) -> Vec<TimelineSlice> {
    let mut slices: Vec<TimelineSlice> = Vec::new();
    for entry in entries {
        let Some(activity) = &entry.activity else {
            continue;
        };
        for (index, observation) in activity.observations.iter().enumerate() {
            let next_at = activity
                .observations
                .get(index + 1)
                .map(|next| next.observed_at_ms)
                .unwrap_or(entry.ended_at_ms);
            let started_at_ms = observation
                .observed_at_ms
                .max(entry.started_at_ms)
                .max(range_start);
            let ended_at_ms = next_at.min(entry.ended_at_ms).min(range_end);
            if ended_at_ms <= started_at_ms {
                continue;
            }
            let app_id = observation.app_id.trim().to_owned();
            let app_name = if observation.app_name.trim().is_empty() {
                if app_id.is_empty() {
                    "Desktop".to_owned()
                } else {
                    app_id.clone()
                }
            } else {
                observation.app_name.trim().to_owned()
            };
            if let Some(previous) = slices.last_mut()
                && app_identity(&previous.app_id, &previous.app_name)
                    == app_identity(&app_id, &app_name)
                && previous.ended_at_ms >= started_at_ms
            {
                previous.ended_at_ms = previous.ended_at_ms.max(ended_at_ms);
                previous.window_title = observation.window_title.clone();
                continue;
            }
            slices.push(TimelineSlice {
                started_at_ms,
                ended_at_ms,
                app_id,
                app_name,
                window_title: observation.window_title.clone(),
            });
        }
    }
    slices.sort_by_key(|slice| slice.started_at_ms);
    slices
}

fn render_timeline_ribbon(
    state: &UiState,
    entries: &[TimelineEntry],
    day_start: i64,
    day_end: i64,
) {
    clear_grid(&state.timeline_ribbon_grid);
    clear_box(&state.timeline_ribbon_axis);
    clear_flow_box(&state.timeline_app_legend);

    for label in ["12 AM", "6 AM", "Noon", "6 PM", "12 AM"] {
        let label = gtk::Label::new(Some(label));
        label.add_css_class("caption");
        label.add_css_class("dim-label");
        state.timeline_ribbon_axis.append(&label);
    }

    let track = gtk::Box::new(Orientation::Horizontal, 0);
    track.add_css_class("timeline-ribbon-track");
    track.set_size_request(-1, 42);
    state.timeline_ribbon_grid.attach(&track, 0, 0, 96, 1);

    let slices = timeline_slices(entries, day_start, day_end);
    let mut app_totals: HashMap<String, (String, i64)> = HashMap::new();
    for slice in &slices {
        let day_duration = day_end - day_start;
        let start = (((slice.started_at_ms - day_start) * 96) / day_duration).clamp(0, 95);
        let end = ((((slice.ended_at_ms - day_start) * 96) + day_duration - 1) / day_duration)
            .clamp(start + 1, 96);
        let span = (end - start).max(1);
        let color = app_color_index(&slice.app_id, &slice.app_name);
        let segment = gtk::Box::new(Orientation::Horizontal, 0);
        segment.add_css_class("timeline-ribbon-segment");
        segment.add_css_class(&format!("app-fill-{color}"));
        segment.set_tooltip_text(Some(&format!(
            "{} · {}{}",
            slice.app_name,
            format_duration(slice.ended_at_ms - slice.started_at_ms),
            if slice.window_title.trim().is_empty() {
                String::new()
            } else {
                format!(" · {}", slice.window_title.trim())
            }
        )));
        if span >= 10 {
            let label = gtk::Label::new(Some(&slice.app_name));
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_margin_start(5);
            label.set_margin_end(5);
            segment.append(&label);
        }
        state
            .timeline_ribbon_grid
            .attach(&segment, start as i32, 0, span as i32, 1);
        let identity = app_identity(&slice.app_id, &slice.app_name);
        let total = app_totals
            .entry(identity)
            .or_insert_with(|| (slice.app_name.clone(), 0));
        total.1 += slice.ended_at_ms - slice.started_at_ms;
    }

    if slices.is_empty() {
        let empty = gtk::Label::new(Some("No completed activity sessions for this day"));
        empty.add_css_class("dim-label");
        empty.set_margin_top(10);
        empty.set_margin_bottom(10);
        state.timeline_ribbon_grid.attach(&empty, 0, 0, 96, 1);
    }

    let mut totals: Vec<(String, String, i64)> = app_totals
        .into_iter()
        .map(|(identity, (name, duration))| (identity, name, duration))
        .collect();
    totals.sort_by_key(|item| std::cmp::Reverse(item.2));
    for (identity, name, duration) in totals.into_iter().take(12) {
        let item = gtk::Box::new(Orientation::Horizontal, 6);
        item.add_css_class("timeline-legend-item");
        let dot = gtk::Box::new(Orientation::Horizontal, 0);
        dot.add_css_class("timeline-app-dot");
        dot.add_css_class(&format!("app-solid-{}", app_color_index(&identity, &name)));
        let label = gtk::Label::new(Some(&format!("{name} · {}", format_duration(duration))));
        label.add_css_class("caption");
        item.append(&dot);
        item.append(&label);
        state.timeline_app_legend.insert(&item, -1);
    }
}

fn render_timeline_insight(state: &UiState, entries: &[TimelineEntry]) {
    let mut returns: HashMap<(String, String), (String, String, u32)> = HashMap::new();
    for entry in entries {
        let Some(activity) = &entry.activity else {
            continue;
        };
        for observation in &activity.observations {
            let title = observation.window_title.trim();
            if title.is_empty() {
                continue;
            }
            let app = if observation.app_name.trim().is_empty() {
                observation.app_id.trim()
            } else {
                observation.app_name.trim()
            };
            let key = (app.to_ascii_lowercase(), title.to_ascii_lowercase());
            let value = returns
                .entry(key)
                .or_insert_with(|| (app.to_owned(), title.to_owned(), 0));
            value.2 += 1;
        }
    }
    let insight = returns
        .into_iter()
        .filter(|(_, (_, _, count))| *count >= 3)
        .max_by_key(|(_, (_, _, count))| *count)
        .map(|(_, (app, title, count))| {
            format!(
                "You returned to “{}” {count} times in {app}. This is evidence of repeated context—not proof of an unfinished task. Brie can use the supporting Remembries if you ask.",
                truncate_display_text(&title, 120)
            )
        });
    if let Some(insight) = insight {
        state.timeline_insight_label.set_text(&insight);
        state.timeline_insight.set_visible(true);
    } else {
        state.timeline_insight.set_visible(false);
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

fn begin_manual_screen_capture(state: &Rc<UiState>) {
    if state.screen_capture_now_busy.replace(true) {
        return;
    }
    state.screen_capture_now_button.set_sensitive(false);
    state
        .screen_capture_now_button
        .set_label("Switch to the window… 5");
    let remaining = Rc::new(Cell::new(5_u8));
    let state_for_countdown = Rc::clone(state);
    gtk::glib::timeout_add_seconds_local(1, move || {
        let next = remaining.get().saturating_sub(1);
        remaining.set(next);
        if next > 0 {
            state_for_countdown
                .screen_capture_now_button
                .set_label(&format!("Switch to the window… {next}"));
            return gtk::glib::ControlFlow::Continue;
        }

        state_for_countdown
            .screen_capture_now_button
            .set_label("Remembering locally…");
        let client = state_for_countdown.client.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(remember_visible_window(&client));
        });
        let state_for_result = Rc::clone(&state_for_countdown);
        gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
            match receiver.try_recv() {
                Ok(Ok(result)) => {
                    finish_manual_screen_capture(&state_for_result);
                    if result.outcome == "stored" {
                        toast(
                            &state_for_result,
                            "Screen context remembered—it will join the current activity session",
                        );
                    } else {
                        toast(
                            &state_for_result,
                            result.reason.as_deref().unwrap_or(
                                "That screen did not contain useful context to remember",
                            ),
                        );
                    }
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    finish_manual_screen_capture(&state_for_result);
                    toast(
                        &state_for_result,
                        &format!("Could not remember that screen: {error}"),
                    );
                    gtk::glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finish_manual_screen_capture(&state_for_result);
                    toast(
                        &state_for_result,
                        "The local screen-memory worker stopped unexpectedly",
                    );
                    gtk::glib::ControlFlow::Break
                }
            }
        });
        gtk::glib::ControlFlow::Break
    });
}

fn finish_manual_screen_capture(state: &Rc<UiState>) {
    state.screen_capture_now_busy.set(false);
    state
        .screen_capture_now_button
        .set_label("Remember this screen in 5 seconds");
    refresh_status(state);
}

fn remember_visible_window(client: &DaemonClient) -> Result<ScreenCaptureResult, String> {
    let proxy = gtk::gio::DBusProxy::for_bus_sync(
        gtk::gio::BusType::Session,
        gtk::gio::DBusProxyFlags::DO_NOT_AUTO_START,
        None,
        CLIPBOARD_DBUS_NAME,
        CLIPBOARD_DBUS_PATH,
        CLIPBOARD_DBUS_INTERFACE,
        None::<&gtk::gio::Cancellable>,
    )
    .map_err(|error| format!("the GNOME Desktop Bridge is unavailable: {error}"))?;
    if proxy.name_owner().is_none() {
        return Err("the GNOME Desktop Bridge is not connected".to_owned());
    }
    let state = proxy
        .call_sync(
            "GetActivityState",
            None,
            gtk::gio::DBusCallFlags::NONE,
            1_000,
            None::<&gtk::gio::Cancellable>,
        )
        .map_err(|error| format!("desktop context could not be read: {error}"))?
        .try_get::<(String, String, String, u32, bool)>()
        .map_err(|error| format!("desktop context was not understood: {error}"))?;
    let activity = client
        .record_activity(ActivitySnapshot {
            app_id: state.0,
            app_name: state.1,
            window_title: state.2,
            idle_ms: u64::from(state.3),
            locked: state.4,
            occurred_at_ms: None,
        })
        .map_err(|error| error.to_string())?;
    if !activity.screen_capture_allowed {
        return Err(activity.reason.unwrap_or_else(|| {
            "Screen Memory is paused, disabled, or excluded for that window".to_owned()
        }));
    }
    let session_id = activity.session_id.ok_or_else(|| {
        "no activity session was available; switch away from Membrie before the countdown ends"
            .to_owned()
    })?;
    let capture = proxy
        .call_sync(
            "CaptureWindow",
            None,
            gtk::gio::DBusCallFlags::NONE,
            10_000,
            None::<&gtk::gio::Cancellable>,
        )
        .map_err(|error| format!("the active window could not be captured: {error}"))?
        .try_get::<(String, String, String, String, u32, u32)>()
        .map_err(|error| format!("the active-window capture was not understood: {error}"))?;
    let path = capture.0.clone();
    let result = client
        .analyze_screen(ScreenCaptureCandidate {
            session_id,
            screenshot_path: capture.0,
            app_id: capture.1,
            app_name: capture.2,
            window_title: capture.3,
            observed_at_ms: Some(current_time_ms()),
            width: capture.4,
            height: capture.5,
        })
        .map_err(|error| error.to_string());
    let _ = fs::remove_file(path);
    result
}

fn refresh_semantic_status(state: &Rc<UiState>) {
    if state.semantic_probe_busy.get() {
        return;
    }
    let settings = gtk::gio::Settings::new("org.gnome.desktop.interface");
    if settings.boolean("toolkit-accessibility") {
        state
            .semantic_button
            .set_label("Test a window in 5 seconds");
    } else {
        state
            .semantic_button
            .set_label("Enable and test semantic context");
    }
    state.semantic_button.set_sensitive(true);
}

fn request_semantic_capture_toggle(parent: &gtk::Button, state: &Rc<UiState>) {
    let status = match state.client.status() {
        Ok(status) => status,
        Err(error) => {
            toast(state, &format!("Could not read capture settings: {error}"));
            return;
        }
    };
    if status.semantic_enabled {
        set_semantic_capture_enabled(state, false);
        return;
    }
    if !status.activity_enabled {
        toast(state, "Enable Activity Context before Semantic Context");
        return;
    }

    let settings = gtk::gio::Settings::new("org.gnome.desktop.interface");
    if settings.boolean("toolkit-accessibility") {
        set_semantic_capture_enabled(state, true);
        return;
    }
    let dialog = adw::AlertDialog::builder()
        .heading("Enable GNOME accessibility and Semantic Context?")
        .body(
            "This allows Membrie—and other local accessibility tools—to receive UI structure that applications provide to GNOME. Membrie reads only the focused matching window, skips password fields, performs no actions, and applies its local privacy rules before storage.",
        )
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("enable", "Enable Semantic Context");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("enable"));
    dialog.set_response_appearance("enable", adw::ResponseAppearance::Suggested);
    let state_for_enable = Rc::clone(state);
    dialog.connect_response(Some("enable"), move |_, _| {
        if settings.set_boolean("toolkit-accessibility", true).is_err() {
            toast(
                &state_for_enable,
                "GNOME accessibility could not be enabled",
            );
            return;
        }
        set_semantic_capture_enabled(&state_for_enable, true);
    });
    dialog.present(Some(parent));
}

fn set_semantic_capture_enabled(state: &Rc<UiState>, enabled: bool) {
    match state.client.set_semantic_enabled(enabled) {
        Ok(status) => {
            apply_status_ui(state, &status);
            toast(
                state,
                if enabled {
                    "Semantic Context enabled—application-provided evidence stays local"
                } else {
                    "Semantic Context disabled"
                },
            );
        }
        Err(error) => toast(
            state,
            &format!("Could not update Semantic Context: {error}"),
        ),
    }
}

fn request_semantic_probe(parent: &gtk::Button, state: &Rc<UiState>) {
    if state.semantic_probe_busy.get() {
        return;
    }
    let settings = gtk::gio::Settings::new("org.gnome.desktop.interface");
    if settings.boolean("toolkit-accessibility") {
        start_semantic_probe_countdown(parent, state);
        return;
    }

    let dialog = adw::AlertDialog::builder()
        .heading("Enable GNOME accessibility?")
        .body(
            "This allows Membrie—and other local accessibility tools—to receive UI structure that applications provide to GNOME. Membrie’s compatibility preview is read-only, performs no actions, skips password fields, and stores nothing. Some open applications may need to be restarted before they provide useful context.",
        )
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("enable", "Enable and test");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("enable"));
    dialog.set_response_appearance("enable", adw::ResponseAppearance::Suggested);
    let parent_for_response = parent.clone();
    let state_for_response = Rc::clone(state);
    dialog.connect_response(Some("enable"), move |_, _| {
        if settings.set_boolean("toolkit-accessibility", true).is_err() {
            toast(
                &state_for_response,
                "GNOME accessibility could not be enabled",
            );
            refresh_semantic_status(&state_for_response);
            return;
        }
        refresh_semantic_status(&state_for_response);
        start_semantic_probe_countdown(&parent_for_response, &state_for_response);
    });
    dialog.present(Some(parent));
}

fn start_semantic_probe_countdown(parent: &gtk::Button, state: &Rc<UiState>) {
    if state.semantic_probe_busy.replace(true) {
        return;
    }
    state.semantic_button.set_sensitive(false);
    state
        .semantic_button
        .set_label("Switch to the window to test · 5");
    state
        .semantic_status
        .set_text("Waiting for you to switch windows · nothing is being read during the countdown");

    let remaining = Rc::new(Cell::new(5_u8));
    let remaining_for_timer = Rc::clone(&remaining);
    let parent_for_timer = parent.clone();
    let state_for_timer = Rc::clone(state);
    gtk::glib::timeout_add_seconds_local(1, move || {
        let next = remaining_for_timer.get().saturating_sub(1);
        remaining_for_timer.set(next);
        if next > 0 {
            state_for_timer
                .semantic_button
                .set_label(&format!("Switch to the window to test · {next}"));
            return gtk::glib::ControlFlow::Continue;
        }
        state_for_timer
            .semantic_button
            .set_label("Reading the active window…");
        state_for_timer
            .semantic_status
            .set_text("Read-only compatibility test in progress · nothing will be stored");
        run_semantic_probe(&parent_for_timer, &state_for_timer);
        gtk::glib::ControlFlow::Break
    });
}

fn run_semantic_probe(parent: &gtk::Button, state: &Rc<UiState>) {
    let target = match focused_semantic_target() {
        Ok(target) => target,
        Err(error) => {
            finish_semantic_probe(state);
            let dialog = adw::AlertDialog::builder()
                .heading("The focused window could not be identified")
                .body(format!(
                    "Membrie stopped before inspecting accessibility content and stored nothing.\n\nDetails: {}",
                    truncate_display_text(&error, 800)
                ))
                .build();
            dialog.add_response("close", "Close");
            dialog.set_default_response(Some("close"));
            dialog.present(Some(parent));
            return;
        }
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(membrie_a11y::inspect_target_window(target, true));
    });
    let started_at = Instant::now();
    let parent_for_result = parent.clone();
    let state_for_result = Rc::clone(state);
    gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
        match receiver.try_recv() {
            Ok(Ok(summary)) => {
                finish_semantic_probe(&state_for_result);
                show_semantic_probe_result(&parent_for_result, &summary);
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(error)) => {
                finish_semantic_probe(&state_for_result);
                let dialog = adw::AlertDialog::builder()
                    .heading("This window did not provide semantic context")
                    .body(format!(
                        "Membrie stored nothing and made no changes. The application may need to be restarted after accessibility was enabled, or it may not expose an accessible active window.\n\nDetails: {}",
                        truncate_display_text(&error.to_string(), 800)
                    ))
                    .build();
                dialog.add_response("close", "Close");
                dialog.set_default_response(Some("close"));
                dialog.present(Some(&parent_for_result));
                gtk::glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty)
                if started_at.elapsed() < Duration::from_secs(20) =>
            {
                gtk::glib::ControlFlow::Continue
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                finish_semantic_probe(&state_for_result);
                let dialog = adw::AlertDialog::builder()
                    .heading("The compatibility test took too long")
                    .body("Membrie stopped waiting after 20 seconds and stored nothing. Try a simpler window, or restart the application you want to test after GNOME accessibility has been enabled.")
                    .build();
                dialog.add_response("close", "Close");
                dialog.set_default_response(Some("close"));
                dialog.present(Some(&parent_for_result));
                gtk::glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                finish_semantic_probe(&state_for_result);
                toast(
                    &state_for_result,
                    "The semantic compatibility test stopped unexpectedly",
                );
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn focused_semantic_target() -> Result<WindowTarget, String> {
    let proxy = gtk::gio::DBusProxy::for_bus_sync(
        gtk::gio::BusType::Session,
        gtk::gio::DBusProxyFlags::DO_NOT_AUTO_START,
        None,
        CLIPBOARD_DBUS_NAME,
        CLIPBOARD_DBUS_PATH,
        CLIPBOARD_DBUS_INTERFACE,
        None::<&gtk::gio::Cancellable>,
    )
    .map_err(|error| format!("the GNOME Desktop Bridge is unavailable: {error}"))?;
    if proxy.name_owner().is_none() {
        return Err("the GNOME Desktop Bridge is not connected".to_owned());
    }
    let (app_id, app_name, window_title, _idle_ms, locked) = proxy
        .call_sync(
            "GetActivityState",
            None,
            gtk::gio::DBusCallFlags::NONE,
            1_000,
            None::<&gtk::gio::Cancellable>,
        )
        .map_err(|error| format!("the focused window could not be read: {error}"))?
        .try_get::<(String, String, String, u32, bool)>()
        .map_err(|error| format!("the focused-window identity was not understood: {error}"))?;
    if locked {
        return Err("the screen is locked".to_owned());
    }
    if app_id.trim().is_empty() && app_name.trim().is_empty() && window_title.trim().is_empty() {
        return Err("GNOME did not report a focused window".to_owned());
    }
    Ok(WindowTarget {
        app_id,
        app_name,
        window_title,
    })
}

fn finish_semantic_probe(state: &Rc<UiState>) {
    state.semantic_probe_busy.set(false);
    refresh_semantic_status(state);
}

fn show_semantic_probe_result(parent: &gtk::Button, summary: &ProbeSummary) {
    let application = display_or_unknown(&summary.application);
    let window = display_or_unknown(&summary.window);
    let quality = match summary.quality() {
        "rich" => "Rich semantic context",
        "partial" => "Partial semantic context",
        _ => "Little semantic context",
    };
    let body = format!(
        "{quality}\n\nApplication: {application}\nWindow: {window}\nCoverage: {} visible items · {} text-capable · {} document-capable\n\nNothing from this preview was stored. Automatic Semantic Context remains off.",
        summary.visible_nodes, summary.text_nodes, summary.document_nodes
    );
    let preview = if summary.preview.is_empty() {
        "No visible semantic labels were provided by this window.".to_owned()
    } else {
        summary.preview.join("\n")
    };
    let preview_label = gtk::Label::new(Some(&truncate_display_text(&preview, 6000)));
    preview_label.set_xalign(0.0);
    preview_label.set_yalign(0.0);
    preview_label.set_wrap(true);
    preview_label.set_selectable(true);
    preview_label.add_css_class("monospace");
    let preview_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(180)
        .max_content_height(360)
        .child(&preview_label)
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading("Semantic Context compatibility result")
        .body(body)
        .extra_child(&preview_scroll)
        .build();
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.present(Some(parent));
}

fn display_or_unknown(value: &str) -> &str {
    if value.trim().is_empty() {
        "Unknown"
    } else {
        value
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
            state.semantic_capture_button.set_sensitive(false);
            state.semantic_interval_combo.set_sensitive(false);
            state.screen_button.set_sensitive(false);
            state.screen_capture_now_button.set_sensitive(false);
            state.screen_interval_combo.set_sensitive(false);
            state.screen_model_combo.set_sensitive(false);
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
            state
                .semantic_status
                .set_text("Semantic Context · Daemon offline");
            state
                .screen_status
                .set_text("Screen Memory · Daemon offline");
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
    let current_screen_model = state
        .client
        .status()
        .map(|capture| capture.screen_model)
        .unwrap_or_else(|_| "gemma4:e2b".to_owned());
    state.screen_settings_updating.set(true);
    state.screen_model_combo.remove_all();
    for model in status
        .available_models
        .iter()
        .filter(|model| is_likely_vision_model(&model.name))
    {
        state
            .screen_model_combo
            .append(Some(&model.name), &model.name);
    }
    if !state
        .screen_model_combo
        .set_active_id(Some(&current_screen_model))
    {
        state
            .screen_model_combo
            .append(Some(&current_screen_model), &current_screen_model);
        state
            .screen_model_combo
            .set_active_id(Some(&current_screen_model));
    }
    state.screen_settings_updating.set(false);
}

fn is_likely_vision_model(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("minicpm-v")
        || name.contains("llava")
        || name.contains("vision")
        || name.starts_with("gemma4")
}

fn finish_brie_request(state: &UiState) {
    state.brie_busy.set(false);
    state.brie_entry.set_sensitive(true);
    state.brie_send_button.set_sensitive(true);
    state.brie_send_button.set_label("Ask Brie");
    state.brie_entry.grab_focus();
}

fn append_brie_answer(
    list: &gtk::ListBox,
    answer: &BrieAnswer,
    state: &Rc<UiState>,
    stack: &gtk::Stack,
) {
    append_brie_message(
        list,
        "Brie",
        &answer.answer,
        &answer.citations,
        Some((Rc::clone(state), stack.clone())),
    );
}

fn append_brie_message(
    list: &gtk::ListBox,
    speaker: &str,
    message: &str,
    citations: &[BrieCitation],
    navigation: Option<(Rc<UiState>, gtk::Stack)>,
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
            button.add_css_class("citation-button");
            button.set_halign(Align::Start);
            button.set_tooltip_text(Some(
                "Open this Remembrie in the Timeline with exact evidence",
            ));
            let citation = citation.clone();
            let navigation = navigation.clone();
            button.connect_clicked(move |button| {
                if let Some((state, stack)) = &navigation {
                    state
                        .timeline_day_start_ms
                        .set(local_day_start_ms(citation.remembrie.occurred_at_ms));
                    *state.timeline_target_id.borrow_mut() = Some(citation.remembrie.id.clone());
                    *state.timeline_render_key.borrow_mut() = None;
                    stack.set_visible_child_name("timeline");
                    refresh_timeline(state);
                    let state_for_evidence = Rc::clone(state);
                    let remembrie_id = citation.remembrie.id.clone();
                    gtk::glib::idle_add_local_once(move || {
                        show_remembrance_evidence(
                            &state_for_evidence.timeline,
                            &state_for_evidence.client,
                            &remembrie_id,
                        );
                    });
                    toast(
                        state,
                        &format!("Opened citation [{}] in the Timeline", citation.number),
                    );
                } else {
                    show_citation_preview(button, &citation);
                }
            });
            content.append(&button);
        }
    }
    row.set_child(Some(&content));
    list.append(&row);
}

fn show_citation_preview(parent: &gtk::Button, citation: &BrieCitation) {
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
    dialog.present(Some(parent));
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
    state.semantic_capture_button.set_sensitive(
        status.activity_enabled
            && (state.clipboard_bridge_available.get() || status.semantic_enabled),
    );
    state
        .semantic_interval_combo
        .set_sensitive(status.semantic_enabled);
    state.screen_button.set_sensitive(
        (state.clipboard_bridge_available.get() || status.screen_enabled)
            && status.activity_enabled,
    );
    state.screen_capture_now_button.set_sensitive(
        state.clipboard_bridge_available.get()
            && status.activity_enabled
            && status.screen_enabled
            && !status.paused
            && !state.screen_capture_now_busy.get(),
    );
    if !state.screen_capture_now_busy.get() {
        state
            .screen_capture_now_button
            .set_label("Remember this screen in 5 seconds");
    }
    state
        .screen_interval_combo
        .set_sensitive(status.screen_enabled);
    state
        .screen_model_combo
        .set_sensitive(!status.screen_enabled);
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
    state
        .semantic_capture_button
        .set_label(if status.semantic_enabled {
            "Disable automatic semantic context"
        } else {
            "Enable automatic semantic context"
        });
    state.screen_button.set_label(if status.screen_enabled {
        "Disable screen memory"
    } else {
        "Enable screen memory"
    });
    state.activity_settings_updating.set(true);
    state
        .activity_idle_combo
        .set_active_id(Some(&status.activity_idle_threshold_ms.to_string()));
    state.activity_settings_updating.set(false);
    state.semantic_settings_updating.set(true);
    state
        .semantic_interval_combo
        .set_active_id(Some(&status.semantic_sample_interval_ms.to_string()));
    state.semantic_settings_updating.set(false);
    state.screen_settings_updating.set(true);
    state
        .screen_interval_combo
        .set_active_id(Some(&status.screen_sample_interval_ms.to_string()));
    if !state
        .screen_model_combo
        .set_active_id(Some(&status.screen_model))
    {
        state
            .screen_model_combo
            .append(Some(&status.screen_model), &status.screen_model);
        state
            .screen_model_combo
            .set_active_id(Some(&status.screen_model));
    }
    state.screen_settings_updating.set(false);
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
    let semantic_text = if !status.semantic_enabled {
        format!(
            "Off · {} application-provided observations stored",
            status.semantic_observation_count
        )
    } else if !gtk::gio::Settings::new("org.gnome.desktop.interface")
        .boolean("toolkit-accessibility")
    {
        "Enabled, but GNOME accessibility is off · no semantic content is being read".to_owned()
    } else if !capture_agent_running {
        "Enabled, but the desktop capture service is not responding".to_owned()
    } else if !status.activity_enabled {
        "Waiting for Activity Context to be enabled".to_owned()
    } else {
        let seconds = status.semantic_sample_interval_ms / 1000;
        let interval = if seconds == 60 {
            "1 minute".to_owned()
        } else if seconds.is_multiple_of(60) {
            format!("{} minutes", seconds / 60)
        } else {
            format!("{seconds} seconds")
        };
        let last_remembered = match (
            status.semantic_last_observed_at_ms,
            status.semantic_last_app.as_deref(),
        ) {
            (Some(timestamp), Some(app)) if !app.trim().is_empty() => {
                format!("\nLast remembered: {} · {app}", format_timestamp(timestamp))
            }
            (Some(timestamp), _) => format!("\nLast remembered: {}", format_timestamp(timestamp)),
            _ => String::new(),
        };
        format!(
            "On · focused matching window only · up to once per {interval}\n{} observations stored{last_remembered}",
            status.semantic_observation_count
        )
    };
    if !state.semantic_probe_busy.get() {
        state.semantic_status.set_text(&semantic_text);
    }
    let screen_text = if !status.screen_enabled {
        format!(
            "Off · {} local screen observations stored · no screenshots retained",
            status.screen_observation_count
        )
    } else if !capture_agent_running {
        "Enabled, but the desktop capture service is not responding".to_owned()
    } else if !status.activity_enabled {
        "Waiting for Activity Context to be enabled".to_owned()
    } else {
        let seconds = status.screen_sample_interval_ms / 1000;
        let interval = if seconds == 60 {
            "1 minute".to_owned()
        } else if seconds.is_multiple_of(60) {
            format!("{} minutes", seconds / 60)
        } else {
            format!("{seconds} seconds")
        };
        let failures = if status.screen_failed_count > 0 {
            format!(
                " · {} local analyses need attention",
                status.screen_failed_count
            )
        } else {
            String::new()
        };
        let processing = if status.screen_processing_count > 0 {
            format!(
                " · {} local analysis in progress",
                status.screen_processing_count
            )
        } else {
            String::new()
        };
        let last_remembered = match (
            status.screen_last_observed_at_ms,
            status.screen_last_app.as_deref(),
        ) {
            (Some(timestamp), Some(app)) if !app.trim().is_empty() => {
                format!("\nLast remembered: {} · {app}", format_timestamp(timestamp))
            }
            (Some(timestamp), _) => format!("\nLast remembered: {}", format_timestamp(timestamp)),
            _ => String::new(),
        };
        format!(
            "On · active window only · up to once per {interval}\nModel: {} · {} observations stored · screenshots retained: 0{processing}{failures}{last_remembered}",
            status.screen_model, status.screen_observation_count,
        )
    };
    state.screen_status.set_text(&screen_text);
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

fn render_timeline_entries(list: &gtk::ListBox, state: &Rc<UiState>, entries: &[TimelineEntry]) {
    clear_list(list);
    if entries.is_empty() {
        render_error(
            list,
            "Nothing remembered on this day",
            "Choose another day, or create a manual Remembrie above.",
        );
        return;
    }
    let target_id = state.timeline_target_id.borrow().clone();
    let mut target_row = None;
    for entry in entries {
        let row = timeline_entry_row(state, entry);
        if target_id.as_deref() == Some(entry.id.as_str()) {
            row.add_css_class("timeline-entry-target");
            target_row = Some(row.clone());
        }
        list.append(&row);
    }
    if let Some(row) = target_row {
        gtk::glib::idle_add_local_once(move || {
            let _ = row.grab_focus();
        });
    }
}

fn timeline_entry_row(state: &Rc<UiState>, entry: &TimelineEntry) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    let outer = gtk::Box::new(Orientation::Horizontal, 0);
    let primary_app = entry
        .activity
        .as_ref()
        .and_then(|activity| activity.observations.first())
        .map(|observation| (observation.app_id.as_str(), observation.app_name.as_str()))
        .unwrap_or_else(|| {
            let source = entry.source_app.as_deref().unwrap_or("Membrie");
            (source, source)
        });
    let color = app_color_index(primary_app.0, primary_app.1);
    let accent = gtk::Box::new(Orientation::Vertical, 0);
    accent.add_css_class("timeline-entry-accent");
    accent.add_css_class(&format!("app-solid-{color}"));
    outer.append(&accent);

    let content = gtk::Box::new(Orientation::Vertical, 7);
    content.set_hexpand(true);
    content.set_margin_top(14);
    content.set_margin_bottom(14);
    content.set_margin_start(14);
    content.set_margin_end(14);

    let top = gtk::Box::new(Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some(&entry.title));
    title.add_css_class("heading");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let time = gtk::Label::new(Some(&format_entry_time(entry)));
    time.add_css_class("caption");
    time.add_css_class("dim-label");
    top.append(&title);
    top.append(&time);
    content.append(&top);

    if let Some(activity) = &entry.activity {
        let durations = activity_app_durations(entry);
        if !durations.is_empty() {
            let apps = gtk::FlowBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .column_spacing(6)
                .row_spacing(5)
                .max_children_per_line(8)
                .build();
            for (identity, name, duration) in durations.into_iter().take(8) {
                let chip = gtk::Box::new(Orientation::Horizontal, 5);
                chip.add_css_class("timeline-app-chip");
                chip.add_css_class(&format!("app-fill-{}", app_color_index(&identity, &name)));
                let dot = gtk::Box::new(Orientation::Horizontal, 0);
                dot.add_css_class("timeline-app-dot");
                dot.add_css_class(&format!("app-solid-{}", app_color_index(&identity, &name)));
                let label =
                    gtk::Label::new(Some(&format!("{name} · {}", format_duration(duration))));
                label.add_css_class("caption");
                chip.append(&dot);
                chip.append(&label);
                apps.insert(&chip, -1);
            }
            content.append(&apps);
        }

        let description = entry.summary.clone().unwrap_or_else(|| {
            let app_count = activity_app_durations(entry).len();
            format!(
                "{} application or window changes across {app_count} {}.",
                activity.observation_count,
                if app_count == 1 {
                    "application"
                } else {
                    "applications"
                }
            )
        });
        if !description.trim().is_empty() {
            let summary = gtk::Label::new(Some(&description));
            summary.set_xalign(0.0);
            summary.set_wrap(true);
            summary.set_lines(3);
            summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
            content.append(&summary);
        }

        let evidence = gtk::Label::new(Some(&format!(
            "{} focus changes · {} semantic observations · {} screen observations · ended {}",
            activity.observation_count,
            activity.semantic_observation_count,
            activity.screen_observation_count,
            activity_end_label(&activity.end_reason)
        )));
        evidence.add_css_class("caption");
        evidence.add_css_class("dim-label");
        evidence.set_xalign(0.0);
        evidence.set_wrap(true);
        content.append(&evidence);
    } else {
        let context = entry
            .summary
            .as_deref()
            .or(entry.window_title.as_deref())
            .unwrap_or_else(|| entry.source_app.as_deref().unwrap_or("Manual Remembrie"));
        let detail = gtk::Label::new(Some(context));
        detail.set_xalign(0.0);
        detail.set_wrap(true);
        detail.set_lines(3);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.append(&detail);
    }

    let evidence_button = gtk::Button::with_label("Show exact evidence");
    evidence_button.add_css_class("flat");
    evidence_button.set_halign(Align::Start);
    let client = state.client.clone();
    let remembrie_id = entry.id.clone();
    evidence_button
        .connect_clicked(move |button| show_remembrance_evidence(button, &client, &remembrie_id));
    content.append(&evidence_button);

    outer.append(&content);
    row.set_child(Some(&outer));
    row
}

fn show_remembrance_evidence(
    parent: &impl IsA<gtk::Widget>,
    client: &DaemonClient,
    remembrie_id: &str,
) {
    match client.get_remembrie(remembrie_id) {
        Ok(Some(remembrie)) => {
            let source = remembrie.source_app.as_deref().unwrap_or("Unknown source");
            let body = if remembrie.body.trim().is_empty() {
                "No captured text was stored for this Remembrie.".to_owned()
            } else {
                remembrie.body.clone()
            };
            let evidence = gtk::Label::new(Some(&body));
            evidence.set_xalign(0.0);
            evidence.set_yalign(0.0);
            evidence.set_wrap(true);
            evidence.set_selectable(true);
            evidence.add_css_class("monospace");
            let scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .min_content_width(680)
                .min_content_height(420)
                .child(&evidence)
                .build();
            let dialog = adw::AlertDialog::builder()
                .heading(&remembrie.title)
                .body(format!(
                    "{} · {source}",
                    format_timestamp(remembrie.occurred_at_ms)
                ))
                .extra_child(&scroll)
                .build();
            dialog.add_response("close", "Close");
            dialog.set_default_response(Some("close"));
            dialog.present(Some(parent));
        }
        Ok(None) => {
            let dialog = adw::AlertDialog::builder()
                .heading("Remembrie no longer available")
                .body("It may have been deleted while the Timeline was open.")
                .build();
            dialog.add_response("close", "Close");
            dialog.present(Some(parent));
        }
        Err(error) => {
            let dialog = adw::AlertDialog::builder()
                .heading("Exact evidence could not be opened")
                .body(error.to_string())
                .build();
            dialog.add_response("close", "Close");
            dialog.present(Some(parent));
        }
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

fn clear_grid(grid: &gtk::Grid) {
    while let Some(child) = grid.first_child() {
        grid.remove(&child);
    }
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn clear_flow_box(container: &gtk::FlowBox) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn activity_app_durations(entry: &TimelineEntry) -> Vec<(String, String, i64)> {
    let Some(activity) = &entry.activity else {
        return Vec::new();
    };
    let mut totals: HashMap<String, (String, i64)> = HashMap::new();
    for (index, observation) in activity.observations.iter().enumerate() {
        let ended_at_ms = activity
            .observations
            .get(index + 1)
            .map(|next| next.observed_at_ms)
            .unwrap_or(entry.ended_at_ms)
            .min(entry.ended_at_ms);
        let started_at_ms = observation.observed_at_ms.max(entry.started_at_ms);
        if ended_at_ms <= started_at_ms {
            continue;
        }
        let app_name = timeline_observation_app_name(observation);
        let identity = app_identity(&observation.app_id, &app_name);
        let total = totals.entry(identity).or_insert((app_name, 0));
        total.1 += ended_at_ms - started_at_ms;
    }
    let mut durations: Vec<(String, String, i64)> = totals
        .into_iter()
        .map(|(identity, (name, duration))| (identity, name, duration))
        .collect();
    durations.sort_by_key(|item| std::cmp::Reverse(item.2));
    durations
}

fn timeline_observation_app_name(observation: &TimelineActivityObservation) -> String {
    if !observation.app_name.trim().is_empty() {
        observation.app_name.trim().to_owned()
    } else if !observation.app_id.trim().is_empty() {
        observation.app_id.trim().to_owned()
    } else {
        "Desktop".to_owned()
    }
}

fn app_identity(app_id: &str, app_name: &str) -> String {
    let identity = if app_id.trim().is_empty() {
        app_name
    } else {
        app_id
    };
    identity.trim().to_ascii_lowercase()
}

fn app_color_index(app_id: &str, app_name: &str) -> u8 {
    let mut hash = 2_166_136_261_u32;
    for byte in app_identity(app_id, app_name).bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    (hash % 8) as u8
}

fn activity_end_label(reason: &str) -> &'static str {
    match reason {
        "idle" => "at an idle break",
        "screen_locked" => "when the screen locked",
        "manual" => "manually",
        "capture_paused" => "when capture paused",
        "excluded_context" => "at an excluded context",
        "sensitive_context" => "at sensitive context",
        "activity_disabled" => "when Activity Context was disabled",
        "daemon_restart" => "at a service restart",
        "desktop_bridge_unavailable" => "when the desktop bridge disconnected",
        "service_stopped" => "when the capture service stopped",
        _ => "at a session boundary",
    }
}

fn format_entry_time(entry: &TimelineEntry) -> String {
    if entry.ended_at_ms > entry.started_at_ms {
        format!(
            "{}–{} · {}",
            format_clock(entry.started_at_ms),
            format_clock(entry.ended_at_ms),
            format_duration(entry.ended_at_ms - entry.started_at_ms)
        )
    } else {
        format_clock(entry.started_at_ms)
    }
}

fn format_duration(duration_ms: i64) -> String {
    if duration_ms <= 0 {
        return "0m".to_owned();
    }
    if duration_ms < 60_000 {
        return "<1m".to_owned();
    }
    let minutes = (duration_ms + 30_000) / 60_000;
    match minutes {
        1..60 => format!("{minutes}m"),
        _ => format!("{}h {:02}m", minutes / 60, minutes % 60),
    }
}

fn format_clock(timestamp_ms: i64) -> String {
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .and_then(|date| date.format("%-I:%M %p"))
        .map(|text| text.to_string())
        .unwrap_or_else(|_| "Unknown time".to_owned())
}

fn local_today_start_ms() -> i64 {
    gtk::glib::DateTime::now_local()
        .ok()
        .map(|now| now.to_unix() * 1000)
        .map(local_day_start_ms)
        .unwrap_or_else(|| current_time_ms() - current_time_ms().rem_euclid(86_400_000))
}

fn local_day_start_ms(timestamp_ms: i64) -> i64 {
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .and_then(|date| {
            gtk::glib::DateTime::from_local(
                date.year(),
                date.month(),
                date.day_of_month(),
                0,
                0,
                0.0,
            )
        })
        .map(|date| date.to_unix() * 1000)
        .unwrap_or(timestamp_ms - timestamp_ms.rem_euclid(86_400_000))
}

fn add_local_days(timestamp_ms: i64, days: i32) -> i64 {
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .and_then(|date| date.add_days(days))
        .map(|date| date.to_unix() * 1000)
        .unwrap_or(timestamp_ms.saturating_add(i64::from(days) * 86_400_000))
}

fn add_local_hours(timestamp_ms: i64, hours: i32) -> i64 {
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .and_then(|date| date.add_hours(hours))
        .map(|date| date.to_unix() * 1000)
        .unwrap_or(timestamp_ms.saturating_add(i64::from(hours) * 3_600_000))
}

fn add_local_months(timestamp_ms: i64, months: i32) -> i64 {
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .and_then(|date| date.add_months(months))
        .map(|date| date.to_unix() * 1000)
        .unwrap_or(timestamp_ms)
}

fn local_day_of_week(timestamp_ms: i64) -> i32 {
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .map(|date| date.day_of_week())
        .unwrap_or(1)
}

fn local_month_start_ms(timestamp_ms: i64) -> i64 {
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .and_then(|date| gtk::glib::DateTime::from_local(date.year(), date.month(), 1, 0, 0, 0.0))
        .map(|date| date.to_unix() * 1000)
        .unwrap_or(timestamp_ms)
}

fn timeline_map_range(day_start: i64, mode: TimelineMapMode) -> (i64, i64) {
    match mode {
        TimelineMapMode::Day => (day_start, add_local_days(day_start, 1)),
        TimelineMapMode::Week => {
            let week_start = add_local_days(day_start, -(local_day_of_week(day_start) - 1));
            (week_start, add_local_days(week_start, 7))
        }
        TimelineMapMode::Month => {
            let month_start = local_month_start_ms(day_start);
            (month_start, add_local_months(month_start, 1))
        }
    }
}

fn shift_timeline_period(day_start: i64, mode: TimelineMapMode, amount: i32) -> i64 {
    match mode {
        TimelineMapMode::Day => add_local_days(day_start, amount),
        TimelineMapMode::Week => add_local_days(day_start, amount.saturating_mul(7)),
        TimelineMapMode::Month => add_local_months(day_start, amount),
    }
}

fn local_day_number(timestamp_ms: i64) -> String {
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .map(|date| date.day_of_month().to_string())
        .unwrap_or_default()
}

fn format_map_row_day(timestamp_ms: i64) -> String {
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .and_then(|date| date.format("%a %-d"))
        .map(|text| text.to_string())
        .unwrap_or_else(|_| "Day".to_owned())
}

fn format_map_cell_time(timestamp_ms: i64, is_day_cell: bool) -> String {
    let pattern = if is_day_cell {
        "%A, %B %-d"
    } else {
        "%A, %B %-d · %-I %p"
    };
    gtk::glib::DateTime::from_unix_local(timestamp_ms / 1000)
        .and_then(|date| date.format(pattern))
        .map(|text| text.to_string())
        .unwrap_or_else(|_| "Unknown time".to_owned())
}

fn display_map_app_name(app_id: &str, app_name: &str) -> String {
    if !app_name.trim().is_empty() {
        app_name.trim().to_owned()
    } else if !app_id.trim().is_empty() {
        app_id.trim().to_owned()
    } else {
        "Desktop".to_owned()
    }
}

fn format_timeline_day(day_start: i64, today: i64) -> String {
    let prefix = if day_start == today { "Today · " } else { "" };
    gtk::glib::DateTime::from_unix_local(day_start / 1000)
        .and_then(|date| date.format("%A, %B %-d"))
        .map(|date| format!("{prefix}{date}"))
        .unwrap_or_else(|_| "Timeline day".to_owned())
}

fn overlap_ms(left_start: i64, left_end: i64, right_start: i64, right_end: i64) -> i64 {
    left_end
        .min(right_end)
        .saturating_sub(left_start.max(right_start))
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
         .quick-brie { background: @window_bg_color; }
         .quick-context {
             padding: 8px 10px;
             border-radius: 8px;
             background: alpha(@accent_bg_color, 0.10);
         }
         .quick-conversation {
             border: 1px solid @borders;
             border-radius: 10px;
             background: @view_bg_color;
         }
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
         .citation-button { padding: 5px 8px; }
         .title { font-weight: 700; }
         .heading { font-weight: 600; }
         .timeline-day-navigation { margin-bottom: 2px; }
         .timeline-section {
             background: color-mix(in srgb, @view_bg_color 88%, @accent_bg_color 12%);
             border: 1px solid @borders;
             border-radius: 12px;
             padding: 16px;
         }
         .timeline-history-grid { margin-top: 6px; }
         .timeline-map-hour-cell {
             min-width: 18px;
             min-height: 24px;
             padding: 0;
             border-radius: 4px;
             border: 1px solid alpha(@window_fg_color, 0.08);
             box-shadow: none;
         }
         .timeline-map-day-cell {
             min-width: 54px;
             min-height: 38px;
             padding: 4px;
             border-radius: 7px;
             border: 1px solid alpha(@window_fg_color, 0.08);
             box-shadow: none;
         }
         .timeline-map-empty { background: alpha(@window_fg_color, 0.08); }
         .timeline-map-level-1 { opacity: 0.34; }
         .timeline-map-level-2 { opacity: 0.54; }
         .timeline-map-level-3 { opacity: 0.76; }
         .timeline-map-level-4 { opacity: 1; }
         .timeline-map-selected { border: 2px solid @window_fg_color; }
         .app-map-0 { background: #7459c7; color: white; }
         .app-map-1 { background: #3584e4; color: white; }
         .app-map-2 { background: #2ec27e; color: white; }
         .app-map-3 { background: #e66100; color: white; }
         .app-map-4 { background: #e01b24; color: white; }
         .app-map-5 { background: #1c8c8c; color: white; }
         .app-map-6 { background: #c061cb; color: white; }
         .app-map-7 { background: #986a44; color: white; }
         .timeline-ribbon-grid { margin-top: 2px; }
         .timeline-ribbon-track {
             background: alpha(@window_fg_color, 0.08);
             border-radius: 8px;
         }
         .timeline-ribbon-segment {
             min-height: 42px;
             border-radius: 7px;
             border-left-width: 3px;
             border-left-style: solid;
         }
         .timeline-legend-item { padding: 3px 0; }
         .timeline-app-dot { min-width: 10px; min-height: 10px; border-radius: 999px; }
         .timeline-app-chip { padding: 4px 8px; border-radius: 999px; }
         .timeline-entry-accent { min-width: 5px; }
         .timeline-entry-target {
             background: alpha(@accent_bg_color, 0.13);
             border: 2px solid @accent_bg_color;
             border-radius: 10px;
         }
         .timeline-insight {
             padding: 16px;
             border-left: 4px solid #7459c7;
             background: alpha(#7459c7, 0.12);
         }
         .manual-capture { padding: 10px 14px; }
         .manual-capture-content { padding: 12px 2px 2px 2px; }
         .app-fill-0 { background: alpha(#7459c7, 0.22); border-color: #7459c7; }
         .app-fill-1 { background: alpha(#3584e4, 0.22); border-color: #3584e4; }
         .app-fill-2 { background: alpha(#2ec27e, 0.22); border-color: #2ec27e; }
         .app-fill-3 { background: alpha(#e66100, 0.22); border-color: #e66100; }
         .app-fill-4 { background: alpha(#e01b24, 0.20); border-color: #e01b24; }
         .app-fill-5 { background: alpha(#1c8c8c, 0.22); border-color: #1c8c8c; }
         .app-fill-6 { background: alpha(#c061cb, 0.22); border-color: #c061cb; }
         .app-fill-7 { background: alpha(#986a44, 0.24); border-color: #986a44; }
         .app-solid-0 { background: #7459c7; }
         .app-solid-1 { background: #3584e4; }
         .app-solid-2 { background: #2ec27e; }
         .app-solid-3 { background: #e66100; }
         .app-solid-4 { background: #e01b24; }
         .app-solid-5 { background: #1c8c8c; }
         .app-solid-6 { background: #c061cb; }
         .app-solid-7 { background: #986a44; }",
    );
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("a graphical display is required"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

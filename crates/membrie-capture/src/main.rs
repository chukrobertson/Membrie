use anyhow::{Context, Result, bail};
use gio::prelude::*;
use membrie_core::{
    ActivitySnapshot, CaptureCandidate, CaptureDecision, DaemonClient, socket_path,
};

const DBUS_NAME: &str = "com.chuk.Membrie.Clipboard";
const DBUS_PATH: &str = "/com/chuk/Membrie/Clipboard";
const DBUS_INTERFACE: &str = "com.chuk.Membrie.Clipboard";

const TEXT_MIMETYPES: &[&str] = &[
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
];

const PASSWORD_MANAGER_MIMETYPES: &[&str] = &[
    "x-kde-passwordManagerHint",
    "application/x-kde-passwordmanagerhint",
    "application/x-keepassxc",
];

fn main() -> Result<()> {
    let proxy = gio::DBusProxy::for_bus_sync(
        gio::BusType::Session,
        gio::DBusProxyFlags::DO_NOT_AUTO_START,
        None,
        DBUS_NAME,
        DBUS_PATH,
        DBUS_INTERFACE,
        None::<&gio::Cancellable>,
    )
    .context("could not connect to the GNOME desktop bridge")?;
    if proxy.name_owner().is_none() {
        bail!(
            "the Membrie GNOME Desktop Bridge is not enabled; run scripts/install-gnome-extension.sh"
        );
    }

    let client = DaemonClient::new(socket_path());
    let client_for_signals = client.clone();
    proxy.connect_g_signal(
        None,
        move |proxy, _, signal_name, parameters| match signal_name {
            "OwnerChanged" => request_clipboard_text(proxy, &client_for_signals, parameters),
            "TextReady" => collect_clipboard_text(proxy, &client_for_signals),
            "ActivityChanged" => poll_activity_state(proxy, &client_for_signals),
            _ => {}
        },
    );
    let client_for_owner = client.clone();
    proxy.connect_g_name_owner_notify(move |proxy| {
        if proxy.name_owner().is_none() {
            if let Err(error) = client_for_owner.end_activity_session("desktop_bridge_unavailable")
            {
                eprintln!("Could not close the desktop activity session: {error}");
            }
        } else {
            poll_activity_state(proxy, &client_for_owner);
        }
    });
    poll_desktop(&proxy, &client);
    let proxy_for_timer = proxy.clone();
    glib::timeout_add_seconds_local(10, move || {
        poll_desktop(&proxy_for_timer, &client);
        glib::ControlFlow::Continue
    });

    println!("Membrie GNOME desktop capture ready");
    glib::MainLoop::new(None, false).run();
    Ok(())
}

fn poll_desktop(proxy: &gio::DBusProxy, client: &DaemonClient) {
    match client.record_clipboard_agent_heartbeat() {
        Ok(status) if status.activity_enabled && !status.paused => {
            request_activity_state(proxy, client);
        }
        Ok(_) => {}
        Err(error) => eprintln!("Could not report desktop capture health: {error}"),
    }
}

fn poll_activity_state(proxy: &gio::DBusProxy, client: &DaemonClient) {
    if client
        .status()
        .is_ok_and(|status| status.activity_enabled && !status.paused)
    {
        request_activity_state(proxy, client);
    }
}

fn request_activity_state(proxy: &gio::DBusProxy, client: &DaemonClient) {
    let response = proxy.call_sync(
        "GetActivityState",
        None,
        gio::DBusCallFlags::NONE,
        1_000,
        None::<&gio::Cancellable>,
    );
    let state = match response
        .as_ref()
        .map_err(|error| error.to_string())
        .and_then(|value| {
            value
                .try_get::<(String, String, String, u32, bool)>()
                .map_err(|error| error.to_string())
        }) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("Could not collect desktop activity state: {error}");
            return;
        }
    };

    let client = client.clone();
    std::thread::spawn(move || {
        let snapshot = ActivitySnapshot {
            app_id: state.0,
            app_name: state.1,
            window_title: state.2,
            idle_ms: u64::from(state.3),
            locked: state.4,
            occurred_at_ms: None,
        };
        if let Err(error) = client.record_activity(snapshot) {
            eprintln!("Desktop activity capture failed: {error}");
        }
    });
}

fn request_clipboard_text(
    proxy: &gio::DBusProxy,
    client: &DaemonClient,
    parameters: &glib::Variant,
) {
    let Ok((mimetypes,)) = parameters.try_get::<(Vec<String>,)>() else {
        eprintln!("Clipboard bridge sent invalid format metadata");
        return;
    };

    if PASSWORD_MANAGER_MIMETYPES
        .iter()
        .any(|protected| mimetypes.iter().any(|value| value == protected))
    {
        println!("Skipped protected password-manager clipboard content");
        return;
    }
    if !TEXT_MIMETYPES
        .iter()
        .any(|text_type| mimetypes.iter().any(|value| value == text_type))
    {
        return;
    }
    if !client
        .status()
        .map(|status| status.clipboard_enabled)
        .unwrap_or(false)
    {
        return;
    }

    if let Err(error) = proxy.call_sync(
        "RequestText",
        None,
        gio::DBusCallFlags::NONE,
        1_000,
        None::<&gio::Cancellable>,
    ) {
        eprintln!("Could not request clipboard text: {error}");
    }
}

fn collect_clipboard_text(proxy: &gio::DBusProxy, client: &DaemonClient) {
    if !client
        .status()
        .map(|status| status.clipboard_enabled)
        .unwrap_or(false)
    {
        return;
    }

    let response = proxy.call_sync(
        "GetText",
        None,
        gio::DBusCallFlags::NONE,
        1_000,
        None::<&gio::Cancellable>,
    );
    let text = match response
        .as_ref()
        .map_err(|error| error.to_string())
        .and_then(|value| {
            value
                .try_get::<(String,)>()
                .map(|(text,)| text)
                .map_err(|error| error.to_string())
        }) {
        Ok(text) if !text.trim().is_empty() => text,
        Ok(_) => return,
        Err(error) => {
            eprintln!("Could not collect clipboard text: {error}");
            return;
        }
    };

    let client = client.clone();
    std::thread::spawn(
        move || match client.capture(CaptureCandidate::clipboard(text)) {
            Ok(CaptureDecision::Stored { remembrie }) => {
                println!("Remembered clipboard {}", remembrie.id);
            }
            Ok(CaptureDecision::Skipped {
                category, reason, ..
            }) => {
                println!("Skipped clipboard ({category}): {reason}");
            }
            Err(error) => eprintln!("Clipboard capture failed: {error}"),
        },
    );
}

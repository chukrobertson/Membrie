use anyhow::{Context, Result, bail};
use gio::prelude::*;
use membrie_core::{CaptureCandidate, CaptureDecision, DaemonClient, socket_path};

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
    .context("could not connect to the GNOME clipboard bridge")?;
    if proxy.name_owner().is_none() {
        bail!(
            "the Membrie GNOME Clipboard Bridge is not enabled; run scripts/install-gnome-extension.sh"
        );
    }

    let client = DaemonClient::new(socket_path());
    proxy.connect_g_signal(
        None,
        move |proxy, _, signal_name, parameters| match signal_name {
            "OwnerChanged" => request_clipboard_text(proxy, &client, parameters),
            "TextReady" => collect_clipboard_text(proxy, &client),
            _ => {}
        },
    );

    println!("Membrie GNOME clipboard capture ready");
    glib::MainLoop::new(None, false).run();
    Ok(())
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

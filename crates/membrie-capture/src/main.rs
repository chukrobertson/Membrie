use anyhow::{Context, Result, bail};
use gdk_pixbuf::prelude::*;
use membrie_core::{
    ActivitySnapshot, CaptureCandidate, CaptureDecision, DaemonClient, ScreenCaptureCandidate,
    screen_spool_dir, socket_path,
};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

#[derive(Default)]
struct ScreenSampler {
    last_sample_at: Option<Instant>,
    last_kept_fingerprint: Option<Vec<u8>>,
}

fn main() -> Result<()> {
    clean_screen_spool();
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
    let screen_sampler = Arc::new(Mutex::new(ScreenSampler::default()));
    let screen_analysis_busy = Arc::new(AtomicBool::new(false));
    let client_for_signals = client.clone();
    let sampler_for_signals = Arc::clone(&screen_sampler);
    let busy_for_signals = Arc::clone(&screen_analysis_busy);
    proxy.connect_g_signal(
        None,
        move |proxy, _, signal_name, parameters| match signal_name {
            "OwnerChanged" => request_clipboard_text(proxy, &client_for_signals, parameters),
            "TextReady" => collect_clipboard_text(proxy, &client_for_signals),
            "ActivityChanged" => poll_activity_state(
                proxy,
                &client_for_signals,
                &sampler_for_signals,
                &busy_for_signals,
            ),
            _ => {}
        },
    );
    let client_for_owner = client.clone();
    let sampler_for_owner = Arc::clone(&screen_sampler);
    let busy_for_owner = Arc::clone(&screen_analysis_busy);
    proxy.connect_g_name_owner_notify(move |proxy| {
        if proxy.name_owner().is_none() {
            if let Err(error) = client_for_owner.end_activity_session("desktop_bridge_unavailable")
            {
                eprintln!("Could not close the desktop activity session: {error}");
            }
        } else {
            poll_activity_state(
                proxy,
                &client_for_owner,
                &sampler_for_owner,
                &busy_for_owner,
            );
        }
    });
    poll_desktop(&proxy, &client, &screen_sampler, &screen_analysis_busy);
    let proxy_for_timer = proxy.clone();
    glib::timeout_add_seconds_local(10, move || {
        poll_desktop(
            &proxy_for_timer,
            &client,
            &screen_sampler,
            &screen_analysis_busy,
        );
        glib::ControlFlow::Continue
    });

    println!("Membrie GNOME desktop capture ready");
    glib::MainLoop::new(None, false).run();
    Ok(())
}

fn poll_desktop(
    proxy: &gio::DBusProxy,
    client: &DaemonClient,
    screen_sampler: &Arc<Mutex<ScreenSampler>>,
    screen_analysis_busy: &Arc<AtomicBool>,
) {
    match client.record_clipboard_agent_heartbeat() {
        Ok(status) if status.activity_enabled && !status.paused => {
            request_activity_state(
                proxy,
                client,
                screen_sampler,
                screen_analysis_busy,
                status.screen_sample_interval_ms,
            );
        }
        Ok(_) => {}
        Err(error) => eprintln!("Could not report desktop capture health: {error}"),
    }
}

fn poll_activity_state(
    proxy: &gio::DBusProxy,
    client: &DaemonClient,
    screen_sampler: &Arc<Mutex<ScreenSampler>>,
    screen_analysis_busy: &Arc<AtomicBool>,
) {
    if let Ok(status) = client.status()
        && status.activity_enabled
        && !status.paused
    {
        request_activity_state(
            proxy,
            client,
            screen_sampler,
            screen_analysis_busy,
            status.screen_sample_interval_ms,
        );
    }
}

fn request_activity_state(
    proxy: &gio::DBusProxy,
    client: &DaemonClient,
    screen_sampler: &Arc<Mutex<ScreenSampler>>,
    screen_analysis_busy: &Arc<AtomicBool>,
    sample_interval_ms: u64,
) {
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

    let snapshot = ActivitySnapshot {
        app_id: state.0,
        app_name: state.1,
        window_title: state.2,
        idle_ms: u64::from(state.3),
        locked: state.4,
        occurred_at_ms: None,
    };
    let activity = match client.record_activity(snapshot) {
        Ok(activity) => activity,
        Err(error) => {
            eprintln!("Desktop activity capture failed: {error}");
            return;
        }
    };
    if !activity.screen_capture_allowed || screen_analysis_busy.load(Ordering::Relaxed) {
        return;
    }
    let Some(session_id) = activity.session_id else {
        return;
    };
    let interval = Duration::from_millis(sample_interval_ms.clamp(30_000, 300_000));
    let due = {
        let Ok(sampler) = screen_sampler.lock() else {
            eprintln!("Screen-change state became unavailable");
            return;
        };
        let elapsed = sampler
            .last_sample_at
            .map(|last| last.elapsed())
            .unwrap_or(Duration::MAX);
        elapsed >= interval
    };
    if !due {
        return;
    }
    if let Ok(mut sampler) = screen_sampler.lock() {
        sampler.last_sample_at = Some(Instant::now());
    }
    capture_changed_screen(
        proxy,
        client,
        screen_sampler,
        screen_analysis_busy,
        session_id,
    );
}

fn capture_changed_screen(
    proxy: &gio::DBusProxy,
    client: &DaemonClient,
    screen_sampler: &Arc<Mutex<ScreenSampler>>,
    screen_analysis_busy: &Arc<AtomicBool>,
    session_id: String,
) {
    let response = proxy.call_sync(
        "CaptureWindow",
        None,
        gio::DBusCallFlags::NONE,
        10_000,
        None::<&gio::Cancellable>,
    );
    let (path, app_id, app_name, window_title, width, height) = match response
        .as_ref()
        .map_err(|error| error.to_string())
        .and_then(|value| {
            value
                .try_get::<(String, String, String, String, u32, u32)>()
                .map_err(|error| error.to_string())
        }) {
        Ok(capture) => capture,
        Err(error) => {
            eprintln!("Could not collect a private screen sample: {error}");
            return;
        }
    };

    let fingerprint = match screen_fingerprint(&path) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            let _ = fs::remove_file(&path);
            eprintln!("Could not compare a private screen sample: {error:#}");
            return;
        }
    };
    let changed = screen_sampler
        .lock()
        .ok()
        .and_then(|sampler| {
            sampler
                .last_kept_fingerprint
                .as_ref()
                .map(|previous| materially_changed(previous, &fingerprint))
        })
        .unwrap_or(true);
    if !changed {
        let _ = fs::remove_file(&path);
        return;
    }
    if let Ok(mut sampler) = screen_sampler.lock() {
        sampler.last_kept_fingerprint = Some(fingerprint);
    }
    if screen_analysis_busy.swap(true, Ordering::AcqRel) {
        let _ = fs::remove_file(&path);
        return;
    }

    let candidate = ScreenCaptureCandidate {
        session_id,
        screenshot_path: path.clone(),
        app_id,
        app_name,
        window_title,
        observed_at_ms: Some(current_time_ms()),
        width,
        height,
    };
    let client = client.clone();
    let busy = Arc::clone(screen_analysis_busy);
    std::thread::spawn(move || {
        match client.analyze_screen(candidate) {
            Ok(result) if result.outcome == "stored" => {
                println!("Remembered changed active-window context");
            }
            Ok(result) => {
                println!(
                    "Skipped screen context: {}",
                    result.reason.as_deref().unwrap_or("not useful")
                );
            }
            Err(error) => eprintln!("Local screen analysis failed: {error}"),
        }
        let _ = fs::remove_file(path);
        busy.store(false, Ordering::Release);
    });
}

fn screen_fingerprint(path: &str) -> Result<Vec<u8>> {
    let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_scale(path, 96, 54, true)
        .context("the captured PNG could not be decoded")?;
    let width = pixbuf.width() as usize;
    let height = pixbuf.height() as usize;
    let channels = pixbuf.n_channels() as usize;
    let rowstride = pixbuf.rowstride() as usize;
    let pixels = pixbuf.read_pixel_bytes();
    let pixels = pixels.as_ref();
    let mut fingerprint = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let offset = y * rowstride + x * channels;
            let red = u16::from(pixels[offset]);
            let green = u16::from(pixels[offset + 1]);
            let blue = u16::from(pixels[offset + 2]);
            fingerprint.push(((red * 77 + green * 150 + blue * 29) >> 8) as u8);
        }
    }
    Ok(fingerprint)
}

fn materially_changed(previous: &[u8], current: &[u8]) -> bool {
    if previous.len() != current.len() || current.is_empty() {
        return true;
    }
    let mut absolute_difference = 0_u64;
    let mut visibly_changed = 0_usize;
    for (&left, &right) in previous.iter().zip(current) {
        let difference = left.abs_diff(right);
        absolute_difference += u64::from(difference);
        if difference >= 18 {
            visibly_changed += 1;
        }
    }
    let mean_difference = absolute_difference as f64 / current.len() as f64;
    let changed_fraction = visibly_changed as f64 / current.len() as f64;
    mean_difference >= 4.0 || changed_fraction >= 0.04
}

fn clean_screen_spool() {
    let directory = screen_spool_dir();
    if let Err(error) = fs::create_dir_all(&directory) {
        eprintln!("Could not prepare the private screen spool: {error}");
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)) {
            eprintln!("Could not make the private screen spool user-only: {error}");
            return;
        }
    }
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("png") {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_tiny_frame_noise() {
        let previous = vec![100_u8; 100];
        let mut current = previous.clone();
        current[0] = 105;
        assert!(!materially_changed(&previous, &current));
    }

    #[test]
    fn notices_meaningful_visual_change() {
        let previous = vec![20_u8; 100];
        let mut current = previous.clone();
        for value in current.iter_mut().take(10) {
            *value = 220;
        }
        assert!(materially_changed(&previous, &current));
    }
}

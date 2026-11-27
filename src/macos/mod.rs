//! macOS integration used by the otherwise cross-platform island UI.

use crate::display_layout::{Bounds, Display};
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window::{
    copy_window_info, kCGWindowBounds, kCGWindowLayer, kCGWindowListExcludeDesktopElements,
    kCGWindowListOptionOnScreenOnly, kCGWindowName, kCGWindowOwnerName, kCGWindowOwnerPID,
};
use objc::runtime::{Class, Object};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

static OUTPUT_VOLUME_CACHE: Mutex<Option<(Instant, Option<(u32, bool)>)>> = Mutex::new(None);
static NETWORK_CACHE: Mutex<Option<(Instant, Option<bool>)>> = Mutex::new(None);
static AVOIDANCE_CACHE: Mutex<Option<(Instant, Option<bool>)>> = Mutex::new(None);
static AUDIO_WORKER: std::sync::OnceLock<()> = std::sync::OnceLock::new();
static NETWORK_WORKER: std::sync::OnceLock<()> = std::sync::OnceLock::new();

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreate(source: *const c_void) -> *mut c_void;
    fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    fn CFRelease(value: *const c_void);
}

pub fn cursor_position() -> Option<(i32, i32)> {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let point = CGEventGetLocation(event);
        CFRelease(event.cast());
        Some((point.x.round() as i32, point.y.round() as i32))
    }
}

pub fn network_sample() -> Option<bool> {
    NETWORK_WORKER.get_or_init(|| {
        let _ = std::thread::Builder::new().name("daozai-network".into()).spawn(|| loop {
            let output = std::process::Command::new("/usr/sbin/scutil")
                .args(["-n", "default"])
                .output()
                .ok();
            let value = output.and_then(|output| {
                let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
                Some(text.contains("present") && output.status.success())
            });
            *NETWORK_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), value));
            std::thread::sleep(Duration::from_secs(15));
        });
    });
    NETWORK_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .and_then(|(_, value)| value)
}

#[derive(Clone, Copy, Debug, Default)]
struct NSRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

pub fn displays() -> Vec<Display> {
    unsafe {
        let Some(screen_class) = Class::get("NSScreen") else {
            return Vec::new();
        };
        let screens: *mut Object = msg_send![screen_class, screens];
        if screens.is_null() {
            return Vec::new();
        }
        let count: usize = msg_send![screens, count];
        if count == 0 {
            return Vec::new();
        }

        let main_screen: *mut Object = msg_send![screens, objectAtIndex: 0usize];
        let main_frame: NSRect = msg_send![main_screen, frame];
        let global_top = main_frame.y + main_frame.height;
        let mut result = Vec::with_capacity(count);
        for index in 0..count {
            let screen: *mut Object = msg_send![screens, objectAtIndex: index];
            let frame: NSRect = msg_send![screen, frame];
            let visible: NSRect = msg_send![screen, visibleFrame];
            let scale: f64 = msg_send![screen, backingScaleFactor];
            let bounds = Bounds {
                left: frame.x.round() as i32,
                top: (global_top - frame.y - frame.height).round() as i32,
                right: (frame.x + frame.width).round() as i32,
                bottom: (global_top - frame.y).round() as i32,
            };
            let work = Bounds {
                left: visible.x.round() as i32,
                top: (global_top - visible.y - visible.height).round() as i32,
                right: (visible.x + visible.width).round() as i32,
                bottom: (global_top - visible.y).round() as i32,
            };
            let primary = index == 0;
            result.push(Display {
                id: format!("screen-{index}-{}x{}", bounds.right - bounds.left, bounds.bottom - bounds.top),
                label: format!(
                    "显示器 {} · {} × {}{}",
                    index + 1,
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top,
                    if primary { " · 主屏" } else { "" }
                ),
                bounds,
                work,
                auto_bottom: 0,
                primary,
                dpi: (96.0 * scale).round() as u32,
            });
        }
        result
    }
}

pub fn configure_window() {
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        // Accessory apps stay out of the Dock and Command-Tab switcher.
        let _: i8 = msg_send![app, setActivationPolicy: 1i64];
        if let Some(window) = window_titled("岛仔") {
            let _: () = msg_send![window, setLevel: 3i64];
            // CanJoinAllSpaces | Stationary | FullScreenAuxiliary.
            let _: () = msg_send![window, setCollectionBehavior: (1u64 | 16u64 | 256u64)];
            let _: () = msg_send![window, setOpaque: false];
            let _: () = msg_send![window, setHasShadow: false];
            let _: () = msg_send![window, setHidesOnDeactivate: false];
        }
    }
}

unsafe fn window_titled(title: &str) -> Option<*mut Object> {
    let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
    let windows: *mut Object = msg_send![app, windows];
    let count: usize = msg_send![windows, count];
    for index in 0..count {
        let window: *mut Object = msg_send![windows, objectAtIndex: index];
        let candidate: *mut Object = msg_send![window, title];
        if candidate.is_null() {
            continue;
        }
        let candidate: *const std::ffi::c_char = msg_send![candidate, UTF8String];
        if !candidate.is_null()
            && std::ffi::CStr::from_ptr(candidate).to_string_lossy() == title
        {
            return Some(window);
        }
    }
    None
}

pub fn activate_settings_window() {
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
        if let Some(window) = window_titled("岛仔设置") {
            let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null_mut::<Object>()];
        }
    }
}

/// Toggle the transparent canvas' hit testing. The caller enables it only while
/// the pointer is over the island, so transparent margins pass clicks through.
pub fn set_window_interactive(interactive: bool) {
    unsafe {
        if let Some(window) = window_titled("岛仔") {
            let _: () = msg_send![window, setIgnoresMouseEvents: (!interactive) as i8];
        }
    }
}

pub fn pointer_should_interact(
    region: eframe::egui::Rect,
    hit: eframe::egui::Rect,
    notice_hit: Option<eframe::egui::Rect>,
    expanded: bool,
    local: eframe::egui::Pos2,
) -> bool {
    expanded
        || region
            .union(hit)
            .union(notice_hit.unwrap_or(eframe::egui::Rect::NOTHING))
            .expand(8.0)
            .contains(local)
}

pub fn window_frame() -> Option<(f32, f32, f32, f32)> {
    unsafe {
        let screen_class = Class::get("NSScreen")?;
        let screens: *mut Object = msg_send![screen_class, screens];
        if screens.is_null() {
            return None;
        }
        let count: usize = msg_send![screens, count];
        if count == 0 {
            return None;
        }
        let main_screen: *mut Object = msg_send![screens, objectAtIndex: 0usize];
        let screen_frame: NSRect = msg_send![main_screen, frame];
        let global_top = screen_frame.y + screen_frame.height;
        let window = window_titled("岛仔")?;
        let frame: NSRect = msg_send![window, frame];
        Some((
            frame.x as f32,
            (global_top - frame.y - frame.height) as f32,
            frame.width as f32,
            frame.height as f32,
        ))
    }
}

#[derive(Clone, Debug)]
struct NativeWindow {
    pid: i32,
    layer: i64,
    name: String,
    title: String,
    bounds: Bounds,
}

unsafe fn nsstring(object: *mut Object) -> Option<String> {
    if object.is_null() {
        return None;
    }
    let text: *const std::ffi::c_char = msg_send![object, UTF8String];
    (!text.is_null()).then(|| std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned())
}

fn application_path(pid: i32) -> Option<String> {
    unsafe {
        let class = Class::get("NSRunningApplication")?;
        let application: *mut Object =
            msg_send![class, runningApplicationWithProcessIdentifier: pid];
        if application.is_null() {
            return None;
        }
        let url: *mut Object = msg_send![application, bundleURL];
        if url.is_null() {
            return None;
        }
        let path: *mut Object = msg_send![url, path];
        nsstring(path)
    }
}

fn frontmost_application() -> Option<(i32, String, String)> {
    unsafe {
        let workspace_class = Class::get("NSWorkspace")?;
        let workspace: *mut Object = msg_send![workspace_class, sharedWorkspace];
        let application: *mut Object = msg_send![workspace, frontmostApplication];
        if application.is_null() {
            return None;
        }
        let pid: i32 = msg_send![application, processIdentifier];
        let name = nsstring(msg_send![application, localizedName]).unwrap_or_else(|| "应用".into());
        let path = application_path(pid).unwrap_or_default();
        (pid > 0).then_some((pid, name, path))
    }
}

fn cf_dictionary(value: *const c_void) -> Option<CFDictionary<CFString, CFType>> {
    (!value.is_null()).then(|| unsafe {
        CFDictionary::wrap_under_get_rule(value as CFDictionaryRef)
    })
}

fn dictionary_number(
    dictionary: &CFDictionary<CFString, CFType>,
    key: &str,
) -> Option<f64> {
    let key = CFString::new(key);
    dictionary
        .find(&key)
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|number| number.to_f64())
}

fn window_bounds(
    dictionary: &CFDictionary<CFString, CFType>,
) -> Option<Bounds> {
    let key = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };
    let bounds = cf_dictionary(dictionary.find(&key)?.as_CFTypeRef())?;
    let x = dictionary_number(&bounds, "X")?;
    let y = dictionary_number(&bounds, "Y")?;
    let width = dictionary_number(&bounds, "Width")?;
    let height = dictionary_number(&bounds, "Height")?;
    (width > 0.0 && height > 0.0).then(|| Bounds {
        left: x.round() as i32,
        top: y.round() as i32,
        right: (x + width).round() as i32,
        bottom: (y + height).round() as i32,
    })
}

fn onscreen_windows() -> Vec<NativeWindow> {
    let Some(info) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        0,
    ) else {
        return Vec::new();
    };
    let mut windows = Vec::new();
    for index in 0..info.len() {
        let Some(value) = info.get(index) else {
            continue;
        };
        let Some(dictionary) = cf_dictionary(*value) else {
            continue;
        };
        let pid_key = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) };
        let layer_key = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
        let pid = dictionary
            .find(&pid_key)
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|number| number.to_i32());
        let layer = dictionary
            .find(&layer_key)
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|number| number.to_i64())
            .unwrap_or_default();
        let Some(bounds) = window_bounds(&dictionary) else {
            continue;
        };
        let Some(pid) = pid.filter(|pid| *pid > 0) else {
            continue;
        };
        let name_key = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerName) };
        let title_key = unsafe { CFString::wrap_under_get_rule(kCGWindowName) };
        let name = dictionary
            .find(&name_key)
            .and_then(|value| value.downcast::<CFString>())
            .map(|value| value.to_string())
            .unwrap_or_default();
        let title = dictionary
            .find(&title_key)
            .and_then(|value| value.downcast::<CFString>())
            .map(|value| value.to_string())
            .unwrap_or_default();
        windows.push(NativeWindow {
            pid,
            layer,
            name,
            title,
            bounds,
        });
    }
    windows
}

fn is_browser_application(name: &str, path: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let path = path.to_ascii_lowercase();
    [
        "safari",
        "google chrome",
        "firefox",
        "microsoft edge",
        "brave browser",
        "arc",
        "vivaldi",
        "opera",
        "zen browser",
        "orion",
        "chromium",
    ]
    .iter()
    .any(|browser| name == *browser || path.ends_with(&format!("/{browser}.app")))
}

fn window_overlaps_island(window: Bounds, island: Bounds) -> bool {
    window.left < island.right
        && window.right > island.left
        && window.top < island.bottom
        && window.bottom > island.top
}

fn island_hit_bounds(canvas: Bounds, dock: crate::model::Dock) -> Bounds {
    if dock.is_side() {
        let scale = (canvas.bottom - canvas.top) as f32 / crate::side_dock::SIDE_H;
        let center_y =
            canvas.top as f32 + crate::side_dock::SIDE_H * scale / 2.0;
        let half = crate::app::PH * scale / 2.0;
        let strip = crate::side_dock::SIDE_BAR_W * scale;
        Bounds {
            left: if dock == crate::model::Dock::Left {
                canvas.left
            } else {
                (canvas.right as f32 - strip) as i32
            },
            top: (center_y - half) as i32,
            right: if dock == crate::model::Dock::Left {
                (canvas.left as f32 + strip) as i32
            } else {
                canvas.right
            },
            bottom: (center_y + half) as i32,
        }
    } else {
        let scale = (canvas.right - canvas.left) as f32 / crate::app::CANVAS_W;
        let center_x = canvas.left as f32 + crate::app::WIN_W * scale / 2.0;
        let half = crate::app::PW * scale / 2.0;
        let height = crate::app::BAR_H * scale;
        Bounds {
            left: (center_x - half) as i32,
            top: if dock == crate::model::Dock::Bottom {
                (canvas.bottom as f32 - height) as i32
            } else {
                canvas.top
            },
            right: (center_x + half) as i32,
            bottom: if dock == crate::model::Dock::Bottom {
                canvas.bottom
            } else {
                (canvas.top as f32 + height) as i32
            },
        }
    }
}

pub(crate) fn avoidance_candidates() -> Result<Vec<crate::avoidance::Candidate>, String> {
    let own_pid = std::process::id() as i32;
    let mut candidates = Vec::new();
    for window in onscreen_windows() {
        if window.pid == own_pid || window.layer != 0 {
            continue;
        }
        let path = application_path(window.pid).unwrap_or_default();
        if path.is_empty()
            || candidates
                .iter()
                .any(|candidate: &crate::avoidance::Candidate| {
                    crate::avoidance::same_app(&candidate.path, &path)
                })
        {
            continue;
        }
        let name = if window.name.is_empty() {
            Path::new(&path)
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("应用")
                .to_owned()
        } else {
            window.name
        };
        candidates.push(crate::avoidance::Candidate {
            browser: is_browser_application(&name, &path),
            path,
            name,
            title: window.title,
        });
    }
    candidates.sort_by_key(|candidate| candidate.name.to_lowercase());
    Ok(candidates)
}

fn avoidance_for_application(
    pid: i32,
    name: &str,
    path: &str,
    settings: &crate::model::Settings,
    own_pid: i32,
    island: Bounds,
    windows: &[NativeWindow],
) -> Option<bool> {
    if !settings.browser_minimal {
        return Some(false);
    }
    if pid == own_pid {
        return None;
    }
    let registered = crate::avoidance::contains_app(&settings.avoid_apps, path);
    let browser = settings.avoid_browsers && is_browser_application(name, path);
    if !registered && !browser {
        return Some(false);
    }
    Some(windows.iter().any(|window| {
        window.pid == pid
            && window.layer == 0
            && window_overlaps_island(window.bounds, island)
    }))
}

pub fn browser_at_top(settings: &crate::model::Settings) -> Option<bool> {
    if !settings.browser_minimal {
        *AVOIDANCE_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return Some(false);
    }
    if let Some((sampled, result)) = *AVOIDANCE_CACHE.lock().unwrap_or_else(|e| e.into_inner()) {
        if sampled.elapsed() < Duration::from_millis(120) {
            return result;
        }
    }
    let (pid, name, path) = frontmost_application()?;
    let (left, top, width, height) = window_frame()?;
    let canvas = Bounds {
        left: left.round() as i32,
        top: top.round() as i32,
        right: (left + width).round() as i32,
        bottom: (top + height).round() as i32,
    };
    let island = island_hit_bounds(canvas, settings.dock);
    let result = avoidance_for_application(
        pid,
        &name,
        &path,
        settings,
        std::process::id() as i32,
        island,
        &onscreen_windows(),
    );
    *AVOIDANCE_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((Instant::now(), result));
    result
}

pub fn output_volume() -> Option<(u32, bool)> {
    AUDIO_WORKER.get_or_init(|| {
        let _ = std::thread::Builder::new().name("daozai-audio".into()).spawn(|| loop {
            refresh_output_volume();
            std::thread::sleep(Duration::from_secs(2));
        });
    });
    OUTPUT_VOLUME_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .and_then(|(_, value)| value)
}

fn refresh_output_volume() {
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", "get volume settings"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    let value = output.and_then(|output| {
        let text = String::from_utf8_lossy(&output.stdout);
        let number = |name: &str| {
            text.split(',')
                .find_map(|part| part.trim().strip_prefix(name))
                .and_then(|value| value.trim().trim_end_matches(',').parse::<u32>().ok())
        };
        Some((number("output volume:")?, number("output muted:") != Some(1)))
    });
    *OUTPUT_VOLUME_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), value));
}

pub fn set_output_volume(volume: u32) {
    let _ = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &format!("set volume output volume {}", volume.min(100))])
        .status();
    *OUTPUT_VOLUME_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((Instant::now(), Some((volume.min(100), true))));
}

pub fn toggle_mute() {
    if let Some((_, unmuted)) = output_volume() {
        let _ = std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                &format!("set volume output muted {}", if unmuted { "true" } else { "false" }),
            ])
            .status();
        *OUTPUT_VOLUME_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((Instant::now(), Some((output_volume().map_or(0, |(v, _)| v), !unmuted))));
    }
}

fn mediaremote_paths() -> Option<(PathBuf, PathBuf)> {
    let exe = std::env::current_exe().ok()?;
    let app_root = exe.parent()?.parent()?;
    let bundled_script = app_root.join("Resources/mediaremote-adapter.pl");
    let bundled_framework = app_root.join("Resources/MediaRemoteAdapter.framework");
    if bundled_script.is_file() && bundled_framework.exists() {
        return Some((bundled_script, bundled_framework));
    }

    let cwd = std::env::current_dir().ok()?;
    let script = cwd.join("macos/mediaremote-adapter/bin/mediaremote-adapter.pl");
    let framework = cwd.join("macos/mediaremote-adapter/build/MediaRemoteAdapter.framework");
    (script.is_file() && framework.exists()).then_some((script, framework))
}

fn run_mediaremote(arguments: &[&str]) -> Option<String> {
    let (script, framework) = mediaremote_paths()?;
    let output = Command::new("/usr/bin/perl")
        .arg(script)
        .arg(framework)
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Fetch the system Now Playing record from MediaRemote. This covers every
/// player that integrates with macOS Control Center, including third-party
/// player applications which do not expose AppleScript controls.
pub fn now_playing() -> Option<serde_json::Value> {
    let text = run_mediaremote(&["get", "--now", "--no-artwork"])?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.is_object().then_some(value)
}

pub fn now_playing_artwork() -> Option<Vec<u8>> {
    let text = run_mediaremote(&["get", "--now"])?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(value.get("artworkData")?.as_str()?)
        .ok()
}

pub fn media_command(command: u32) -> bool {
    run_mediaremote(&["send", "--force", &command.to_string()]).is_some()
}

pub fn media_seek(seconds: f64) -> bool {
    if !seconds.is_finite() || seconds < 0.0 {
        return false;
    }
    let micros = (seconds * 1_000_000.0).round() as u64;
    run_mediaremote(&["seek", &micros.to_string()]).is_some()
}

pub fn open_file_picker(title: &str, multiple: bool, allows_other: bool) -> Vec<PathBuf> {
    let multiple_clause = if multiple { " with multiple selections allowed" } else { "" };
    let _ = allows_other;
    let prompt = title.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "try\nset chosenFiles to choose file with prompt \"{prompt}\"{multiple_clause}\nend try\nset output to \"\"\nif class of chosenFiles is list then\nrepeat with f in chosenFiles\nset output to output & POSIX path of f & linefeed\nend repeat\nelse\nset output to POSIX path of chosenFiles\nend if\nreturn output"
    );
    let Ok(output) = Command::new("/usr/bin/osascript").args(["-e", &script]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

pub fn is_running(bundle_id: &str) -> bool {
    Command::new("/usr/bin/pgrep")
        .args(["-x", bundle_id])
        .status()
        .is_ok_and(|status| status.success())
}

pub fn path_exists(path: &Path) -> bool {
    path.exists()
}

pub fn keychain_set(service: &str, account: &str, value: &str) -> Result<(), String> {
    keyring::Entry::new(service, account)
        .map_err(|_| "无法打开 macOS 钥匙串".to_string())?
        .set_password(value)
        .map_err(|_| "无法安全保存到 macOS 钥匙串".to_string())
}

pub fn keychain_get(service: &str, account: &str) -> Result<String, String> {
    keyring::Entry::new(service, account)
        .map_err(|_| "无法打开 macOS 钥匙串".to_string())?
        .get_password()
        .map_err(|_| "macOS 钥匙串中没有这条凭据".to_string())
}

pub fn keychain_delete(service: &str, account: &str) {
    if let Ok(entry) = keyring::Entry::new(service, account) {
        let _ = entry.delete_credential();
    }
}

pub fn seal(raw: &[u8]) -> Result<Vec<u8>, String> {
    use base64::Engine;
    use ring::aead;
    use ring::rand::{SecureRandom, SystemRandom};
    let random = SystemRandom::new();
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    let mut id = [0u8; 24];
    random.fill(&mut key).map_err(|_| "无法生成安全随机密钥")?;
    random.fill(&mut nonce).map_err(|_| "无法生成安全随机数")?;
    random.fill(&mut id).map_err(|_| "无法生成安全凭据引用")?;
    let id = id.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    keychain_set("Daozai.Vault.Seal", &id, &base64::engine::general_purpose::STANDARD.encode(key))?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &key).map_err(|_| "无法初始化安全存储")?,
    );
    let nonce_bytes = nonce;
    let nonce = aead::Nonce::assume_unique_for_key(nonce);
    let mut sealed = raw.to_vec();
    key.seal_in_place_append_tag(nonce, aead::Aad::from(b"Daozai"), &mut sealed)
        .map_err(|_| "无法加密登录会话")?;
    let mut blob = Vec::with_capacity(4 + id.len() + 12 + sealed.len());
    blob.extend_from_slice(b"DAZ1");
    blob.extend_from_slice(id.as_bytes());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&sealed);
    Ok(blob)
}

pub fn open(blob: &[u8]) -> Result<Vec<u8>, String> {
    use base64::Engine;
    use ring::aead;
    if blob.len() < 4 + 48 + 12 + 16 || !blob.starts_with(b"DAZ1") {
        return Err("登录会话格式无效".into());
    }
    let id = std::str::from_utf8(&blob[4..52]).map_err(|_| "登录会话引用无效")?;
    let key = keychain_get("Daozai.Vault.Seal", id)?;
    let key = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|_| "钥匙串密钥格式无效")?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &key).map_err(|_| "钥匙串密钥格式无效")?,
    );
    let nonce = aead::Nonce::try_assume_unique_for_key(&blob[52..64])
        .map_err(|_| "登录会话随机数无效")?;
    let mut sealed = blob[64..].to_vec();
    let plain = key
        .open_in_place(nonce, aead::Aad::from(b"Daozai"), &mut sealed)
        .map_err(|_| "登录会话无法解密，请重新连接")?;
    Ok(plain.to_vec())
}

#[derive(Clone, Copy, Debug)]
pub enum TrayAction {
    Settings,
    Calendar,
    TogglePet,
    ToggleAutostart,
    OpenData,
    Quit,
}

pub struct MacTray {
    _icon: TrayIcon,
    events: std::sync::mpsc::Receiver<MenuEvent>,
}

impl MacTray {
    pub fn new(ctx: eframe::egui::Context) -> Option<Self> {
        let menu = Menu::new();
        let settings = MenuItem::with_id("daozai.settings", "打开设置", true, None);
        let calendar = MenuItem::with_id("daozai.calendar", "打开日历", true, None);
        let pet = MenuItem::with_id("daozai.pet", "显示 / 隐藏吉祥物", true, None);
        let autostart = MenuItem::with_id("daozai.autostart", "切换开机启动", true, None);
        let data = MenuItem::with_id("daozai.data", "打开本地数据目录", true, None);
        let quit = MenuItem::with_id("daozai.quit", "退出岛仔", true, None);
        menu.append_items(&[
            &settings,
            &calendar,
            &pet,
            &autostart,
            &PredefinedMenuItem::separator(),
            &data,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok()?;
        let image = image::load_from_memory(include_bytes!("../../assets/daozai-logo.png"))
            .ok()?
            .thumbnail(44, 44)
            .to_rgba8();
        let icon = tray_icon::Icon::from_rgba(
            image.as_raw().clone(),
            image.width(),
            image.height(),
        )
        .ok()?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("岛仔")
            .with_icon(icon)
            .with_title("岛仔")
            .with_menu(Box::new(menu))
            .build()
            .ok()?;
        let (sender, events) = std::sync::mpsc::channel();
        MenuEvent::set_event_handler(Some(std::sync::Arc::new(move |event| {
            let _ = sender.send(event);
            ctx.request_repaint();
        })));
        Some(Self {
            _icon: icon,
            events,
        })
    }

    pub fn poll(&self) -> Vec<TrayAction> {
        let mut actions = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            let action = match event.id().0.as_str() {
                "daozai.settings" => TrayAction::Settings,
                "daozai.calendar" => TrayAction::Calendar,
                "daozai.pet" => TrayAction::TogglePet,
                "daozai.autostart" => TrayAction::ToggleAutostart,
                "daozai.data" => TrayAction::OpenData,
                "daozai.quit" => TrayAction::Quit,
                _ => continue,
            };
            actions.push(action);
        }
        actions
    }
}

fn launch_agent_path() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("HOME")?)
            .join("Library/LaunchAgents/dev.daozai.mac.plist"),
    )
}

pub fn autostart_enabled() -> bool {
    launch_agent_path().is_some_and(|path| path.is_file())
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let path = launch_agent_path().ok_or("无法找到用户 LaunchAgents 目录")?;
    if !enabled {
        let _ = Command::new("/bin/launchctl")
            .args(["bootout", &format!("gui/{}/dev.daozai.mac", unsafe { libc_geteuid() })])
            .status();
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("无法取消开机启动".into()),
        };
    }
    let executable = std::env::current_exe().map_err(|_| "无法找到岛仔应用路径")?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|_| "无法创建 LaunchAgents 目录")?;
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>dev.daozai.mac</string><key>ProgramArguments</key><array><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><false/></dict></plist>\n",
        executable.to_string_lossy().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    );
    if std::fs::write(&path, plist).is_err() {
        return Err("无法保存开机启动配置".into());
    }
    let domain = format!("gui/{uid}", uid = unsafe { libc_geteuid() });
    let status = Command::new("/bin/launchctl")
        .args(["bootstrap", &domain, &path.to_string_lossy()])
        .status();
    if status.is_ok_and(|status| status.success()) {
        Ok(())
    } else {
        Err("开机启动已保存，但 launchctl 未能立即载入；下次登录仍会生效".into())
    }
}

#[link(name = "System")]
unsafe extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

#[cfg(test)]
mod avoidance_tests {
    use super::*;

    #[test]
    fn expanded_window_keeps_mouse_events_for_all_island_buttons() {
        let region=eframe::egui::Rect::from_min_max(eframe::egui::pos2(100.0,20.0),eframe::egui::pos2(540.0,380.0));
        let hit=eframe::egui::Rect::from_min_max(eframe::egui::pos2(110.0,20.0),eframe::egui::pos2(530.0,360.0));
        assert!(pointer_should_interact(region,hit,None,false,eframe::egui::pos2(300.0,100.0)));
        assert!(!pointer_should_interact(region,hit,None,false,eframe::egui::pos2(20.0,100.0)));
        assert!(pointer_should_interact(region,hit,None,true,eframe::egui::pos2(20.0,100.0)),"展开期间必须能接收按钮点击");
    }

    #[test]
    fn island_hit_bounds_follow_horizontal_and_side_docks() {
        let canvas = Bounds {
            left: 0,
            top: 24,
            right: crate::app::CANVAS_W as i32,
            bottom: 24 + crate::app::WIN_H as i32,
        };
        let top = island_hit_bounds(canvas, crate::model::Dock::Top);
        assert_eq!(top.top, 24);
        assert_eq!(top.bottom, 24 + crate::app::BAR_H as i32);
        assert!(window_overlaps_island(
            Bounds { left: 0, top: 24, right: 1920, bottom: 1080 },
            top,
        ));
        assert!(!window_overlaps_island(
            Bounds { left: 0, top: 200, right: 1920, bottom: 1080 },
            top,
        ));

        let canvas = Bounds {
            left: 0,
            top: 0,
            right: crate::side_dock::SIDE_W as i32,
            bottom: crate::side_dock::SIDE_H as i32,
        };
        let left = island_hit_bounds(canvas, crate::model::Dock::Left);
        assert_eq!(left.left, 0);
        assert!(left.right > left.left);
        assert!(window_overlaps_island(
            Bounds { left: 0, top: 0, right: 1920, bottom: 1080 },
            left,
        ));
    }

    #[test]
    fn common_macos_browser_bundles_are_recognized() {
        for (name, path) in [
            ("Safari", "/Applications/Safari.app"),
            ("Google Chrome", "/Applications/Google Chrome.app"),
            ("Firefox", "/Applications/Firefox.app"),
            ("Arc", "/Applications/Arc.app"),
            ("Microsoft Edge", "/Applications/Microsoft Edge.app"),
        ] {
            assert!(is_browser_application(name, path), "{name}");
        }
        assert!(!is_browser_application("微信", "/Applications/WeChat.app"));
    }

    #[test]
    fn frontmost_app_only_avoids_when_enabled_matched_and_overlapping() {
        let mut settings = crate::model::Settings::default();
        let window = NativeWindow {
            pid: 42,
            layer: 0,
            name: "Google Chrome".into(),
            title: "网页".into(),
            bounds: Bounds { left: 0, top: 0, right: 1920, bottom: 1080 },
        };
        let island = Bounds { left: 700, top: 24, right: 900, bottom: 70 };
        assert_eq!(
            avoidance_for_application(
                42,
                "Google Chrome",
                "/Applications/Google Chrome.app",
                &settings,
                7,
                island,
                &[window.clone()],
            ),
            Some(false),
            "总开关关闭时不避让",
        );
        settings.browser_minimal = true;
        assert_eq!(
            avoidance_for_application(
                42,
                "Google Chrome",
                "/Applications/Google Chrome.app",
                &settings,
                7,
                island,
                &[window.clone()],
            ),
            Some(true),
            "前台浏览器遮住岛时应避让",
        );
        assert_eq!(
            avoidance_for_application(
                42,
                "Google Chrome",
                "/Applications/Google Chrome.app",
                &settings,
                42,
                island,
                &[window],
            ),
            None,
            "前台是岛仔自身时保持当前避让状态",
        );
    }

    #[test]
    #[ignore = "需要当前 macOS 桌面有可见窗口"]
    fn live_onscreen_window_metadata_is_available() {
        let candidates = avoidance_candidates().unwrap();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|candidate| !candidate.path.is_empty()));
        assert!(
            candidates.iter().any(|candidate| candidate.browser),
            "当前桌面应至少识别出一个可见浏览器：{:?}",
            candidates
                .iter()
                .map(|candidate| (&candidate.name, &candidate.path))
                .collect::<Vec<_>>()
        );
    }
}

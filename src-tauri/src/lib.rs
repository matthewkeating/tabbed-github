use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::webview::NewWindowResponse;
use tauri::{
    AppHandle, Manager, State, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;

/// The page every tab starts on.
const START_URL: &str = "https://github.com/matthewkeating?tab=repositories";
/// Shared macOS tabbing identifier: windows with the same id become native tabs.
const TABBING_IDENTIFIER: &str = "github-tab";

/// Monotonic counter used to give each tab-window a unique label.
struct TabCounter(AtomicU32);

/// True when the URL points at GitHub or one of its asset hosts, i.e. it should
/// stay inside the app rather than opening in the system browser.
fn is_github_host(url: &Url) -> bool {
    match url.host_str() {
        Some(host) => {
            host == "github.com"
                || host.ends_with(".github.com")
                || host == "githubusercontent.com"
                || host.ends_with(".githubusercontent.com")
                || host == "githubassets.com"
                || host.ends_with(".githubassets.com")
        }
        None => false,
    }
}

/// Only http(s) links are candidates for handoff to the browser; schemes like
/// `blob:`, `data:` and `about:` are page internals and must be left alone.
fn is_http(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

/// Open a URL in the user's default browser.
fn open_external(app: &AppHandle, url: &Url) {
    let _ = app.opener().open_url(url.as_str(), None::<&str>);
}

/// Next unique window label, e.g. `tab-1`, `tab-2`, ...
fn next_label(app: &AppHandle) -> String {
    let n = app.state::<TabCounter>().0.fetch_add(1, Ordering::SeqCst);
    format!("tab-{n}")
}

/// Build a new tab as its own `WebviewWindow`. All tabs share a tabbing
/// identifier so macOS groups them into a single native tab bar.
fn create_tab(app: &AppHandle, url: Url) -> tauri::Result<WebviewWindow> {
    // Capture the tab this one should attach to *before* creating it, so on
    // macOS we can fold the new window into the existing native tab group.
    #[cfg(target_os = "macos")]
    let host = focused_or_any_window(app);

    let label = next_label(app);

    let nav_app = app.clone();
    let new_win_app = app.clone();

    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title("GitHub")
        .inner_size(1200.0, 800.0)
        .tabbing_identifier(TABBING_IDENTIFIER)
        // Same-window navigation: keep GitHub in-app, send the rest to the browser.
        .on_navigation(move |url| {
            if is_github_host(url) || !is_http(url) {
                true
            } else {
                open_external(&nav_app, url);
                false
            }
        })
        // window.open / target="_blank": GitHub opens a new in-app tab, the rest
        // go to the browser. We deny the default and drive the outcome ourselves.
        .on_new_window(move |url, _features| {
            if is_github_host(&url) {
                let app = new_win_app.clone();
                // Defer window creation off the delegate callback to avoid
                // re-entering the event loop while it is still running.
                let _ = new_win_app.run_on_main_thread(move || {
                    let _ = create_tab(&app, url);
                });
            } else if is_http(&url) {
                open_external(&new_win_app, &url);
            }
            NewWindowResponse::Deny
        })
        // Keep the native tab label in sync with the page title.
        .on_document_title_changed(|window, title| {
            let _ = window.set_title(&title);
        })
        .build()?;

    #[cfg(target_os = "macos")]
    {
        enable_swipe_navigation(&window);
        // tao only sets the tabbing identifier; whether same-identifier windows
        // merge into tabs otherwise depends on the system "Prefer tabs" setting.
        // Attaching explicitly makes new tabs behave as tabs regardless.
        if let Some(host) = host {
            add_as_tab(&window, &host);
        }
    }

    Ok(window)
}

/// The tab a newly created tab should be grouped with: the focused one if any,
/// otherwise any existing tab. `None` means this is the first tab.
fn focused_or_any_window(app: &AppHandle) -> Option<WebviewWindow> {
    let windows = app.webview_windows();
    windows
        .values()
        .find(|w| w.is_focused().unwrap_or(false))
        .or_else(|| windows.values().next())
        .cloned()
}

/// Fold `new_window` into `host`'s native macOS tab group via
/// `-[NSWindow addTabbedWindow:ordered:]`. Both `with_webview` closures run
/// synchronously here because this is always called on the main thread.
#[cfg(target_os = "macos")]
fn add_as_tab(new_window: &WebviewWindow, host: &WebviewWindow) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // Read the host window's NSWindow pointer (populated synchronously).
    let host_ns = Arc::new(AtomicUsize::new(0));
    let host_ns_setter = host_ns.clone();
    let _ = host.with_webview(move |webview| {
        host_ns_setter.store(webview.ns_window() as usize, Ordering::SeqCst);
    });
    let host_ns = host_ns.load(Ordering::SeqCst);
    if host_ns == 0 {
        return;
    }

    let _ = new_window.with_webview(move |webview| unsafe {
        let new_ns = webview.ns_window() as *mut Object;
        let host_ns = host_ns as *mut Object;
        // NSWindowOrderingMode::Above == 1
        let _: () = msg_send![host_ns, addTabbedWindow: new_ns ordered: 1isize];
    });
}

/// Enable the trackpad two-finger swipe back/forward gesture on the underlying
/// WKWebView (there is no cross-platform Tauri setting for this).
#[cfg(target_os = "macos")]
fn enable_swipe_navigation(window: &WebviewWindow) {
    use objc::runtime::{Object, YES};
    use objc::{msg_send, sel, sel_impl};

    let _ = window.with_webview(|webview| unsafe {
        let wk_webview = webview.inner() as *mut Object;
        let _: () = msg_send![wk_webview, setAllowsBackForwardNavigationGestures: YES];
    });
}

/// Run `js` on whichever tab currently has focus.
fn eval_on_focused(app: &AppHandle, js: &str) {
    for webview in app.webview_windows().values() {
        if webview.is_focused().unwrap_or(false) {
            let _ = webview.eval(js);
            return;
        }
    }
}

/// Toggle the web inspector (devtools) on whichever tab currently has focus.
fn toggle_devtools_on_focused(app: &AppHandle) {
    for webview in app.webview_windows().values() {
        if webview.is_focused().unwrap_or(false) {
            if webview.is_devtools_open() {
                webview.close_devtools();
            } else {
                webview.open_devtools();
            }
            return;
        }
    }
}

/// A self-contained "URL copied" toast injected into the focused page: it
/// fades in, holds briefly, then fades out and removes itself. Uses a fixed id
/// so repeated copies replace the previous toast instead of stacking. There is
/// no DOM of our own to render into (tabs load github.com directly), so the
/// overlay lives on the GitHub page itself.
const TOAST_JS: &str = r#"(function () {
  var id = '__tabbed_github_toast__';
  var existing = document.getElementById(id);
  if (existing) existing.remove();
  var el = document.createElement('div');
  el.id = id;
  el.textContent = 'URL copied';
  el.style.cssText = [
    'position:fixed', 'top:50%', 'left:50%', 'transform:translate(-50%,-50%)',
    'z-index:2147483647', 'background:rgba(0,0,0,0.82)', 'color:#fff',
    'padding:10px 18px', 'border-radius:8px',
    'font:13px -apple-system,system-ui,sans-serif',
    'box-shadow:0 4px 14px rgba(0,0,0,0.35)', 'pointer-events:none',
    'opacity:0', 'transition:opacity 0.2s ease'
  ].join(';');
  document.body.appendChild(el);
  requestAnimationFrame(function () { el.style.opacity = '1'; });
  setTimeout(function () {
    el.style.opacity = '0';
    setTimeout(function () { el.remove(); }, 300);
  }, 1200);
})();"#;

/// Copy the focused tab's current page URL to the system clipboard, then show a
/// brief "URL copied" toast on that page. The URL is read straight from the
/// webview (no JS round-trip), so it reflects the live location after redirects
/// and client-side navigation.
fn copy_focused_url(app: &AppHandle) {
    for webview in app.webview_windows().values() {
        if webview.is_focused().unwrap_or(false) {
            if let Ok(url) = webview.url() {
                let _ = app.clipboard().write_text(url.to_string());
                let _ = webview.eval(TOAST_JS);
            }
            return;
        }
    }
}

/// User settings, persisted as `settings.json` in the app config directory
/// (e.g. `~/Library/Application Support/com.matthewkeating.tabbed-github/`).
/// Every field is optional; a missing file or field just leaves the default.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Settings {
    #[serde(default)]
    shortcuts: ShortcutSettings,
}

/// Accelerator strings (e.g. `"CmdOrCtrl+Shift+G"`) for the global hotkeys.
/// `None`/absent means that hotkey is unset and nothing is registered for it.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct ShortcutSettings {
    /// Bring the app forward (show + focus the frontmost tab).
    focus: Option<String>,
    /// Bring the app forward and open a new tab.
    focus_new_tab: Option<String>,
}

/// The live global shortcuts. For each action we keep the accelerator string as
/// the user entered it (shown in the settings UI and persisted) alongside its
/// parsed form (used by the plugin handler to match a fired hotkey). Behind a
/// `Mutex` so the settings window can update it at runtime.
#[derive(Default)]
struct GlobalShortcuts {
    focus: Option<(String, Shortcut)>,
    focus_new_tab: Option<(String, Shortcut)>,
}

/// The shortcut values exchanged with the settings window.
#[derive(serde::Serialize, serde::Deserialize)]
struct ShortcutValues {
    focus: Option<String>,
    focus_new_tab: Option<String>,
}

/// Read `settings.json` from the app config directory. Any failure (no file,
/// bad JSON) falls back to defaults rather than erroring — settings are optional.
fn load_settings(app: &AppHandle) -> Settings {
    let Ok(dir) = app.path().app_config_dir() else {
        return Settings::default();
    };
    match std::fs::read_to_string(dir.join("settings.json")) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

/// Write the shortcut values back to `settings.json`, creating the config
/// directory if it does not exist yet.
fn save_settings(app: &AppHandle, values: ShortcutValues) -> Result<(), String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let settings = Settings {
        shortcuts: ShortcutSettings {
            focus: values.focus,
            focus_new_tab: values.focus_new_tab,
        },
    };
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("settings.json"), json).map_err(|e| e.to_string())
}

/// Bring the application forward: unminimize, show, and focus the frontmost tab.
/// On macOS focusing a window also activates the app over whatever was in front.
fn bring_app_forward(app: &AppHandle) {
    if let Some(window) = focused_or_any_window(app) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Parse one optional accelerator string, treating blank as unset. Returns the
/// trimmed string paired with its parsed `Shortcut`, or a message on a bad value.
fn parse_shortcut(value: Option<String>) -> Result<Option<(String, Shortcut)>, String> {
    match value.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(s) => {
            let shortcut =
                Shortcut::from_str(&s).map_err(|_| format!("\"{s}\" is not a valid shortcut"))?;
            Ok(Some((s, shortcut)))
        }
        None => Ok(None),
    }
}

/// Parse, (re-)register, and store the given hotkeys as the live set, replacing
/// whatever was registered before. On any error (bad value, or both actions
/// bound to the same key) the previous registration and state are left intact.
fn apply_shortcuts(app: &AppHandle, values: ShortcutValues) -> Result<(), String> {
    let focus = parse_shortcut(values.focus)?;
    let focus_new_tab = parse_shortcut(values.focus_new_tab)?;

    if let (Some((_, a)), Some((_, b))) = (&focus, &focus_new_tab) {
        if a == b {
            return Err("Both actions are set to the same shortcut".to_string());
        }
    }

    let global_shortcut = app.global_shortcut();
    let _ = global_shortcut.unregister_all();
    if let Some((_, shortcut)) = &focus {
        global_shortcut
            .register(shortcut.clone())
            .map_err(|e| e.to_string())?;
    }
    if let Some((_, shortcut)) = &focus_new_tab {
        global_shortcut
            .register(shortcut.clone())
            .map_err(|e| e.to_string())?;
    }

    let state = app.state::<Mutex<GlobalShortcuts>>();
    let mut guard = state.lock().unwrap();
    guard.focus = focus;
    guard.focus_new_tab = focus_new_tab;
    Ok(())
}

/// Register the hotkeys saved in `settings.json` at startup. Errors are ignored:
/// a bad saved value simply leaves that hotkey unregistered rather than
/// preventing the app from launching.
fn register_global_shortcuts(app: &AppHandle) {
    let settings = load_settings(app);
    let _ = apply_shortcuts(
        app,
        ShortcutValues {
            focus: settings.shortcuts.focus,
            focus_new_tab: settings.shortcuts.focus_new_tab,
        },
    );
}

/// Command: the current hotkey accelerator strings, for populating the UI.
#[tauri::command]
fn get_shortcuts(state: State<'_, Mutex<GlobalShortcuts>>) -> ShortcutValues {
    let guard = state.lock().unwrap();
    ShortcutValues {
        focus: guard.focus.as_ref().map(|(s, _)| s.clone()),
        focus_new_tab: guard.focus_new_tab.as_ref().map(|(s, _)| s.clone()),
    }
}

/// Command: validate, apply, and persist new hotkeys from the settings window.
/// Returns an error string the window can display when a value is rejected; on
/// success the settings file mirrors exactly what was registered.
#[tauri::command]
fn set_shortcuts(app: AppHandle, values: ShortcutValues) -> Result<(), String> {
    apply_shortcuts(&app, values)?;
    let current = get_shortcuts(app.state::<Mutex<GlobalShortcuts>>());
    save_settings(&app, current)
}

/// Open (or focus) the settings window: a normal app window loading our own
/// `settings.html`, deliberately *not* a GitHub tab (no tabbing identifier, no
/// link routing).
fn open_settings_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Settings")
        .inner_size(460.0, 300.0)
        .resizable(false)
        .build();
}

/// Build the macOS application menu. Custom items (New Tab, Back, Forward) carry
/// accelerators and are dispatched in the menu event handler; the rest are
/// standard predefined items so copy/paste, close, minimize, etc. keep working.
fn build_menu(app: &AppHandle) -> tauri::Result<()> {
    let settings_item = MenuItemBuilder::new("Settings…")
        .id("settings")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;
    let app_menu = SubmenuBuilder::new(app, "GitHub")
        .about(None)
        .separator()
        .item(&settings_item)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let new_tab = MenuItemBuilder::new("New Tab")
        .id("new_tab")
        .accelerator("CmdOrCtrl+T")
        .build(app)?;
    let file_menu = SubmenuBuilder::new(app, "File")
        .item(&new_tab)
        .close_window()
        .build()?;

    let copy_url = MenuItemBuilder::new("Copy URL")
        .id("copy_url")
        .accelerator("CmdOrCtrl+L")
        .build(app)?;
    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .separator()
        .item(&copy_url)
        .build()?;

    let back = MenuItemBuilder::new("Back")
        .id("back")
        .accelerator("CmdOrCtrl+[")
        .build(app)?;
    let forward = MenuItemBuilder::new("Forward")
        .id("forward")
        .accelerator("CmdOrCtrl+]")
        .build(app)?;
    let reload = MenuItemBuilder::new("Reload")
        .id("reload")
        .accelerator("CmdOrCtrl+R")
        .build(app)?;
    let inspector = MenuItemBuilder::new("Toggle Web Inspector")
        .id("inspector")
        .accelerator("Alt+CmdOrCtrl+I")
        .build(app)?;
    let history_menu = SubmenuBuilder::new(app, "History")
        .item(&back)
        .item(&forward)
        .separator()
        .item(&reload)
        .item(&inspector)
        .build()?;

    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .separator()
        .fullscreen()
        .build()?;

    let menu = MenuBuilder::new(app)
        .items(&[&app_menu, &file_menu, &edit_menu, &history_menu, &window_menu])
        .build()?;

    app.set_menu(menu)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                // A configured global hotkey fired: match it to its action and,
                // because this may run off the main thread and can create windows,
                // run the work on the main thread.
                .with_handler(|app, shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    // Match the fired hotkey under the lock, then release it
                    // before dispatching work to the main thread.
                    let (is_focus, is_focus_new_tab) = {
                        let state = app.state::<Mutex<GlobalShortcuts>>();
                        let guard = state.lock().unwrap();
                        (
                            guard.focus.as_ref().map(|(_, s)| s) == Some(shortcut),
                            guard.focus_new_tab.as_ref().map(|(_, s)| s) == Some(shortcut),
                        )
                    };
                    let handle = app.clone();
                    if is_focus {
                        let _ = app.run_on_main_thread(move || bring_app_forward(&handle));
                    } else if is_focus_new_tab {
                        let _ = app.run_on_main_thread(move || {
                            bring_app_forward(&handle);
                            if let Ok(url) = START_URL.parse() {
                                let _ = create_tab(&handle, url);
                            }
                        });
                    }
                })
                .build(),
        )
        .manage(TabCounter(AtomicU32::new(1)))
        .manage(Mutex::new(GlobalShortcuts::default()))
        .invoke_handler(tauri::generate_handler![get_shortcuts, set_shortcuts])
        .on_menu_event(|app, event| match event.id().as_ref() {
            "new_tab" => {
                if let Ok(url) = START_URL.parse() {
                    let _ = create_tab(app, url);
                }
            }
            "back" => eval_on_focused(app, "history.back()"),
            "forward" => eval_on_focused(app, "history.forward()"),
            "reload" => eval_on_focused(app, "location.reload()"),
            "inspector" => toggle_devtools_on_focused(app),
            "copy_url" => copy_focused_url(app),
            "settings" => open_settings_window(app),
            _ => {}
        })
        .setup(|app| {
            build_menu(app.handle())?;
            register_global_shortcuts(app.handle());
            create_tab(app.handle(), START_URL.parse()?)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

use std::sync::atomic::{AtomicU32, Ordering};

use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::webview::NewWindowResponse;
use tauri::{
    AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
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
#[cfg(target_os = "macos")]
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

/// Build the macOS application menu. Custom items (New Tab, Back, Forward) carry
/// accelerators and are dispatched in the menu event handler; the rest are
/// standard predefined items so copy/paste, close, minimize, etc. keep working.
fn build_menu(app: &AppHandle) -> tauri::Result<()> {
    let app_menu = SubmenuBuilder::new(app, "GitHub")
        .about(None)
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
        .manage(TabCounter(AtomicU32::new(1)))
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
            _ => {}
        })
        .setup(|app| {
            build_menu(app.handle())?;
            create_tab(app.handle(), START_URL.parse()?)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

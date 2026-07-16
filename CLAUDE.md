# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A native macOS app (Tauri v2) that wraps GitHub in system tabs. There is effectively **no frontend** — each tab is its own native `WebviewWindow` that loads `github.com` directly. All application logic lives in Rust in `src-tauri/src/lib.rs`.

## Commands

```sh
pnpm install                        # install the Tauri CLI (only JS dependency)
pnpm tauri dev                      # run the app in development
pnpm tauri build --bundles app      # build → src-tauri/target/release/bundle/macos/GitHub.app
```

Rust-only iteration (faster than a full `tauri dev` cycle when you just want to check the crate compiles):

```sh
cd src-tauri && cargo build         # cargo check also works
```

There are no tests or linters configured.

## Architecture

The whole app is `src-tauri/src/lib.rs` (invoked via the thin `main.rs`). `frontend/index.html` exists only to satisfy Tauri's required `frontendDist` and is never displayed.

**Tabs are windows.** Each tab is a separate `WebviewWindow` created by `create_tab`, all sharing the `TABBING_IDENTIFIER` so macOS folds them into one native tab bar. On macOS, `create_tab` explicitly calls `add_as_tab` (raw `objc` `addTabbedWindow:ordered:`) to attach the new window to the focused one, because the tabbing identifier alone only merges windows when the system "Prefer tabs" setting is on. Tab labels are `tab-N` from the monotonic `TabCounter`.

**Link routing** is the core behavior, enforced in two places on every window:
- `on_navigation` — same-window clicks: GitHub hosts (see `is_github_host`) and non-http(s) schemes stay in-app; everything else is handed to the system browser via `open_external` and the in-app navigation is cancelled.
- `on_new_window` — `target="_blank"` / `window.open`: GitHub URLs spawn a new in-app tab, other http(s) URLs go to the browser, and the native new-window is always denied (`NewWindowResponse::Deny`) so the app drives the outcome itself. New tab creation is deferred via `run_on_main_thread` to avoid re-entering the event loop from the delegate callback.

**Menu = keyboard shortcuts.** There is no HTML UI, so all commands are `MenuItemBuilder` items with accelerators, built in `build_menu` and dispatched in the `on_menu_event` handler by id: `new_tab` (⌘T), `back` (⌘[), `forward` (⌘]), `copy_url` (⌘L). Back/Forward run `history.back()/forward()` via `eval_on_focused`; Copy URL reads `webview.url()` directly in Rust (no JS round-trip) and writes to the clipboard in `copy_focused_url`. **To add a command, add both the menu item and its `on_menu_event` arm** — there is nowhere else to wire it up.

**macOS-native touches** are done through raw `objc` `msg_send` in `#[cfg(target_os = "macos")]` blocks because Tauri exposes no cross-platform setting: `enable_swipe_navigation` (trackpad back/forward on the `WKWebView`) and `add_as_tab`.

## Gotchas

- **Permissions are capability-gated.** Any new plugin command a window calls must be allow-listed in `src-tauri/capabilities/default.json` (scoped to `tab-*` windows) or it will fail silently at runtime. Adding the plugin to `Cargo.toml` and `.plugin(...)` is not enough.
- The app starts every tab at `START_URL` (the `matthewkeating?tab=repositories` page); `setup` builds the menu and opens the first tab.
- This is macOS-focused. Tab grouping, swipe navigation, and the app-menu model assume macOS; there is no equivalent UI on other platforms.

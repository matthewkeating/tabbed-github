# Tabbed Web App

A native macOS app (Tauri v2) that wraps a website in **system tabs** — the same
native tab bar you get in Safari or Terminal, driven entirely by macOS. Press
⌘T for a new tab. Links that belong to the site stay in the app; every other
link opens in your default browser.

There is almost no frontend: each tab is its own native `WebviewWindow` that
loads the site directly, and all application logic lives in Rust in
`src-tauri/src/lib.rs`. The only real HTML page is a small settings window for
configuring global hotkeys.

## One codebase, two apps

The same source ships as two products that differ only in a small compile-time
`Site` profile (URL, in-app host list, name, bundle id, icons):

| App    | Site              | Cargo feature      | Deployed as   |
| ------ | ----------------- | ------------------ | ------------- |
| GitHub | github.com        | `github` (default) | `GitHub.app`  |
| Gemini | gemini.google.com | `gemini`           | `Gemini.app`  |

All logic and fixes live once; only per-site data varies. The active site is
chosen at compile time via a Cargo feature, defaulting to GitHub.

## Features

- **Native system tabs** — each tab is a real macOS window folded into one tab
  bar via a shared tabbing identifier, so it looks and behaves like a
  first-class Mac app.
- **Smart link routing** — in-site links (and non-http(s) schemes) stay in the
  app; `target="_blank"` / `window.open` links to the site spawn a new in-app
  tab; anything else is handed to your default browser.
- **Keyboard shortcuts** — New Tab (⌘T), Back (⌘[), Forward (⌘]), Reload (⌘R),
  Web Inspector (⌥⌘I), Copy URL (⌘L), Settings (⌘,).
- **Trackpad swipe navigation** — two-finger swipe for back/forward.
- **Global hotkeys** — optional system-wide shortcuts to bring the app forward,
  or bring it forward and open a new tab. Configured in the Settings window and
  persisted across launches (applied live, no relaunch needed).

## Prerequisites

- [Rust](https://rustup.rs)
- [pnpm](https://pnpm.io)

## Develop

```sh
pnpm install            # installs the Tauri CLI (the only JS dependency)
pnpm dev                # run the GitHub app
pnpm dev:gemini         # run the Gemini app
```

For faster Rust-only iteration (just checking the crate compiles):

```sh
cd src-tauri && cargo build   # `cargo check` also works
```

## Build

```sh
pnpm tauri build --bundles app                 # GitHub
```

To build the Gemini app, pass its Cargo feature and config overrides:

```sh
pnpm tauri build --bundles app --features gemini --config src-tauri/tauri.gemini.conf.json
```

The packaged app is written to:

```
src-tauri/target/release/bundle/macos/GitHub.app     # or Gemini.app
```

## Deploy

`pnpm deploy` builds the app, quits any running instance, copies it into
`~/Applications`, and clears the Gatekeeper quarantine attribute so unsigned
local builds open without a warning.

```sh
pnpm deploy             # build + install GitHub.app  → ~/Applications
pnpm deploy:gemini      # build + install Gemini.app  → ~/Applications
```

## Adding a third site

The build system is designed to scale. To add another site with no logic
changes:

1. Add a `#[cfg(feature = "…")]` `SITE` const in `src-tauri/src/lib.rs`.
2. Declare the Cargo feature in `src-tauri/Cargo.toml`.
3. Add a `tauri.<site>.conf.json` overriding `productName`, `identifier`, and
   `bundle.icon`.
4. Generate an icon set: `pnpm tauri icon <png> -o src-tauri/icons-<site>`.
5. Add a `case` arm in `scripts/deploy.sh`.

## Known issues

- **App-menu name in dev/debug launches.** The bold macOS application-menu title
  (e.g. "Gemini" vs "GitHub") is taken from the bundle's `productName`, which is
  applied by the Tauri CLI's `--config` merge. A raw `cargo`/lldb launch of the
  unbundled binary (VS Code Run & Debug, or `cargo run`) skips that merge, so the
  title falls back to the base config's name. The `.vscode/launch.json` Gemini
  configs work around this by setting `TAURI_CONFIG` in their `env`. **This only
  affects the dev/debug launch — packaged `.app` bundles are always correct**,
  because the deploy/build commands go through the `--config` merge. (The
  individual menu *items* — About/Hide/Quit — are named from `SITE.name` in code,
  so they are correct in every build.)

## Notes

- macOS only. Tab grouping, swipe navigation, and the app-menu model all assume
  macOS.
- There are no tests or linters configured.

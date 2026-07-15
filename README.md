# GitHub

A native macOS app that puts GitHub in system tabs (⌘T for a new tab).
Links to GitHub stay in the app; everything else opens in your default browser.

## Prerequisites

[Rust](https://rustup.rs) and [pnpm](https://pnpm.io).

## Develop

```sh
pnpm install
pnpm tauri dev
```

## Build

```sh
pnpm tauri build --bundles app
```

The packaged app is written to:

```
src-tauri/target/release/bundle/macos/GitHub.app
```

---
paths:
  - "app/**"
triggers:
  - changing a Yard.app screen, widget or menu item
  - adding an action Yard.app runs against yardd, the yard CLI or Terminal
  - bumping GPUI or adding a dependency to the app workspace
---

# Yard.app (app/)

- **Boundaries.** `yard-app-model` holds the decisions (hints, health, settle plans, jobs, onboarding)
  with no GPUI, tokio, files or clock, so each rule is a plain unit test; `yard-app` holds the GPUI
  entities, platform glue, widgets and screens. Only `yard-app` imports `gpui`, `tray-icon`, `muda`
  or `objc2*` (rust-workspace rule).
- **The app writes nothing of Yard's.** Every change goes through yardd's API (`yard-client`), the
  `yard` CLI (`start`, `stop`, `trust`, `untrust`: `AppEnv::run_cli`) or Terminal (`sudo`, brew).
  It keeps only its own settings, in its defaults.
- **Hints are an allowlist.** `HintAction::parse` maps a known hint to an action and anything else
  to Copy. The Terminal hand-off rebuilds the command from the parsed fields and single-quotes every
  argument (`terminal_script`); a hint's text never reaches a shell. Files and folders open through
  `Opener` (one `O_NOFOLLOW` open, nothing executable or launchable, a package revealed), never
  `open` on a path a hint named.
- **Every widget reads aloud.** Each control has an `A11y` id, role and label; text goes through
  `widgets::text` (a `Role::Label` with only `aria_label` gives VoiceOver nothing); a control is
  pressed with `on_press` (GPUI turns an accessibility press into a click at the centre, which
  misses a control scrolled out of view); a greyed-out control is `Disabled` with its reason as
  the description; a table column always has a title, and a table stays an AccessKit `List` of
  `ListItem`s (a `Table` has no rows on macOS, and VoiceOver reads it as empty). An overlay (a
  sheet, a picker's popup) `occlude`s, or a click on it also presses what is under it. A sheet
  (`Form`) takes Enter and Escape, and a picker's popup closes on a click outside it. A button
  whose label does not say what it does (Copy) gets a `description`, and a tab's badge is read
  as its description.
- **Every failure reaches the screen.** An action records its error under an `ActionId`; the
  screen that owns it shows it beside the control, and `window::main::shown_on` says which ids each
  screen shows. Whatever the open screen does not show appears above its content, so a new screen
  that shows its own errors adds them to `shown_on`. Input a sheet cannot send (a port that is not
  one) is recorded the same way and shown in the sheet (`Form::error`), never dropped.
- **Yard's files by `yard_core::Paths`.** A file of the data root the app opens comes from
  `AppEnv::paths()`, never a path spelled out by hand.
- **Drawing never fetches.** A screen reads `AppState`; what is fetched comes from the keys
  `visible_keys` returns for the screen, the menu's (`MENU`, plus each installed PHP's extensions),
  the events that invalidate them, and the actions that change them. A refresh from `render` loops
  as soon as the fetch fails.
- **Updating and uninstalling go through the CLI** (M7 spec §6.3, §10). The app runs `yard
  self-update --check` at launch and daily, shows the item `yard_app_model::update::item` gives,
  and installs with `yard self-update --yes` (on its own only with Settings' switch on and an
  install.sh bundle), then relaunches; a brew install gets `brew upgrade --cask yard` copied,
  never run. **Uninstall Yard…** hands `yard uninstall` to Terminal and quits. Start yardd runs
  `yard start --yes`: the click is the go-ahead to install nginx and dnsmasq.
- **One window per kind.** The main window, Settings and onboarding keep their handle in a global
  and raise the open window rather than open another (`window::is_open`, `window::raise`).
- **Check a UI change on the real app.** After the tests, `app/scripts/checklist.sh` and
  `app/scripts/checklist-installs.sh` (scratch roots, a fake brew; never edit a script while it
  runs). `app/scripts/checklist-real.sh` touches the real yardd: only with the maintainer's approval. System
  Events (`ax.js`) can give a repeated structure's later items the first one's text and frame;
  `swift app/scripts/axdump.swift <pid>` reads the AX API as VoiceOver does.
- **GPUI is pinned by `rev`** to a Zed stable release commit, `gpui` and `gpui_platform` only.
  A bump: read the release's GPUI changes, update the `rev`, review the `app/Cargo.lock` diff, run
  `cargo deny check`, build, run the tests and both checklists. Never a crate from Zed outside
  GPUI's tree (`ui`, `theme`, `editor` are GPL), and no `gpui-component` (a second GPUI).

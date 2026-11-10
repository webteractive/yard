# Yard.app visual design Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Yard.app look like a small native Mac utility and fix glitches G1–G21, then replace
the app icon and the menu bar icon with the D3 logo.

**Architecture:** First the tokens: `widgets/theme.rs` gets colours, spacing, radii, sizes and a
`Typography` trait. Then each widget is rebuilt on them, then each screen. Behaviour, ids, roles and
labels stay the same. Where the checklist presses something that moves, such as a row action that
moves into a detail pane or an inline field that moves into a sheet, the checklist scripts change in
the same task. The logo is an SVG source. A script renders it to `AppIcon.icns`, and the tray icon
is drawn in code from the same geometry.

**Tech Stack:** Rust 1.98.1, GPUI (pinned rev), tray-icon/muda, bash + JXA checklist scripts, Swift
(for rendering the icon), `iconutil`.

**Spec:** `docs/superpowers/specs/2026-11-09-yard-app-visual-design.md` (approved 2026-11-10). It
has the token values (§3), the screen sketches (§4), the change order (§5) and the logo (§6). Read it
first. This plan only adds what the spec leaves open: signatures, tests, and which checklist lines
each task touches.

## Global Constraints

- Work in `app/` with its toolchain. Each task ends with `cargo fmt --check`, `cargo clippy
  --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked`, all
  run in `app/`.
- Every `A11y` id, role and label stays the same, unless a task names the change and updates
  `app/scripts/checklist.sh`, `checklist-installs.sh` or `ax.js` in the same commit.
- `.hydra/rules/app.md` holds throughout:
  - text goes through `widgets::text`, and controls are pressed with `on_press`;
  - a greyed-out control is `Disabled` with its reason;
  - tables stay `List`/`ListItem`, overlays `occlude`, and sheets take Enter and Escape;
  - every failure reaches the screen.
- Only `yard-app` imports `gpui`. No new crates (no `gpui-component`, nothing from Zed outside
  GPUI). The PNG for the app icon is embedded with `include_bytes!` and drawn with GPUI's `img`.
- Colours come only from `Theme` tokens. A hex literal outside `theme.rs` (and outside the tray
  icon's black-on-clear) is a defect.
- Commits: one per task, with a lowercase prose subject, no type prefix and no trailers. Work stays
  on main (AGENTS.md: until the first release, there are no feature branches). Never push.
- Docs move with the code. The spec's Status records each step as built. Touch `AGENTS.md` and
  `app.md` only where a rule changes (Task 18 changes the `ax.js icon` wording).
- After Tasks 9, 10, 13, 14 and 19, run `app/scripts/checklist.sh` and
  `app/scripts/checklist-installs.sh` (scratch roots and a fake brew; never edit a script while it
  runs), plus `scripts/bundle.sh` first. Never run `checklist-real.sh` without the maintainer's
  approval.

## Review Focus

1. **Dark mode.** Every new surface (pill, group box, issues strip, sheet labels, toolbar) must keep
   its contrast. The contrast test only covers token pairs, so look at a dark screenshot from the
   checklist run as well.
2. **Long values.** A 60-character site URL, a 200-character Doctor message or an 80-character log
   line must truncate or wrap inside its cell or pane, never widen the window (G2, G4).
3. **Narrow window.** At 760×480 (the new minimum), the Sites detail falls below the list, and the
   sidebar keeps its 200 pt.
4. **Errors stay visible.** After the issues strip and Dismiss, a failed action on another screen
   still shows until dismissed or until it succeeds, and dismissing one error clears only that one.
5. **Selection-dependent actions.** Once row actions move into detail panes, an instance or version
   that disappears while selected (removed or uninstalled) must not leave a pane acting on nothing.
   The selection falls back to none.

---

### Task 1: Window titles and minimum size (spec step 1, G9)

**Files:** `app/yard-app/src/window/main.rs`, `onboarding.rs`, `settings_window.rs`;
`app/scripts/ax.js` and `checklist*.sh`, only if they match windows by an empty title.

**Interfaces:**
- Produces: `fn window_options(bounds: Bounds<Pixels>) -> WindowOptions` in each of the three modules
  (private, `pub(crate)` for tests).

- [ ] Read how `ax.js` (`windows`, `isMain`) and the scripts identify the onboarding and Settings
  windows. Record whether a title change breaks a match.
- [ ] Write the failing test `window_titles_name_each_window`. It checks the three `window_options`:
  `titlebar.title` is `Some("Yard")`, `Some("Set up Yard")` and `Some("Yard Settings")`, and
  `window_min_size` is `Some(size(px(760.), px(480.)))` for the main window, while the other two keep
  their fixed size. Run it and expect a FAIL.
- [ ] Implement it, update any script matches found in the first step, and run the suite. Then commit.

### Task 2: Containment and unstretched controls (spec steps 2–3, G3, G4, G7)

**Files:** `window/main.rs` (sidebar `flex_shrink_0`; content column and `#content` `min_w(px(0.))`),
`screens/doctor.rs` (message `min_w(0)`, wraps), `widgets/mod.rs` (new `bar()`), `widgets/button.rs`
(`flex_none`), and the call sites `screens/doctor.rs:63`, `screens/node.rs:89`, `activity.rs:101`
and `sites/detail.rs:196`.

**Interfaces:** Produces `pub fn bar() -> Div` (a flex row with `items_center`, `gap(space::SM)` once
Task 5 lands, `px(8.)` until then, and `flex_wrap`).

- [ ] Failing gpui tests, using `debug_selector` and `vcx.debug_bounds`:
  - `sidebar_keeps_its_width_beside_wide_content`: a Doctor check with a 300-character message, in a
    900 pt window. The sidebar's width equals its set width, and `#content`'s right edge is at or
    inside the window's.
  - `a_lone_button_sizes_to_its_label`: the Run doctor button is narrower than half the content.
- [ ] Implement, run the suite, and commit.

### Task 3: Text field width and clipping (spec step 4, G5, G21)

**Files:** `widgets/text_field.rs`; call sites `screens/node.rs`, `screens/tunnels.rs`,
`screens/ports.rs`, `screens/logs.rs`, `settings_window.rs`.

**Interfaces:** Produces `TextField::width(&mut self, w: Pixels)` (it stores the width; a field
made by `new` defaults to `px(180.)`) and `TextField::full_width(&mut self)` for Settings. Text and
caret are centred vertically on one line-height constant shared with the box height. Paint runs
inside `window.with_content_mask(Some(ContentMask { bounds }), …)`.

- [ ] Failing tests:
  - `a_field_in_a_row_keeps_its_width`: a field in a `bar()` beside a button has bounds width 180.
  - `a_long_value_stays_inside_the_field`: the value is 200 `x`s, and the button after the field
    starts at or after the field's right edge. Put a `debug_selector` on both.
- [ ] Implement, set the widths at each call site (Settings gets `full_width`), run the suite, and
  commit.

### Task 4: Onboarding scrolls, note under the title, logo in the heading (spec step 5, §6, G8)

**Files:** `onboarding.rs`; `app/yard-app/macos/AppIcon-112.png` (made in Task 18, so this task adds
a placeholder of the current icon at 112 px, and Task 18 replaces it).

- [ ] Failing tests:
  - `onboarding_scrolls_to_check_again`: in the 620×640 window with every step pending, the root has
    `overflow_y_scroll`. Assert that scrolling the root's `ScrollHandle` to its end brings
    `onboarding:check` inside the window bounds.
  - `the_helper_note_sits_under_its_title`: the note's left edge equals the step title's.
- [ ] Implement:
  - the root scrolls;
  - the note becomes a child of the title column, and the row gets `min_w(0)`;
  - the heading row is `img(Arc<Image>)` at 56 pt from `include_bytes!("../macos/AppIcon-112.png")`
    plus the title, with the image marked `aria_hidden` or given the label "Yard".
- [ ] Run the suite and commit.

### Task 5: Theme tokens and the contrast test (spec step 6, §3.4, §3.6)

**Files:** `widgets/theme.rs`.

**Interfaces:** Produces, exactly as spec §3.6:
- `Theme { pub c: Colors }` plus the alias fields, which become methods `background()`, `surface()`
  and the rest. **Ruling for the executor:** keep the aliases as fields copied from the matching
  tokens, so no call site changes in this task.
- `Colors`, which holds every §3.4 token, with alpha tokens as `Hsla` with alpha.
- `Tone { Success, Warning, Danger, Neutral, Accent }`.
- `Theme::state(&self, Tone) -> (Hsla, Hsla, Hsla)`, returning dot, text and wash.
- `Theme::wash(color, alpha)`. Today's one-argument callers pass `0.14`.
- The modules `space`, `radius` and `size`, which hold the §3.3 values, with `size::SIDEBAR = 200`,
  `FIELD = 180`, `SHEET = 460` and `PICKER_MIN = 120`.
- `trait Typography: Styled`, which gives the §3.2 sizes, line heights and weights. Mono uses
  `font_family("SF Mono")`; the Menlo fallback is a ruling: GPUI falls back on its own, so if
  SF Mono is missing it shows the system mono.

- [ ] Failing tests:
  - `text_tokens_meet_contrast`: WCAG relative luminance ≥ 4.5 for `text`, `text_secondary`, each
    `*_text` and `on_accent`/`accent_fill` against `content_bg`, in both themes. Check `*_text` over
    its wash at .15 composited on `content_bg` as well.
  - `light_and_dark_tokens_match_the_spec`: spot-check `accent`, `danger_fill` and `window_bg` in
    each theme against the hex values in §3.4.
- [ ] Implement. Run it, and if a spec pair fails 4.5, the spec value is wrong: adjust the
  lightness the least amount that passes, record a ruling, and update §3.4. Then run the suite and
  commit.

### Task 6: Buttons and the sidebar badge (spec step 7, G11, G13, G16)

**Files:** `widgets/button.rs`, `window/main.rs:322-333`.

**Interfaces:** Produces `Button::small(self) -> Self`. `ButtonStyle` keeps its three names. Heights
come from `size::CONTROL` and `size::CONTROL_SMALL`. Destructive is the Normal look with
`danger_text` as the label. Hover and press use `.hover`/`.active` with the background darkened by
4% and 8%. Disabled is opacity .45, with no pointer cursor.

- [ ] Failing tests:
  - `buttons_have_fixed_heights`: the Normal and the small button have bounds heights of 24 and 20.
  - `destructive_is_not_filled`: pull the style decision out into `fn colors(style, &Theme) -> (bg,
    fg)`. It returns `bg == control_bg` and `fg == danger_text` for Destructive.
  - `the_badge_is_centred_on_its_label`: the badge's vertical centre is within 1 pt of the nav
    item's.
- [ ] Implement, run the suite, and commit.

### Task 7: Type scale and line heights (spec step 8, G13)

**Files:** `widgets/*`, `screens/mod.rs` (`heading`), every screen module, and the sidebar items
(`size::SIDEBAR_ITEM`).

- [ ] Failing test `sidebar_items_are_28_high`: `nav:Overview` has a bounds height of 28.
- [ ] Apply `Typography` everywhere that uses `text_sm`, `text_xs` or the default size. Then
  `grep -rn "text_sm\|text_xs\|text_base" app/yard-app/src` must print nothing.
- [ ] Run the suite and commit.

### Task 8: Table columns, actions and empty states (spec step 9, G1, G2, G18)

**Files:** `widgets/table.rs` and the call sites: `overview.rs`, `sites/mod.rs`,
`services/table.rs`, `screens/php.rs`, `node.rs`, `tunnels.rs`, `ports.rs`, `sources.rs`.

**Interfaces:** Produces:
- `pub struct Column { pub title: SharedString, pub width: Width, pub align: Align }`;
- `pub enum Width { Fixed(Pixels), Flex(f32) }` and `pub enum Align { Start, End }`;
- `Column::flex(title)` and `Column::fixed(title, px)`;
- `Table::new(id, Vec<Column>, Vec<Row>)`, plus `.actions(width: Pixels)` and
  `.empty(text, Option<AnyElement>)`;
- `Row::cell(...)`, which stays `Vec<SharedString>`, where an empty string renders `—`, with
  `Row::extra` unchanged and drawn inside the reserved actions width;
- `pub const EMPTY: &str = "—"`.

The a11y label stays the column titles joined.

- [ ] Failing tests:
  - `columns_line_up_with_and_without_actions`: one row with two buttons and one with none, under
    `actions(px(120.))`. Each column's cell x equals the header's x for every row.
  - `a_long_cell_truncates`: a 300-character URL cell is no wider than its column.
  - `an_empty_table_says_so`: no rows draws the empty text, and the `List` keeps its label.
  - `an_empty_value_reads_as_a_dash`.
- [ ] Keep `a_table_is_a_list…` and `table_keeps_row_ids_across_refreshes` unchanged, and expect them
  to pass.
- [ ] Migrate every call site. Pick widths from the §4 sketches, and give each table with row actions
  `.actions(..)`. The empty texts come from §3.5: Node, "No Node versions installed."; Tunnels, "No
  tunnels. Share a site to get a public URL."; and plain "No sites linked.", "No services." and so on
  for the rest.
- [ ] Run the suite and commit.

### Task 9: Status pills (spec step 10, G17)

**Files:** new `widgets/pill.rs`; the same call sites as Task 8 for state cells; `activity.rs:73-85`;
onboarding step states.

**Interfaces:** Produces:
- `pub struct Pill { id, word, tone }`, made by `Pill::new(id, word, Tone)`;
- `pub fn tone_for(word: &str) -> Tone`, with the §3.5 mapping, prefix-matched so that "waiting for
  …" is Warning, and unknown words Neutral;
- `Row::cell_element(i, AnyElement)`, or the ruling's equivalent, so that a cell can hold a pill
  while its a11y text stays the word.

- [ ] Failing tests:
  - `a_pill_reads_its_word`: the pill's text element has the word as its text, through
    `widgets::text`.
  - `states_map_to_tones`: running, serving, ready and done are Success; needs setup and waiting for
    x are Warning; failed and in use are Danger; stopped, sleeping and cancelled are Neutral.
  - `a_job_status_is_centred_on_its_title`: the vertical centres are within 1 pt.
- [ ] Activity's "waiting for php-install-1" shows the job's title ("waiting for Install PHP 7.1")
  when that job is known, and the id otherwise. Write the failing test
  `waiting_names_the_job_not_its_id` first.
- [ ] Implement, run the suite, commit, and then run the bundle and both checklists.

### Task 10: The issues strip and Dismiss (spec step 11, G6)

**Files:** `window/main.rs:242` (`elsewhere_errors` moves into `#content`, at the top of the scroll
area), `widgets/error_view.rs` (inline style: the 3 px danger bar, a wash at .10, small buttons; a
compact line variant `ErrorView::compact()` with Dismiss), `runner.rs` (`pub fn dismiss(&mut self,
id: &ActionId, cx)`, which calls `errors.clear` and `cx.notify()`).

**Interfaces:**
- Dismiss has the id `dismiss:<action id>`, the label "Dismiss", and the description `dismisses
  <message>`.
- With three or more errors, the strip shows two plus a "N more…" button (`issues:more`), which
  expands the rest.

- [ ] Failing tests:
  - `a_dismissed_error_leaves_the_strip`: record two errors elsewhere and press `dismiss:<first>`.
    `elsewhere_error_ids` then holds only the second.
  - `three_errors_show_two_and_more`.
  - `the_strip_scrolls_with_the_content`: the strip's element is a descendant of `#content`. Assert
    through bounds: the strip moves when `#content` scrolls.
- [ ] Keep `an_error_shows_on_whatever_screen_is_open` unchanged.
- [ ] Implement, run the suite, commit, and then run the bundle and both checklists.

### Task 11: Switch, picker, log view, sheets (spec step 12, G10, G15, G19, G20)

**Files:** `widgets/toggle.rs`, `widgets/picker.rs`, `widgets/log_view.rs`, `widgets/form.rs`; the
sheet call sites in `sites/forms.rs` and `services/forms.rs`.

**Interfaces:**
- Picker: the popup is positioned from the button's bounds, measured with `on_children_prepainted`
  or a `canvas` that records them in the picker's state. It no longer uses `mt(26)`.
- `Form::field_labeled(label: SharedString, element)`, where every existing `.field` call passes
  the field's own a11y label. The sheet is anchored at the top of the content (`top(px(0.))`), 460
  wide.
- LogView keeps the id `<id>:copy` for Copy all, now a `Button::small`, plus a header with the line
  count and the empty text "No log lines yet".

- [ ] Failing tests:
  - `the_picker_popup_starts_under_its_button`: popup top − button bottom is between 0 and 6 pt.
  - `the_off_switch_track_is_visible`: the off track colour has a contrast ≥ 1.5 against `surface`
    in light mode (pull it out as `fn track(on, &Theme)`).
  - `an_empty_log_says_so`.
  - `a_long_log_line_wraps_inside_the_view`.
  - `sheet_fields_show_their_labels`: for a sheet field with a11y label "Port", a text element
    "Port" is drawn above it.
- [ ] Implement, run the suite, and commit. Task 13 runs `checklist-installs.sh`, which covers the
  sheets.

### Task 12: Group boxes, Overview, site detail, Settings (spec step 13, G12, G14)

**Files:** new `widgets/group.rs`, `overview.rs:35-68`, `sites/detail.rs:171-380`,
`settings_window.rs`.

**Interfaces:**
- `pub struct Group`, made by `Group::new(id, title: Option<SharedString>)`, with
  `.row(label, value: AnyElement)`, `.row_below(label, value)` for long values,
  `.footer(AnyElement)`, and `ROW_HEIGHT = 32`.
- Overview: Start yardd shows only while offline, and Restart and Stop only while connected, all
  read from the connection state. "Open in…" on the site detail is a `Picker`-style menu with the
  items `site:<n>:open-editor`, `:open-finder` and `:open-terminal` (the existing ids). Each is a
  `MenuItem` that keeps its label.

- [ ] Failing tests:
  - `start_is_offered_only_while_offline`, for both states.
  - `overview_fact_rows_are_one_height`, checking that the CA row (which has a button) is the same
    height as Helper's.
  - `open_in_lists_editor_finder_terminal`: open the menu and read the three ids and labels.
- [ ] Implement. Update `checklist.sh` wherever it presses `open-editor`, `open-finder` or
  `open-terminal` directly, so that it opens "Open in…" first: run `grep -n "open-" app/scripts/*`.
  Run the suite and commit.

### Task 13: Row actions into detail panes (spec step 14)

**Files:** `services/table.rs`, new `services/detail.rs`, `screens/php.rs`, `app/scripts/checklist.sh`,
`checklist-installs.sh`.

**Interfaces:**
- Services uses `Screen::Services(Option<name>)` as its selection. Rows keep at most one action
  (Stop, Start, or Logs for emailulator). The detail group holds Port…, Sleep after…, Restart, Logs,
  Copy .env, Open console and Remove…, keeping today's ids (`service:<n>:port`, `:sleep` and the
  rest).
- PHP gets a selected version in the screen's view state. Its group box holds Make default, the
  Xdebug switch, Open 99-overrides.ini and Uninstall… (`php:<v>:uninstall`), plus the extensions
  table and php.ini, with the existing ids.
- A selection whose instance or version no longer exists becomes `None` (Review Focus 5).

- [ ] Failing tests:
  - `service_actions_live_in_the_detail`: with redis selected, `service:redis:port` is drawn. With
    nothing selected it is not, and the row shows at most one button.
  - `a_removed_selection_falls_back_to_none`, for both Services and PHP.
  - `php_actions_live_in_the_selected_version`.
- [ ] Update both checklists: before each press of `service:<n>:{port,sleep,remove,…}` or
  `php:<v>:{uninstall,default,…}`, the script selects that row (`ax.js` row press on
  `services:row:<n>` or `php:row:<v>`). Run `grep -n "service:\|php:" app/scripts/checklist*.sh`
  to find them.
- [ ] Run the suite and commit. Then run the bundle and both checklists, which must pass at the
  previous counts (72/72 and 14/14).

### Task 14: Sheets for inline forms (spec step 15)

**Files:** `screens/tunnels.rs`, `screens/ports.rs`, `screens/node.rs`, new `screens/forms.rs`,
checklists.

**Interfaces:**
- Tunnels gets Share… (`tunnels:share`), which opens a sheet with the site picker, Quick or Named,
  and the host, and Add domain… (`tunnels:add-domain`).
- Ports gets Range… (`ports:range`). Node gets Install… (`node:install`).
- The field ids stay the same, and the checklist opens the sheet first.

- [ ] Failing test `share_opens_a_sheet_with_the_old_fields`: press `tunnels:share`, and the old
  field ids are drawn inside `sheet:share`. Write the same test for range, domain and node install.
- [ ] Implement, update the checklists, run the suite, commit, and run the bundle and both
  checklists.

### Task 15: Sidebar groups and order (spec step 16, §3.1)

**Files:** `window/main.rs:122-134` (`PAGES`) and `:301-350`.

- [ ] Failing test `the_sidebar_is_grouped`. It reads the sidebar's child texts in order: Yard,
  Overview, Activity, Serve, Sites, Services, Tunnels, Runtimes, PHP, Node, System, Ports, Logs,
  Doctor, Sources. Group labels have no `Tab` role.
- [ ] Implement, run the suite, and commit.

### Task 16: Transparent title bar and toolbar strip (spec step 17)

**Files:** `window/main.rs` and `screens/mod.rs` (`heading` becomes `toolbar(title, actions:
Vec<AnyElement>)`), plus every screen's header row.

**Interfaces:** `TitlebarOptions { title: Some("Yard"), appears_transparent: true,
traffic_light_position: Some(point(px(16.), px(18.))) }`. The toolbar is `size::TOOLBAR` high, and
its empty space calls `window.start_window_move()` on mouse down. The heading text keeps the id
`heading`.

- [ ] Failing tests:
  - `each_screen_has_its_title_in_the_toolbar`: the `heading` text equals the page name for every
    page.
  - `primary_actions_sit_in_the_toolbar`: on Sites, `sites:link`'s top is within the toolbar's
    bounds.
- [ ] Implement, run the suite, and commit.

### Task 17: Blurred sidebar (spec step 18, optional)

- [ ] Set `WindowBackgroundAppearance::Blurred` and a translucent `sidebar_bg`, and check the
  checklist screenshots in light and dark. Keep it only if the content-pane text stays opaque. If not,
  revert it and record a ruling. No test, since this is appearance only. Commit only if kept.

### Task 18: The D3 logo: app icon and menu bar icon (spec §6)

**Files:**
- new `app/yard-app/macos/AppIcon.svg` (from the canvas's `Logo.svg`);
- new `app/scripts/make-icon.sh`, which renders the SVG to the iconset PNGs and `AppIcon-112.png`
  with a Swift snippet (`NSImage(contentsOf:)` draws SVG), then runs `iconutil -c icns`;
- `app/yard-app/macos/AppIcon.icns` and `AppIcon-112.png`, regenerated;
- new `app/yard-app/src/menubar/icon.rs`, which turns `menubar.rs` into `menubar/mod.rs`;
- `app/scripts/ax.js` (`iconShape`), and `checklist.sh` only if a state word changes.

**Interfaces:**
- `pub(crate) fn icon_alpha(health: &Health) -> Vec<u8>` returns 36×36 coverage values. Each pixel
  is a 4×4 supersample of the §6 shapes:
  - circles test the distance;
  - strokes test the distance to a segment ≤ width/2;
  - the triangle is a point-in-triangle test;
  - offline is the normal shape × 0.38.
- `icon()` maps the alpha into black RGBA and stays the caller's entry point.

- [ ] Failing test `the_icon_tells_each_health_apart`. It samples pixels by (x, y):

  | State | body (7,18) | route (18,21) | corner (24,24) | badge centre (28,28) | warn foot (24,33) |
  |---|---|---|---|---|---|
  | ok | 255 | 0 | 255 | — | — |
  | busy | 255 | — | 0 | 0 | — |
  | warning | 255 | — | 0 | 255 | 255 |
  | error | 255 | — | 0 | 255 | 0 |
  | offline | 97 ± 3 | — | 97 ± 3 | — | — |

  The test fails today because the circle has no route cut.
- [ ] Implement it and run the test.
- [ ] Update `ax.js iconShape`. It samples, in pt offsets from the status item's centre: body
  (−5.25, +0.25), corner (+3.25, +3.25), badge centre (+5.25, +5.25) and warn foot (+3.25, +7.75). It
  classifies a point by its colour distance from the bar's background: on means more than 1.5 (of 3)
  and faint means 0.3 to 1.5, so offline is a faint body. It returns ok, busy, warning, error or
  offline, and the comment states the geometry. The checklist still asks for `ok` and `offline`.
- [ ] Write `make-icon.sh`, run it, and check the outputs: `iconutil` exits 0, `sips -g pixelWidth`
  on the 1024 PNG prints 1024, and `AppIcon-112.png` is 112. Then `scripts/bundle.sh` and
  `open app/target/Yard.app --args --root /tmp/yard-dev` (against a scratch yardd): the Dock and
  Finder show the D3 tile.
- [ ] Update the spec's Status, and AGENTS.md's `checklist.sh` line if it describes the icon. Run the
  suite and commit.

### Task 19: Final checks and docs

- [ ] Run `scripts/bundle.sh`, `app/scripts/checklist.sh` (72/72 or the new count with a reason) and
  `checklist-installs.sh` (14/14). Look at the light and dark screenshots of every screen against
  §4. Every G1–G21 is gone, or else ledgered as a ruling.
- [ ] Docs: the spec's Status ("built", with the date and commits), the AGENTS.md M6 paragraph (one
  line: the visual design is built, see the spec), and `.hydra/rules/app.md` (add "colours, spacing
  and type come from `widgets::theme` tokens; no hex outside `theme.rs`"). Grep the docs for `nav:`,
  `heading`, `mt(26)`, `text_sm` and `ax.js icon`. Remove `app/target` afterwards if disk matters
  (AGENTS.md).
- [ ] Final whole-branch review on the most capable model, one fix pass, then commit.

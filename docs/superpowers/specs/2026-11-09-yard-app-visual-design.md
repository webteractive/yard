# Yard.app: visual design and glitch survey

Status: proposal of 2026-11-09, approved 2026-11-10 (the maintainer: implement the design, then the
release); planned in `docs/superpowers/plans/2026-11-10-yard-app-visual-design.md` and built on
2026-11-10 (45aa1b5..a3a3d27 and the commit after it), steps 1–17 of §5 and the logo of §6. Not
built: step 18, the blurred sidebar (it needs a look at both appearances first); "Open in…" as one
menu on the site detail (the editor, Finder and Terminal stay three buttons); Ports' controls stay
in its page rather than the toolbar. Several text colours moved a little to pass 4.5:1 where pills and group labels sit (§3.4). The unit tests cover the layout fixes; `checklist.sh` and
`checklist-installs.sh` were updated for the moved controls but not yet run on this build. It changes how Yard.app looks and lays out,
not what it does: every screen, action, error path, sheet key and accessibility rule in the M6 design
(`2026-11-05-yard-m6-app-design.md` §8) and `.hydra/rules/app.md` still holds. The menu bar menu stays
a native `NSMenu` (muda) and its content is out of scope; its icon, and the app icon, change to the
logo in §6.

Screenshots cited below come from the checklist runs' temp directories:

- `latest/` = `yard-check-20261109-115440/` (light, scratch yardd; every screen)
- `installs/` = `yard-check-installs-20261109-113156/` (light; Activity jobs, a live tunnel)
- `real/` = `yard-check-real-20261107-010951/` (dark, the real yardd: overview, doctor, ports)
- `dark/` = `yard-check-20261106-195109/` (dark; services, onboarding)

Every run since 2026-11-06 shows the same layout, so the glitches below are not one-off.

## 1. What looks basic today

**Everywhere.**

- The type is web-sized, not Mac-sized. Body text is `text_sm` (14 px) or the 16 px default. GPUI's
  default line height is φ (1.618), so a 14 px line is about 23 px tall. A native Mac app uses 13 px on
  about 16 px. Rows come out 43 pt tall (`latest/php.png`, `latest/ports.png`) where Finder or
  Activity Monitor would use 24–28.
- Buttons are 32 pt tall, bordered and white, and every action is one, so screens are walls of
  buttons: 22 on PHP, 11 of them solid red (`latest/php.png`), and 8 on one Services row
  (`latest/services.png`).
- Destructive actions are filled red everywhere. On a Mac, red is kept for the confirming alert,
  which the app already gets from `Window::prompt`.
- No state has a colour or shape. "running", "serving", "needs setup", "free", "listening", "done"
  and "cancelled" are all the same plain text (`latest/overview.png`, `latest/site-detail.png`,
  `installs/activity-p2.png`).
- The title bar is an empty white or black band with no title (every screenshot). The sidebar is a
  bare list of words with a green wash for the selected item, and no grouping.
- No list has an empty state. Node and Tunnels draw a header row over nothing
  (`latest/node.png`, `latest/tunnels.png`).
- Errors are large pink slabs stacked above the content. They are the loudest thing on most
  screens, even when they are old (`latest/node.png` to `latest/sources.png`).

**Overview** (`latest/overview.png`, `real/overview.png`): the facts are a loose key/value list.
Start, Restart, Set up and Stop all carry the same weight, and Start is enabled while yardd runs.
The units table spends a 32 pt Restart button on every row.

**Sites** (`latest/site-detail.png`): the URL runs into the PHP column. The detail pane is a stack of
sentences ("PHP 8.4.27 — the default", "Docroot … — the default"), then an unlabelled picker that
spans the whole pane, a faint switch, and nine buttons on two rows with a red Unlink between them.

**Services** (`latest/services.png`, `dark/services.png`): the first row's cells collapse into
"redisRedisrunning2837915myardchk-c". The sidebar shrinks.

**PHP** (`latest/php.png`): columns drift from row to row. Make default and a red Uninstall repeat
on all 12 rows. The extensions and php.ini controls the spec puts here cannot be seen.

**Node** (`latest/node.png`): an empty table, an install field squeezed to one character,
a full-width "Show available" button, and the available versions as one comma-joined line.

**Tunnels** (`latest/tunnels.png`, `installs/tunnels.png`): the share and add-domain forms sit
directly under the tables with collapsed fields, and the URL runs into the state ("…comready").

**Ports** (`latest/ports.png`, `real/ports.png`): columns drift between Pin and Unpin rows, and the
range field shows "29".

**Logs** (`latest/logs.png`): the filter field, the Pause switch and its label overlap. The log area
has no visible bounds, so with no lines the screen shows a floating "Copy all" and nothing else.

**Doctor** (`latest/doctor.png`, `real/doctor.png`): "Run doctor" is a full-width green bar. Passing
checks are bordered cards with the same weight as failures. Long messages push the page off the right
edge of the window.

**Sources** (`latest/sources.png`): readable, but every row repeats Untrust and nothing marks
which rows are taps and which are formulae.

**Activity** (`installs/activity-waiting.png`): the cards work. The status is plain grey text, the
Cancel button spans the card, and "waiting for php-install-1" shows an internal id.

**Onboarding** (`latest/onboarding.png`, `dark/onboarding.png`): green titles are the only sign a step
is done. The helper note runs out of its card, and the window cuts off "Check again" at the bottom.

**Settings** (no screenshot): from the code (`settings_window.rs:79-120`), it has small labels over
full-width fields and a Save row. That is acceptable; it only needs the new tokens.

## 2. Glitches

Status: **seen** means it is visible in a screenshot. **Suspected** means it is read from the code
only. Line numbers are as of a58e252.

| # | Glitch | Evidence | Cause (file:line) |
|---|---|---|---|
| G1 | Table columns do not line up with the header or with each other. A row's action buttons take width out of its cells, so rows with two buttons, one button, or none have different column widths, and the header (no buttons) has the widest. | seen: `latest/php.png` (the 8.4 row against the others), `latest/ports.png` (Pin and Unpin rows), `latest/overview.png` ("State" header over "running") | `widgets/table.rs:82-87` (header cells `flex_1`, no actions column), `:108-111` (cells `flex_1`, then `row.extra` appended) |
| G2 | Cells overrun or collapse: nothing sets `min_w(0)`, `overflow_hidden` or truncation, so long text paints into the next cell and narrow cells squash their text together. | seen: `latest/services.png`, `dark/services.png` ("redisRedisrunning…"), `latest/site-detail.png` (URL into "PHP 8.4"), `installs/tunnels.png` ("…comready") | `widgets/table.rs:109`; the Services row puts up to 8 buttons in `extra` (`services/table.rs:130-261`) |
| G3 | The sidebar is squeezed when the content is wide: `w(170)` without `flex_shrink_0`. | seen: `latest/services.png` (142 pt), `latest/doctor.png` (97 pt), `real/doctor.png` | `window/main.rs:339` |
| G4 | The content pane overflows sideways: the content column is `flex_1` with no `min_w(0)`, and a long, unwrapped Doctor message widens it past the window, so cards and the Run button lose their right edge. | seen: `latest/doctor.png` | `window/main.rs:238-249`; `screens/doctor.rs:89` |
| G5 | Text fields collapse to their padding in a row: `TextElement` asks for `relative(1.)` width, which resolves to nothing in a shrink-to-fit flex row, and `paint` has no content mask, so the placeholder draws over the controls beside it. | seen: `latest/node.png` ("22" behind Install), `latest/ports.png` ("29"), `latest/tunnels.png` ("bl", "ex"), `latest/logs.png` (the filter placeholder over the Pause switch) | `widgets/text_field.rs:511` (width), `:641`/`:670-672` (no width on the outer div), `:616` (unclipped paint); call sites `screens/node.rs:75`, `screens/tunnels.rs:216,225,234`, `screens/ports.rs:113`, `screens/logs.rs:76-84` |
| G6 | Old action errors pile up above every screen and never leave. An error is cleared only when the same action later succeeds, there is no dismiss, and the stack sits outside the scroll area, so two old errors take about 200 pt off every screen. | seen: "PHP 8.4 is the default version" and "redis is used by chk-c" on `latest/node.png`, `ports.png`, `logs.png`, `doctor.png`, `tunnels.png`, `sources.png`, `activity.png` | `window/main.rs:242` (`elsewhere_errors` outside `#content`); `runner.rs:58,62` (clear only on success); `widgets/error_view.rs:108-117` (full-bleed wash, no margin, so stacked blocks meet in a seam) |
| G7 | Controls stretch across the pane: they are direct children of a `flex_col`, whose default alignment is stretch. | seen: `latest/doctor.png` (Run doctor), `latest/node.png` (Show available), `installs/activity-waiting.png` (Cancel …), `latest/site-detail.png` (PHP picker) | `screens/doctor.rs:63-64`, `screens/node.rs:89`, `activity.rs:101`, `sites/detail.rs:195-196` |
| G8 | Onboarding does not scroll, so the last step and Check again are cut off. The helper note is a third column and runs out of its card. | seen: `latest/onboarding.png`, `dark/onboarding.png` ("macOS asks for y") | `onboarding.rs:149-156` (no `overflow_y_scroll`), `:140-145` (note as a row child) |
| G9 | Windows have no title. The spec names the main window `Yard`, but `WindowOptions` sets no `titlebar`, which leaves an empty bar, and Mission Control and the Window menu show nothing. | seen: every screenshot | `window/main.rs:80`, `onboarding.rs:218`, `settings_window.rs:150` |
| G10 | The log view has no visible bounds on the Logs screen: its background is `t.background`, the same as the page. "Copy all" is monospace text, not a button-shaped control. Long lines are neither wrapped nor clipped. | seen: `latest/logs.png`, `latest/activity.png`; long lines suspected | `widgets/log_view.rs:63-77` |
| G11 | Dark-mode filled buttons have weak contrast: dark text (`on_accent` 0x0b1f14) on mid-green 0x3fb37a, and the same dark text on salmon 0xff6b6b for Destructive. The sidebar badge puts that dark text on red. | seen: `real/overview.png` (Stop yardd…), `real/doctor.png` (Run doctor), `dark/services.png` (Remove…), `dark/onboarding.png` | `widgets/theme.rs:47-49`; `widgets/button.rs:78-79`; `window/main.rs:328-329` |
| G12 | Start yardd is enabled while yardd runs (and Stop while it is stopped). | seen: `latest/overview.png`, `real/overview.png` | `overview.rs:73,82` (no `.disabled`) |
| G13 | Rows are tall and uneven: the φ line height plus 32 pt buttons give 43 pt rows. Rows without buttons are 33 pt (`latest/site-detail.png` list), so the rhythm changes from screen to screen. | seen | `widgets/button.rs:89-96`; `widgets/table.rs:103`; no `line_height` set anywhere |
| G14 | The Overview's CA line is taller than its neighbours and its label sits above the value's centre: `line()` has no `items_center`, and a button is added to it. | seen: `latest/overview.png` | `overview.rs:35-46,64` |
| G15 | In light mode the off switch is almost invisible: track `t.border` (#d8d8d4) on white. | seen: `latest/site-detail.png` | `widgets/toggle.rs:70` |
| G16 | The sidebar badge is taller than its label and sits high. Its pill has no fixed height and uses a different line box. | seen: `latest/php.png`, `installs/activity-waiting.png` | `window/main.rs:322-333` |
| G17 | A job's status is not aligned with its title (no `items_center` or baseline). | seen: `latest/activity.png` ("done" sits higher than "Add MinIO") | `activity.rs:73-85` |
| G18 | Empty values are inconsistent: "-" in some cells and blank in others (PHP's Default column is blank, its Sites column is "-"). | seen: `latest/php.png`, `latest/ports.png` | `screens/php.rs` and `screens/ports.rs` cell builders |
| G19 | The picker popup sits at a hard-coded `mt(26)`, so once the picker's height changes (it is 30–32 pt today) the popup overlaps the button or leaves a gap. | suspected | `widgets/picker.rs:155` |
| G20 | Sheet fields have no visible label, only a placeholder that disappears once you type (the accessibility label is there). The sheet floats in the centre of the window rather than hanging from the top as a macOS sheet does. | suspected (no sheet screenshot) | `widgets/form.rs:147-163`; `.field(...)` call sites in `sites/forms.rs`, `services/forms.rs` |
| G21 | Text inside a field is drawn on `window.line_height()` while the box is a fixed 28 px. After the type scale changes, the text and caret will sit off-centre unless both come from one token. | suspected | `widgets/text_field.rs:512,667-671` |

## 3. Visual direction

Aim: it should look like a small native Mac utility (Activity Monitor or System Settings density),
not a web dashboard. That means a quiet chrome, the colour carried by state, and few buttons. The
mechanics: a full-height sidebar under a transparent title bar, a 13 px type scale with explicit line
heights, compact tables with status pills and one or two row actions, details in a grouped pane, and
errors attached to their control.

### 3.1 Window chrome

- Main window: `TitlebarOptions { title: Some("Yard"), appears_transparent: true,
  traffic_light_position: Some(point(px(16.), px(18.))) }`. The sidebar runs to the top edge. A 52 pt
  toolbar strip at the top of the content holds the page title on the left and the page's primary
  actions on the right, and dragging it moves the window (`window.start_window_move()` on mouse down
  in empty toolbar space). `window_min_size` is 760×480.
- The sidebar is 200 pt wide, fixed (`flex_shrink_0`), with background `sidebar_bg`, optionally
  `WindowBackgroundAppearance::Blurred` behind a translucent fill (last step, §5). It is split into
  three groups with 11 px secondary group labels. Its role stays `TabList` and each item stays a `Tab`,
  so the labels are text only:
  - **Yard**: Overview, Activity
  - **Serve**: Sites, Services, Tunnels
  - **Runtimes**: PHP, Node
  - **System**: Ports, Logs, Doctor, Sources

  The order changes, which `ax.js`'s tab lookups by name do not mind. A group label is plain text,
  not a tab.
- Onboarding (620×640) and Settings (560×380) get titles ("Set up Yard", "Yard Settings"), keep the
  standard title bar, and scroll.

### 3.2 Type scale (SF Pro is the GPUI system font; mono is "SF Mono", falling back to Menlo)

| Token | Size / line height | Weight | Use |
|---|---|---|---|
| `title` | 20 / 26 | 600 | page title in the toolbar strip |
| `section` | 13 / 18 | 600 | group headings ("Units", "Failing", "Domains") |
| `body` | 13 / 18 | 400 | cells, facts, messages, sidebar items |
| `body_strong` | 13 / 18 | 500 | a row's name cell, button labels |
| `secondary` | 12 / 16 | 400 | table headers, hints, sources ("from sites.toml") |
| `caption` | 11 / 14 | 500 | pills, badges, sidebar group labels |
| `mono` | 12 / 17 | 400 | log lines, hints that are commands |

Every text style sets its own `line_height`, so φ never applies.

### 3.3 Spacing, radius, sizes

- Spacing: `xs 4`, `sm 8`, `md 12`, `lg 16`, `xl 20`, `xxl 24`. The content pane's padding is
  `xl` left and right, `lg` top, `xxl` bottom; sections are `xxl` apart, items inside one `sm`.
- Radius: `sm 4` (pill inner, focus ring offset), `md 6` (buttons, fields, sidebar items), `lg 8`
  (group boxes, cards, log view), `xl 10` (sheets, popups).
- Control heights: `control 24` (buttons, pickers, fields), `control_small 20` (row actions),
  `row 28` (table row, text only), `row_actions 32` (a row with small buttons), `sidebar_item 28`,
  `header 24` (table header), `toolbar 52`.
- Widths: sidebar 200; field default 180 (`TextField::width(px)` overrides); picker min 120;
  sheet 460.

### 3.4 Colour tokens

These are named for their role, not their hue. Text colours meet 4.5:1 on their usual background
(checked against `content_bg`); state colours are paired with words or pills and are never the only
signal.

| Token | Light | Dark | Use |
|---|---|---|---|
| `window_bg` | #F5F5F7 | #1E1E1E | behind everything |
| `sidebar_bg` | #EDEDEF (≈ 92% when blurred) | #262628 | sidebar |
| `content_bg` | #FFFFFF | #1E1E1E | content pane |
| `surface` | #FFFFFF | #2A2A2C | cards, group boxes, sheets, popups |
| `surface_sunken` | #F5F5F7 | #161617 | log view, a group box on `surface` |
| `control_bg` | #FFFFFF | #3A3A3C | buttons, pickers, fields |
| `control_border` | rgba(0,0,0,.14) | rgba(255,255,255,.10) | buttons, pickers, fields |
| `separator` | rgba(0,0,0,.09) | rgba(255,255,255,.09) | row lines, panel edges |
| `text` | #1D1D1F | #F2F2F4 | primary text |
| `text_secondary` | #6A6A6F | #98989D | headers, hints, facts' labels |
| `text_tertiary` | #A1A1A6 | #6C6C70 | placeholders, disabled labels |
| `accent` | #1F7A4D | #4CC38A | links, switch on, focus ring, selected-row wash |
| `accent_fill` | #1F7A4D | #23794F | primary button fill |
| `on_accent` | #FFFFFF | #FFFFFF | text on `accent_fill` and on `danger_fill` |
| `selection` | rgba(0,0,0,.08) | rgba(255,255,255,.10) | sidebar selected item |
| `row_selected` | accent at .14 | accent at .20 | selected table row |
| `hover` | rgba(0,0,0,.04) | rgba(255,255,255,.05) | row and sidebar hover |
| `focus_ring` | accent at .40, 3 px | accent at .50, 3 px | focused field or control |
| `success` / `success_text` | #34C759 / #137029 | #30D158 / #4CD964 | running, serving, done, trusted |
| `warning` / `warning_text` | #FF9500 / #944F00 | #FF9F0A / #FFB340 | needs setup, warnings, waiting |
| `danger` / `danger_text` | #FF3B30 / #B5190D | #FF453A / #FF8E86 | failed, error, destructive labels |
| `danger_fill` | #D93025 | #C9372C | the sidebar's failure badge only |
| `neutral` / `neutral_text` | #8E8E93 / #5F5F64 | #98989D / #B8B8BC | stopped, sleeping, off, free, cancelled |

The `*_text` values and `accent_text` (#0E693C light, #5BD299 dark, for an accent pill's text) were
made darker in light and lighter in dark when built (2026-11-10), so that each passes 4.5:1 on its pill
wash both on the content and on a selected row, and `text_secondary` on `sidebar_bg`; the test
`text_tokens_meet_contrast` checks every pair. A pill's background is its state colour at .15 alpha (`Theme::wash`, with the alpha as a parameter).
A wash under danger text at .15 keeps 4.5:1 for both themes' `*_text`.

### 3.5 Components

- **Button**: `control` height (fixed `h`, not padding), `md` radius, `px 10`, `body_strong`. It has
  three styles:
  - **Normal**: `control_bg` and `control_border`, with a 1 px bottom shadow in light mode.
  - **Primary**: `accent_fill` with `on_accent`, at most one per page or sheet.
  - **Destructive**: Normal with `danger_text` as the label. No fill, since the prompt that follows
    carries the red.

  A `.small()` variant (`control_small`, 12 px label) is for rows. Hover and press darken by 4%/8%.
  Disabled is 45% opacity, keeps `Disabled` and its description, and has no pointer cursor. A button
  sizes to its label (`flex_none`) and never stretches.
- **Picker**: looks like a Normal button with a trailing "⌄" in `text_secondary`. Its popup is
  anchored to the measured button bounds (no `mt(26)`), under the button, or above it when it would
  not fit under it, on `surface`, with `xl` radius, shadow, 24 pt items, a check mark on the chosen
  item and `row_selected` on hover.
- **Text field**: `control` height and `md` radius on `control_bg`, with a `control_border` that
  becomes the `focus_ring` when focused. 180 pt wide by default. Text and caret are centred on the
  `body` line, and the paint is clipped to the field's bounds (`window.with_content_mask`).
- **Switch**: a 32×18 track with a 14 pt knob. Off is #D1D1D6 light or #48484A dark, with a knob
  shadow; on is `accent`. Its label sits to the left as a form label, or to the right inline.
- **Table**: `Table::new(id, columns, rows)` where a column is `Column { title, width: Fixed(px) |
  Flex(f32), align }`, plus `actions(width)`, which reserves the same trailing width in the header and
  in every row. That fixes G1.
  - Cells: `min_w(0)`, `truncate()` (ellipsis), `body`; the first column is `body_strong`.
  - Header: `header` height, `secondary` text, a `separator` under it.
  - Rows: `row` or `row_actions` height, `hover` wash, `row_selected` when selected, `separator`
    lines inset 8 pt.
  - With no rows, the table draws its empty state.
  - A row is still a `ListItem` keyed by its row key, and the table is still a `List` labelled by its
    column titles.
- **Status pill** (new `widgets/pill.rs`): `caption` text on its wash, 18 pt high, 6 px dot then
  word, `px 7`, fully rounded. The word goes through `widgets::text`, so VoiceOver reads it as the
  cell's text, as it does today. Mapping:
  - success: running, serving, listening, ready, trusted, done, ok
  - warning: needs setup, starting, waiting, deprecated, warn
  - danger: failed, error, in use
  - neutral: stopped, sleeping, off, free, cancelled, tapped, installed
- **Group box**: `surface` with a `separator` border, `lg` radius and inset rows of 32 pt (label on the
  left in `text_secondary`, value or control on the right, or label above for long values). It is the
  System Settings look, used by the Overview facts, the site and service details, PHP's extensions,
  Settings, and onboarding steps.
- **Error view (inline)**: a 3 px `danger` bar on the left, `danger` wash at .10, `md` radius. The
  message is in `body` `danger_text`, the hint in `mono` `text_secondary`, and the buttons are small.
  It sits directly under the control that failed. Role `Alert` is unchanged.
- **Issues strip** (the "elsewhere" errors, G6): it lives inside the scroll area, at the top of the
  content. Each error is one compact line: a danger dot, the message, a small hint button, and Dismiss
  (a small Normal button whose description is "dismisses <message>", which calls
  `ActionErrors::clear`). An error longer than 60 characters, or one with a hint or output, offers
  Details, which shows it whole. With three or more errors it shows two and "N more…" to expand. An error
  still appears until it is dismissed or its action succeeds; the rule that every failure reaches the
  screen holds.
- **Banner** (offline, config, stale): full width under the toolbar, 36 pt, wash background with a
  `separator` bottom border, `body` text and small buttons on the right. The primary action (Start
  yardd) is Primary.
- **Log view**: `surface_sunken`, `separator` border, `lg` radius, `mono`, padding 8, lines wrapped
  (`whitespace_normal`) and clipped. A slim header row holds a line count on the left and a small
  "Copy all" Button on the right; it stays a `Role::Button` with the same id. On the Logs screen it
  fills the rest of the height and scrolls itself. With no lines it shows "No log lines yet" in
  `text_secondary`.
- **Empty state**: inside the table's area, 120 pt tall and centred. It has one line of `body`
  `text_secondary` plus at most one button, for example "No tunnels. Share a site to get a public
  URL." with Share…, or "No Node versions installed." with Install….
- **Sheet**: drops from the top of the content, 460 wide, `surface`, `xl` radius, shadow. Its title is
  `section` and each field has a visible `secondary` label above it, using the same text as its
  accessibility label. The buttons are right-aligned (Cancel, then Primary). It keeps the backdrop at
  .25 and stays occluded, Enter and Escape work, and `Form::error` stays under the fields.
- **Sidebar item**: `sidebar_item` height, `md` radius, `px 8`, `body`; selected is `selection` with
  `body_strong` text; badge is a `caption` pill (danger_fill + on_accent for failures, neutral wash
  for running jobs) 16 pt high, vertically centred.

### 3.6 How `theme.rs` holds it

```rust
pub struct Theme { pub c: Colors }                // Theme::of(window) unchanged
pub struct Colors { pub window_bg: Hsla, pub sidebar_bg: Hsla, /* … every §3.4 token … */ }
impl Theme {
    pub fn light() -> Theme; pub fn dark() -> Theme;
    pub fn wash(color: Hsla, alpha: f32) -> Hsla;   // today's wash takes the alpha
    pub fn state(&self, s: Tone) -> (Hsla /*dot*/, Hsla /*text*/, Hsla /*wash*/);
}
pub enum Tone { Success, Warning, Danger, Neutral, Accent }
pub mod space  { pub const XS: Pixels = px(4.); /* SM 8 … XXL 24 */ }
pub mod radius { pub const SM: Pixels = px(4.); /* MD 6, LG 8, XL 10 */ }
pub mod size   { pub const CONTROL: Pixels = px(24.); /* CONTROL_SMALL, ROW, ROW_ACTIONS,
                 SIDEBAR, SIDEBAR_ITEM, HEADER, TOOLBAR, FIELD */ }
/// Type tokens as a Styled extension, so a call site says `.type_body()`.
pub trait Typography: Styled + Sized {
    fn type_title(self) -> Self;   // size, line height, weight
    fn type_section(self) -> Self; fn type_body(self) -> Self; fn type_body_strong(self) -> Self;
    fn type_secondary(self) -> Self; fn type_caption(self) -> Self; fn type_mono(self) -> Self;
}
```

During the migration, the old field names (`background`, `surface`, `text`, `muted`, `border`,
`accent`, `on_accent`, `danger`, `warning`) stay as thin aliases on `Theme`, so screens move one at a
time. A unit test checks the contrast of each `*_text` and `on_accent` pair against its background
(WCAG relative luminance, ≥ 4.5).

## 4. Screens

Sketches at about 900×600. `[ ]` is a button, `[x ⌄]` a picker, `(●)` a pill, `▸` a selected row.

**Overview**
```
│ Overview                                      [Restart yardd] [Stop yardd…]  │
│ ┌ Yard ──────────────────────────────────────────────────────────────────┐ │
│ │ yardd        0.1.0 (2bedf38)                              (● running)   │ │
│ │ Helper       0.1.0                                                      │ │
│ │ Profile      standard · .yard · 8080 / 8443                             │ │
│ │ HTTPS CA     not trusted · expires 2036-11-06                   [Trust] │ │
│ │ Data root    /tmp/yard-check                                            │ │
│ │ Yard.app     0.1.0 (2bedf38)                         [Set up Yard…]     │ │
│ └────────────────────────────────────────────────────────────────────────┘ │
│ Units                                                                       │
│ Unit            State          PID      Restarts                            │
│ nginx           (● running)    40711    0                       [Restart]   │
```
Start yardd replaces Restart and Stop when yardd is offline (G12). The banner already offers it.

**Sites, list and detail** (the detail is a right pane at 900 pt and wider, below the list when
narrower)
```
│ Sites                                          [Add proxy…] [Link folder…]   │
│ Name     URL                          PHP      HTTPS  Status        │ chk-a │
│▸chk-a    http://chk-a.yard:28080/     8.4      off    (● serving)   │ http://chk-a.yard:28080/ ↗
│ chk-c    http://chk-c.yard:28080/     7.1      off    (● needs setup)│ ┌──────────────────────┐
│                                                                     │ │ PHP     [Default (8.4)⌄]
│                                                                     │ │         from sites.toml
│                                                                     │ │ HTTPS   [ ○ ]        │
│                                                                     │ │ Docroot …/chk-a  [Detect]
│                                                                     │ │ Node    22 · ~/.nvmrc │
│                                                                     │ └──────────────────────┘
│                                                                     │ [Open in browser] [Open in…⌄]
│                                                                     │ [Copy .env] [Save to .yard]
│                                                                     │ [Edit nginx override]
│                                                                     │ Unlink…  (danger text)
```
"Open in…" is a picker-style menu (Editor, Finder, Terminal). Its items keep their ids and labels as
`MenuItem`s; the checklist needs updating (§5 step 10).

**Services**: list and detail, as Sites. `Screen::Services(Option<name>)` already exists.
```
│ Services                                                  [Add service…]     │
│ Instance     Engine        State          Ports        Sleep     Used by     │
│▸redis        Redis 8.10    (● running)    28379        15 min    chk-c [Stop]│
│ emailulator  emailulator   (● running)    2525, 8026   —         —    [Logs] │
│ ┌ redis ───────────────────────────────────────────────────────────────────┐│
│ │ Port 28379 [Port…]   Sleeps after 15 min [Sleep after…]   Run by yard     ││
│ │ [Restart] [Logs] [Copy .env] [Open console]                Remove…        ││
│ └───────────────────────────────────────────────────────────────────────────┘│
│ A failed instance's reason and fix go under the table, as today.            │
```

**PHP**
```
│ PHP                                                   [Install a version…]   │
│ Version  Status          Sites                 Pool                          │
│ 8.5      installed       —                     —                             │
│▸8.4      (● default)     chk-a, chk-b, chk-new (● running)                   │
│ 7.1      installed       chk-c                 —                             │
│ ┌ PHP 8.4 ─────────────────────────────────────────────────────────────────┐ │
│ │ [Make default]  Xdebug [ ○ ]  [Open 99-overrides.ini]       Uninstall…   │ │
│ │ Extensions                                         [Add extension…]      │ │
│ │ redis     [ ● ]                                    Remove                │ │
│ │ php.ini   memory_limit = 512M   [Set…]                                   │ │
│ └───────────────────────────────────────────────────────────────────────────┘ │
```
Row actions move into the selected version's group box. That removes 20 buttons and every red fill.

**Node**
```
│ Node                                  [22      ] [Install]   [Show available] │
│ Version   Status          Sites                                              │
│ 22.11.0   (● default)     eepower, yard-site          [Make default] Uninstall…│
│   (empty state: "No Node versions installed.")                               │
│ Available (shown on request): a 4-column grid of `secondary` versions        │
```

**Tunnels**
```
│ Tunnels                                                         [Share…]     │
│ Site    Kind    URL                                     State                │
│ chk-t   quick   https://quick-29000.trycloudflare.com   (● ready) [Copy][Open] Stop│
│   (empty: "No tunnels. Share a site to get a public URL.")                   │
│ Domains for named tunnels                                    [Add domain…]   │
│ example.com    signed in                                           Remove    │
```
Share… opens a sheet (a site picker, Quick or Named, a host) in place of the two inline fields. The
fields keep their ids.

**Ports**
```
│ Ports        Profile [standard (8080, 8443) ⌄]   [Range…]   [Check now]      │
│ Unit             Port    Mode       Status          Held by                  │
│ dnsmasq          28053   pinned     (● yard)        —           [Unpin][Set…]│
│ emailulator-smtp 2525    external   (● listening)   —           [Pin]  [Set…]│
```
Range… opens a sheet; there is no inline field (G5).

**Logs**
```
│ Logs     [All units ⌄]  [⌕ Filter          ]  Pause [ ○ ]  [Show log file]    │
│ ┌ 1,204 lines ─────────────────────────────────────────────── [Copy all] ┐  │
│ │ 12:01:03 nginx   GET / 200                                             │  │
│ │ …  (fills the height, scrolls itself; empty: "No log lines yet")        │  │
```

**Doctor**
```
│ Doctor                                    Last run 12:03         [Run doctor] │
│ Failing (1)                                                                  │
│ │ chk-c needs the xdebug extension for PHP 7.1 and PostgreSQL 17           │ │
│ │ cd …/chk-c && yard init                                    [Fix] [Copy]  │ │
│ Warnings (3)                                                                 │
│ ✓ 12 checks passed                                            [Show ⌄]       │
```
Failing and Warnings are inline error-style cards (danger or warning bar). Passing checks fold into
one line that expands to a compact list. Messages wrap (`min_w(0)`).

**Sources**: the table as today, with taps as section rows (`body_strong`, `surface_sunken`) and
their formulae under them, a Status pill (installed or tapped) and a Trust pill, and a small
Untrust or Trust button.

**Activity**
```
│ Activity                                                                     │
│ ┌ Install PHP 7.2                           (● waiting for Install PHP 7.1) ┐│
│ │ ┌ log ────────────────────────────────────────────────── [Copy all] ┐   ││
│ │ └────────────────────────────────────────────────────────────────────┘ [Cancel]│
│ └───────────────────────────────────────────────────────────────────────────┘│
```
Cancel is a small button labelled "Cancel" with the description "Cancel <title>", so VoiceOver
keeps what it hears today. A finished job's log folds to its last line.

**Settings**: one group box with three rows, `yard` command, Editor and Terminal, each with a value
and Choose…, plus a read-only Data root row. Save is Primary, bottom right, with its result line to
its left.

**Onboarding**
```
│ Set up Yard                                                                  │
│ Each step shows done once it is. Do one in Terminal and come back.           │
│ ┌──────────────────────────────────────────────────────────────────────────┐ │
│ │ ✓ Homebrew                                                   (● done)    │ │
│ │ ✓ The yard command                                           (● done)    │ │
│ │ 3 yardd                                                  [Start yardd]   │ │
│ │ 4 Root helper: names under .yard, and 80/443 if wanted [Open in Terminal]│ │
│ │   macOS asks for your password in Terminal: the helper writes …          │ │
│ │ 5 HTTPS (optional)                                   (● waiting) …       │ │
│ └──────────────────────────────────────────────────────────────────────────┘ │
│                                                             [Check again]    │
```
The window scrolls and the note sits under the step's title (G8). Done is a ✓ glyph plus a pill, not
colour alone.

## 5. Changes, smallest risk first

Each step keeps the tests green, keeps every `A11y` id, role and label (or updates the checklist
scripts in the same step, as noted), and is checked with `checklist.sh` and `checklist-installs.sh`
as `app.md` requires.

1. **Titles and minimum size** (G9). `window/main.rs` (open), `onboarding.rs` (open),
   `settings_window.rs` (open): set `titlebar.title` and `window_min_size`. Check how `ax.js` and the
   checklist scripts match onboarding and Settings windows before naming them.
2. **Layout containment** (G3, G4). `window/main.rs:239-249,339`: the sidebar gets `flex_shrink_0`;
   the content column and `#content` get `min_w(px(0.))`; Doctor's message gets `min_w(0)` and wraps
   (`screens/doctor.rs:81-89`).
3. **No stretched controls** (G7). Add a `widgets::bar()` helper (a flex row, `items_center`,
   `gap(sm)`, `flex_wrap`) and wrap the lone controls in it: `screens/doctor.rs:63`,
   `screens/node.rs:89`, `activity.rs:101`, `sites/detail.rs:196`. Button gets `flex_none`
   (`widgets/button.rs`).
4. **Text field sizing and clipping** (G5, G21). `widgets/text_field.rs`: a `width` builder
   (default 180), the outer div's `w` set from it, the paint wrapped in `with_content_mask(bounds)`,
   and text centred on one line-height token. Call sites: `screens/node.rs`, `screens/tunnels.rs`,
   `screens/ports.rs`, `screens/logs.rs`, `settings_window.rs` (`w_full` there).
5. **Onboarding fixes** (G8). `onboarding.rs:113-156`: the root scrolls, the note moves into the
   title column, and the row gets `min_w(0)`.
6. **Theme tokens** (no visual change yet). `widgets/theme.rs`: `Colors`, `Tone`, `space`, `radius`,
   `size` and `Typography` as in §3.6, the old names as aliases, and the contrast test.
7. **Dark-mode contrast and button styles** (G11, G13). `widgets/button.rs` (fixed heights, `small()`,
   Destructive as red text, hover and press), `window/main.rs:322-333` (badge, G16). Every
   `ButtonStyle::Destructive` call site keeps its style name, so only the look changes.
8. **Type scale and line heights** (G13). Apply `Typography` in `widgets/*`, `screens/mod.rs`
   (`heading`), then each screen. Sidebar items go to 28 pt (`window/main.rs:301-350`).
9. **Table columns and actions** (G1, G2, G18). `widgets/table.rs`: `Column`, `actions(width)`,
   truncation, hover, heights, empty state, and a single `—` for every empty value. Call sites:
   `overview.rs`, `sites/mod.rs`, `services/table.rs`, `screens/php.rs`, `screens/node.rs`,
   `screens/tunnels.rs`, `screens/ports.rs`, `screens/sources.rs`. The `a_table_is_a_list…` and
   row-id tests stay as they are.
10. **Status pills** (new `widgets/pill.rs`, plus a unit test that its text is read). Replace the
    plain state cells in the same call sites, then `activity.rs:80-84` (G17) and onboarding's step
    states.
11. **Errors** (G6). `window/main.rs:242` moves `elsewhere_errors` into `#content` as the issues
    strip; `widgets/error_view.rs` gets the compact inline style and the Dismiss button (description
    as in §3.5); `runner.rs` exposes `clear` for it. Add a test that a dismissed error leaves
    `elsewhere_error_ids`, and keep `an_error_shows_on_whatever_screen_is_open`.
12. **Small widgets**: `widgets/toggle.rs` (G15), `widgets/picker.rs` (anchored to its bounds, G19;
    check mark; small variant), `widgets/log_view.rs` (G10: frame, wrap, Copy all as a Button with
    the same id, empty text), `widgets/form.rs` (G20: visible labels from each field's label,
    top-anchored placement). Run `checklist-installs.sh` for the sheets.
13. **Group boxes and details**: a new `widgets/group.rs`; the Overview facts (`overview.rs:35-68`,
    G14, G12 with `disabled` from the connection state); the site detail (`sites/detail.rs:171-380`,
    with "Open in…" as a menu); Settings (`settings_window.rs`).
14. **Row actions into detail panes** (Services, PHP). `services/table.rs` plus a new
    `services/detail.rs`, and `screens/php.rs`. This is the riskiest step for the checklist: it presses
    `service:<n>:port`, `:sleep`, `:remove` and `php:<v>:uninstall` from rows, so update
    `app/scripts/checklist.sh` and `checklist-installs.sh` to select the row first, in the same change.
    The ids stay the same.
15. **Sheets for inline forms**: Tunnels Share… and Add domain…, Ports Range…, Node Install
    (`screens/tunnels.rs`, `screens/ports.rs`, `screens/node.rs`, `sites/forms.rs` or a new
    `screens/forms.rs`). The field ids stay; the checklist opens the sheet first.
16. **Sidebar groups and order** (§3.1): `window/main.rs:122-134,301-350`. Group labels are plain
    `widgets::text`, not tabs.
17. **Transparent title bar and toolbar strip**: `window/main.rs` (`TitlebarOptions`, traffic-light
    position, a 52 pt toolbar with the page title and primary actions, window drag from its empty
    space). Each screen's header row moves into the toolbar (`screens/mod.rs::heading` becomes
    `toolbar(title, actions)`). Check that VoiceOver still finds the heading text (id `heading`).
18. **Optional, last**: `WindowBackgroundAppearance::Blurred` under a translucent `sidebar_bg`.
    Keep it only if text in the content pane stays opaque and readable in both appearances.

## 6. Logo (D3, chosen 2026-11-10)

A green tile holding a light disc; on the disc, two requests (the upper nodes) join and land on a
site (the lower node). It replaces the green "Y" `AppIcon.icns` and the plain circle the menu bar
drew (`menubar.rs` `icon`, now `menubar/icon.rs`). The canvas board "Logo: D3, the light disc" shows every size.

**App icon** (viewBox 100; the source is `app/yard-app/macos/AppIcon.svg`, 1024 px):

- Tile: `rect` rx 22.5, a vertical gradient #34A86C → #1B6E44.
- Disc: `circle` cx 50, cy 51, r 35, #F3FBF6.
- Routes: `M37 37 L50 52 L63 37 M50 52 V66`, stroke #1F7A4D (the app's `accent`), width 5, round
  caps and joins; nodes (37,37) r6, (63,37) r6, (50,52) r4; the site (50,68) r7.5, all #1F7A4D.
- `AppIcon.icns` holds the standard iconset sizes (16 to 512, each at 1x and 2x), rendered from the
  SVG by `app/scripts/make-icon.sh`. Onboarding's heading shows the icon at 56 pt beside "Set up Yard".

**Menu bar icon**: an 18 pt template image (36×36 px, black on clear; macOS tints it), in the same
36 grid:

- normal: a disc (centre 18,18, r 15) with the routes cut out of it: `M12.5 12.5 L18 18.5 L23.5
  12.5 M18 18.5 V24` at width 2.3, nodes (12.5,12.5) and (23.5,12.5) r 2.6, the site (18,25.5) r 3.2.
- busy, warning, error: the disc with its corner cut away (a circle at 29,29, r 9) and a badge in the
  cut. Busy is a ring (centre 29,29, r 4.6, stroke 2.2); warning a triangle `M29 22.5 L35.5 34.5
  H22.5Z`; error a solid dot (r 5.8). Warning and error now differ, which the circle did not.
- offline: the normal icon at 38% opacity.

Edges are antialiased (supersampled coverage as alpha). The checklist's `ax.js icon` reads the
shape from a screenshot at points that tell the five states apart (plan Task 18).

Out of scope here, and noted only: the spec's searchable Sites list and **+** button (§8.2) and
Activity as a toolbar button. Both are features rather than looks, and each needs its own plan item.

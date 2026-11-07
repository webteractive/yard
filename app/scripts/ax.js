// Drives Yard.app through macOS accessibility, for scripts/checklist.sh (the M6 plan's Task 17
// checklist). Run as `osascript -l JavaScript ax.js <command> [args…]`; every command prints JSON.
// The calling app (the terminal) needs Accessibility, and screenshots need Screen Recording.
//
//   menu                      the menu bar menu as a tree: [{name, mark, enabled, children}]
//   menu-click <name>…        opens the menu and clicks the item at that path
//   windows                   [{index, title, main}] (main: it has the Screens tab group)
//   frame                     the main window's frame as x,y,w,h, for screencapture -R
//   tree [other]              the main window's elements: [{role, subrole, name, desc, help, enabled,
//                             value, row}] (help: AXHelp, a control's description such as why it is off) (a group only when named: a banner is an AXApplicationAlert);
//                             `other`: the first window that is not the main one (Settings, a sheet)
//   tab <name>                selects a screen in the main window
//   press <name> [row]        presses the button, switch, checkbox or picker named <name>, inside
//                             table row [row] if given; [row] `@other` looks only outside the main
//                             window (Settings, onboarding)
//   type <name> <text>        focuses the text field named <name>, selects all and types <text>
//   key <return|escape|tab>   presses that key in Yard
//   pick <picker> <option>    opens the picker named <picker> and chooses <option>
//   goto <folder>             in the open panel Yard shows, goes to <folder> and chooses it
//   windows-of <process>      another app's window titles (an editor, Finder): [title]
//   close-window <process> <title>  presses that window's close button
//   row <name>                presses (opens) the table row named <name>
//   dialog                    the open prompt's text and buttons, or null
//   answer <button>           presses a button of the open prompt
//   watch <ready> <timeout> menu-header|control:<name> is|not <text>
//                             finds the element (opening the menu for menu-header and keeping it
//                             open), touches <ready>, then polls it every 50 ms and prints
//                             {at, value} when it matches, `at` in Unix seconds; {at: null} on timeout
//   icon                      the menu bar icon's shape: ok (filled circle), offline (ring), busy
//                             (ring and dot) or warning (notched circle), from a screenshot
//   menu-log <ready> <seconds>  opens the menu, touches <ready>, and records every distinct list of
//                             its top-level item names for <seconds> without reopening it: [{at, items}]
//
// The status item is clicked with a real mouse event: tray-icon's button ignores AXPress.

ObjC.import("CoreGraphics");

const se = Application("System Events");

function yard() {
  const p = se.processes.whose({ name: "Yard" });
  if (p.length === 0) throw new Error("Yard is not running");
  return p[0];
}

function mouseClick(x, y) {
  const pt = $.CGPointMake(x, y);
  [$.kCGEventMouseMoved, $.kCGEventLeftMouseDown, $.kCGEventLeftMouseUp].forEach((t) => {
    $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent($(), t, pt, $.kCGMouseButtonLeft));
    delay(0.05);
  });
}

function statusItem() {
  return yard().menuBars[0].menuBarItems[0];
}

function openMenu() {
  const item = statusItem();
  if (item.menus.length === 0) {
    const [x, y] = item.position();
    const [w, h] = item.size();
    mouseClick(x + w / 2, y + h / 2);
    for (let i = 0; i < 20 && item.menus.length === 0; i++) delay(0.1);
  }
  if (item.menus.length === 0) throw new Error("the menu bar menu did not open");
  return item.menus[0];
}

function closeMenu() {
  try {
    if (statusItem().menus.length > 0) se.keyCode(53);
  } catch (e) {
    // The item quit the app (Quit Yard.app): there is no menu left to close.
  }
}

function readMenu(menu) {
  return menu.menuItems().map((mi) => {
    let mark = null;
    try { mark = mi.attributes.byName("AXMenuItemMarkChar").value() || null; } catch (e) {}
    const entry = { name: mi.name() || null, mark, enabled: mi.enabled() };
    try { if (mi.menus.length) entry.children = readMenu(mi.menus[0]); } catch (e) {}
    return entry;
  });
}

function menuItemAt(menu, path) {
  let item = null;
  path.forEach((name, i) => {
    item = menu.menuItems().find((mi) => mi.name() === name);
    if (!item) throw new Error("no menu item " + JSON.stringify(path.slice(0, i + 1)));
    if (i < path.length - 1) {
      item.actions.byName("AXPress").perform();
      delay(0.3);
      menu = item.menus[0];
    }
  });
  return item;
}

function screens(w) {
  try {
    return w.entireContents().find((e) => str(() => e.role()) === "AXTabGroup" && str(() => e.name()) === "Screens") || null;
  } catch (e) { return null; }
}

function isMain(w) {
  return screens(w) !== null;
}

// GPUI builds a window's accessibility tree only once the app is active, so a window cannot be
// told apart before Yard is frontmost.
function activate() {
  const p = yard();
  if (!p.frontmost()) {
    p.frontmost = true;
    delay(0.5);
  }
  return p;
}

function findMain(p) {
  for (let i = 0; i < 10; i++) {
    const w = p.windows().find(isMain);
    if (w) return w;
    delay(0.2);
  }
  return null;
}

function mainWindow() {
  const p = activate();
  const w = findMain(p);
  if (!w) throw new Error("the main window is not open");
  if (p.windows()[0] && isMain(p.windows()[0])) return w;
  w.actions.byName("AXRaise").perform();
  // Raised is not key: GPUI keeps the accessibility tree of the window in front, so make it main.
  try { w.attributes.byName("AXMain").value = true; } catch (err) {}
  delay(0.3);
  // System Events names a window by its place in the order, which raising it just changed.
  const raised = findMain(p);
  if (!raised) throw new Error("the main window lost its screens when raised");
  return raised;
}

function str(f) {
  try { return f() || null; } catch (e) { return null; }
}

const PRESSABLE = ["AXButton", "AXCheckBox", "AXPopUpButton", "AXMenuItem", "AXRadioButton"];

function otherWindow() {
  const p = activate();
  const w = p.windows().find((x) => !isMain(x));
  if (!w) throw new Error("no window but the main one is open");
  return w;
}

// The element named `name` with one of `roles`: in the main window when it is open, else in any of
// Yard's windows (Settings, onboarding).
function findNamed(name, roles, row) {
  const p = activate();
  // `@other`: only in the windows that are not the main one (Settings, onboarding).
  const other = row === "@other";
  if (other) row = null;
  const find = () => {
    const windows = other ? p.windows().filter((w) => !isMain(w)) : p.windows();
    return windows.flatMap((w) => w.entireContents()).filter((e) => {
      if (!roles.includes(str(() => e.role())) || str(() => e.name()) !== name) return false;
      if (!row) return true;
      try { return e.attributes.byName("AXParent").value().name() === row; } catch (err) { return false; }
    });
  };
  // A window nobody has asked about yet answers with an empty tree until GPUI's next frame.
  let match = find();
  for (let i = 0; i < 10 && match.length === 0; i++) {
    delay(0.2);
    match = find();
  }
  if (match.length !== 1) throw new Error(match.length + " elements named " + name + (row ? " in row " + row : ""));
  return match[0];
}

function clickCentre(e) {
  const [x, y] = e.position();
  const [w, h] = e.size();
  mouseClick(x + w / 2, y + h / 2);
}

function press(e) {
  try {
    e.actions.byName("AXPress").perform();
  } catch (err) {
    clickCentre(e);
  }
}

const KEYS = { return: 36, escape: 53, tab: 48 };

// A table row: the app's tables are lists of groups (AccessKit gives a macOS table no rows, so
// VoiceOver read every table as empty), so a group in a list is reported as AXRow.
function isRow(e) {
  const role = str(() => e.role());
  if (role === "AXRow") return true;
  // A row is named (its first cell); asking every unnamed group for its parent slows a tree read
  // past the scripts' waits.
  if (role !== "AXGroup" || !str(() => e.name())) return false;
  try { return e.attributes.byName("AXParent").value().role() === "AXList"; } catch (err) { return false; }
}

function tree(w) {
  const out = [];
  w.entireContents().forEach((e) => {
    let role = str(() => e.role());
    // A group is kept only when it carries a name: a banner is an alert group named by its text.
    if (!role || (role === "AXGroup" && !str(() => e.name()))) return;
    if (role === "AXGroup" && isRow(e)) role = "AXRow";
    let row = null;
    try {
      const parent = e.attributes.byName("AXParent").value();
      if (parent && isRow(parent)) row = parent.name();
    } catch (err) {}
    let value = null;
    if (role === "AXCheckBox" || role === "AXPopUpButton" || role === "AXStaticText" || role === "AXTextField") value = str(() => String(e.value()));
    const help = str(() => e.attributes.byName("AXHelp").value());
    out.push({ role, subrole: str(() => e.subrole()), name: str(() => e.name()), desc: str(() => e.description()), help, enabled: (() => { try { return e.enabled(); } catch (err) { return null; } })(), value, row });
  });
  return out;
}

function dialogOf(w) {
  const sheets = w.sheets();
  if (sheets.length) return sheets[0];
  return null;
}

function openDialog() {
  const p = yard();
  for (const w of p.windows()) {
    const s = dialogOf(w);
    if (s) return s;
    if (str(() => w.subrole()) === "AXDialog") return w;
  }
  return null;
}

function touch(path) {
  $.NSString.alloc.initWithUTF8String("").writeToFileAtomicallyEncodingError(path, true, $.NSUTF8StringEncoding, null);
}

function watch(ready, timeout, target, op, text) {
  let read;
  if (target === "menu-header") {
    // Looked up on every poll: a menu whose items are added or removed is replaced, not patched.
    openMenu();
    read = () => str(() => openMenu().menuItems[0].name());
  } else if (target.startsWith("control:")) {
    const name = target.slice("control:".length);
    const e = mainWindow().entireContents().find((x) => str(() => x.name()) === name);
    if (!e) throw new Error("no control named " + name);
    read = () => str(() => String(e.value()));
  } else {
    throw new Error("unknown watch target " + target);
  }
  touch(ready);
  const end = Date.now() + timeout * 1000;
  let value = read();
  while (Date.now() < end) {
    value = read();
    if ((value === text) === (op === "is")) {
      const at = Date.now() / 1000;
      if (target === "menu-header") closeMenu();
      return { at, value };
    }
    delay(0.05);
  }
  if (target === "menu-header") closeMenu();
  return { at: null, value };
}

function menuLog(ready, seconds) {
  const item = statusItem();
  openMenu();
  touch(ready);
  const log = [];
  let last = "";
  const end = Date.now() + seconds * 1000;
  while (Date.now() < end) {
    if (item.menus.length === 0) {
      log.push({ at: Date.now() / 1000, items: null });
      break;
    }
    const items = item.menus[0].menuItems().map((mi) => str(() => mi.name()));
    const key = JSON.stringify(items);
    if (key !== last) {
      log.push({ at: Date.now() / 1000, items });
      last = key;
    }
    delay(0.1);
  }
  closeMenu();
  return log;
}

ObjC.import("AppKit");

// The icon is an 18 pt template image drawn in the menu bar's text colour (menubar.rs `icon`).
// Its centre is on for ok and busy, off for offline and warning; a point 3.5 pt to the right of
// the centre is on for ok and warning, off for offline and busy.
function iconShape() {
  const [x, y] = statusItem().position();
  const [w, h] = statusItem().size();
  const file = $.NSTemporaryDirectory().js + "yard-icon-" + Date.now() + ".png";
  const app = Application.currentApplication();
  app.includeStandardAdditions = true;
  app.doShellScript(`screencapture -x -R${x},${y},${w},${h} '${file}'`);
  const rep = $.NSBitmapImageRep.imageRepWithData($.NSData.dataWithContentsOfFile(file));
  app.doShellScript(`rm -f '${file}'`);
  const scale = rep.pixelsWide / w;
  const at = (px, py) => {
    const c = rep.colorAtXY(Math.round(px * scale), Math.round(py * scale)).colorUsingColorSpace($.NSColorSpace.sRGBColorSpace);
    return [c.redComponent, c.greenComponent, c.blueComponent];
  };
  const far = (a, b) => Math.abs(a[0] - b[0]) + Math.abs(a[1] - b[1]) + Math.abs(a[2] - b[2]) > 0.6;
  const bg = at(1, 1);
  const centre = far(at(w / 2, h / 2), bg);
  const side = far(at(w / 2 + 3.5, h / 2), bg);
  if (centre && side) return "ok";
  if (!centre && !side) return "offline";
  if (centre) return "busy";
  return "warning";
}

function run(argv) {
  const [cmd, ...args] = argv;
  let result;
  switch (cmd) {
    case "menu": {
      result = readMenu(openMenu());
      closeMenu();
      break;
    }
    case "menu-click": {
      menuItemAt(openMenu(), args).actions.byName("AXPress").perform();
      delay(0.2);
      closeMenu();
      result = { clicked: args };
      break;
    }
    case "windows": {
      result = activate().windows().map((w, index) => ({ index, title: str(() => w.name()), main: isMain(w) }));
      break;
    }
    case "frame": {
      const w = mainWindow();
      return w.position().concat(w.size()).join(",");
    }
    case "tree": {
      result = tree(args[0] === "other" ? otherWindow() : mainWindow());
      break;
    }
    case "type": {
      const [name, text] = args;
      const field = findNamed(name, ["AXTextField", "AXTextArea"]);
      // Focused through accessibility: a field scrolled out of view cannot be clicked.
      try { field.focused = true; } catch (err) {}
      delay(0.2);
      if (!str(() => field.focused())) {
        clickCentre(field);
        delay(0.2);
      }
      se.keystroke("a", { using: "command down" });
      se.keyCode(51);
      if (text) se.keystroke(text);
      delay(0.2);
      result = { typed: name };
      break;
    }
    case "windows-of": {
      const p = se.processes.whose({ name: args[0] });
      result = p.length ? p[0].windows().map((w) => str(() => w.name())) : [];
      break;
    }
    case "close-window": {
      const p = se.processes.whose({ name: args[0] });
      const w = p.length ? p[0].windows().find((x) => str(() => x.name()) === args[1]) : null;
      if (w) w.buttons().find((b) => str(() => b.subrole()) === "AXCloseButton").actions.byName("AXPress").perform();
      result = { closed: !!w };
      break;
    }
    case "goto": {
      const p = activate();
      for (let i = 0; i < 30 && !p.windows().some((w) => str(() => w.name()) === "Open"); i++) delay(0.1);
      if (!p.windows().some((w) => str(() => w.name()) === "Open")) throw new Error("no open panel");
      se.keystroke("g", { using: ["command down", "shift down"] });
      delay(0.8);
      se.keystroke(args[0]);
      delay(0.4);
      se.keyCode(36);
      delay(0.8);
      se.keyCode(36);
      delay(0.8);
      result = { chose: args[0] };
      break;
    }
    case "key": {
      activate();
      se.keyCode(KEYS[args[0]]);
      delay(0.3);
      result = { key: args[0] };
      break;
    }
    case "pick": {
      const [name, option] = args;
      press(findNamed(name, ["AXPopUpButton", "AXComboBox"]));
      delay(0.4);
      press(findNamed(option, ["AXMenuItem", "AXButton", "AXStaticText"]));
      delay(0.4);
      result = { picked: option, in: name };
      break;
    }
    case "tab": {
      const w = mainWindow();
      let group = screens(w);
      for (let i = 0; i < 10 && !group; i++) {
        delay(0.2);
        group = screens(w);
      }
      if (!group) throw new Error("the main window shows no screens");
      const tab = group.radioButtons.byName(args[0]);
      tab.actions.byName("AXPress").perform();
      delay(0.5);
      result = { tab: args[0] };
      break;
    }
    case "press": {
      const [name, row] = args;
      press(findNamed(name, PRESSABLE, row));
      delay(0.3);
      result = { pressed: name, row: row || null };
      break;
    }
    case "row": {
      const w = mainWindow();
      const r = w.entireContents().find((e) => isRow(e) && str(() => e.name()) === args[0]);
      if (!r) throw new Error("no row named " + args[0]);
      r.actions.byName("AXPress").perform();
      delay(0.5);
      result = { row: args[0] };
      break;
    }
    case "dialog": {
      const d = openDialog();
      result = d && {
        text: d.entireContents().filter((e) => str(() => e.role()) === "AXStaticText").map((e) => str(() => e.value())),
        buttons: d.buttons().map((b) => str(() => b.name())),
      };
      break;
    }
    case "answer": {
      const d = openDialog();
      if (!d) throw new Error("no prompt is open");
      d.buttons.byName(args[0]).actions.byName("AXPress").perform();
      delay(0.3);
      result = { answered: args[0] };
      break;
    }
    case "icon": {
      return iconShape();
    }
    case "menu-log": {
      result = menuLog(args[0], Number(args[1]));
      break;
    }
    case "watch": {
      const [ready, timeout, target, op, text] = args;
      result = watch(ready, Number(timeout), target, op, text);
      break;
    }
    default:
      throw new Error("unknown command " + cmd);
  }
  return JSON.stringify(result);
}

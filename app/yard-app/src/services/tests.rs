//! Smoke tests for the Services screen.

use crate::env::AppEnv;
use crate::runner::ActionId;
use crate::testing::{env, services};
use gpui::{Context, IntoElement, Render, TestAppContext, Window};
use yard_proto::ErrorCode;

struct Host(AppEnv);

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::services(&self.0, window, cx)
    }
}

#[gpui::test]
async fn services_table_renders_every_state(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    env.state.update(cx, |s, _| s.services.value = serde_json::from_value(services()).ok());
    let rows = cx.update(|cx| super::table::row_texts(&env, cx));
    assert_eq!(rows[0], ["redis", "Redis", "running", "6380", "15m", "yard", "blog"]);
    assert_eq!(rows[1][2], "failed");
    assert_eq!(rows[2][5], "LaunchAgent");
    let (_v, cx) = cx.add_window_view(|_, _| Host(env));
    cx.run_until_parked();
}

#[gpui::test]
async fn remove_is_refused_locally_while_in_use(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    env.state.update(cx, |s, _| s.services.value = serde_json::from_value(services()).ok());
    let refused = cx.update(|cx| super::table::refuse_in_use(&env, "redis", cx));
    assert!(refused);
    let e = env
        .runner
        .read_with(cx, |r, _| r.errors.get(&ActionId("service:redis:remove".into())).cloned())
        .unwrap();
    assert_eq!(e.code, ErrorCode::ServiceInUse);
    assert_eq!(e.message, "redis is used by blog");
    assert_eq!(fake.count("services.remove"), 0);
    assert!(!cx.update(|cx| super::table::refuse_in_use(&env, "mysql-8.4", cx)), "nobody uses it");
}

/// Remove asks whether to delete the data too (design §11.3; M6 final review: it always deleted).
#[test]
fn remove_asks_whether_to_keep_the_data() {
    let (title, buttons) = super::table::remove_question("postgres-17");
    assert_eq!(title, "Delete postgres-17 and its data?");
    assert_eq!(buttons, ["Delete", "Remove, keep its data", "Cancel"]);
    assert_eq!(super::table::keep_data(0), Some(false));
    assert_eq!(super::table::keep_data(1), Some(true));
    assert_eq!(super::table::keep_data(2), None);
}

/// Copy .env copies what `yard env` prints: an instance that is not added is the one note that
/// says so and how to add it (found by the checklist script: the app copied a bare `# postgres-17`).
#[test]
fn copy_env_matches_yard_env() {
    let blocks: Vec<yard_proto::EnvBlock> = serde_json::from_value(serde_json::json!([
        {"instance": "postgres-17", "lines": [], "note": "postgres-17 is not added: yard service add postgres --version 17"},
        {"instance": "redis", "lines": ["REDIS_HOST=127.0.0.1", "REDIS_PORT=6380"], "note": null}
    ]))
    .unwrap();
    assert_eq!(
        super::table::env_text(&blocks),
        "# postgres-17 is not added: yard service add postgres --version 17\n\n# redis\nREDIS_HOST=127.0.0.1\nREDIS_PORT=6380\n"
    );
}

/// A failed instance says why on the Services screen, holder and fix included, as `yard services`
/// does under its table (found by the checklist script: the row said only `failed`).
#[gpui::test]
async fn a_failed_instance_says_why(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    env.state.update(cx, |s, _| s.services.value = serde_json::from_value(services()).ok());
    let failures = cx.update(|cx| super::failures(&env, cx));
    assert_eq!(failures.len(), 1);
    let (name, e) = &failures[0];
    assert_eq!(name, "mysql-8.4");
    assert!(e.message.contains("held by mysqld (pid 9)"), "{}", e.message);
    assert_eq!(e.hint.as_deref(), Some("yard service port mysql-8.4 <port>"));
}

/// A port field that is not a port says so beside the sheet, which stays open, and nothing is
/// sent (M6 final review: Move did nothing, and Add dropped the port and took an auto one).
#[gpui::test]
async fn a_port_that_is_not_a_port_says_so(cx: &mut TestAppContext) {
    use crate::env::Sheet;
    use crate::widgets::text_field::TextField;
    use gpui::AppContext;
    let (fake, env) = env(cx);
    let field = |text: &str, cx: &mut TestAppContext| {
        let f = cx.new(|cx| TextField::new("f", "Port", "", cx));
        let text = text.to_string();
        f.update(cx, |f, cx| f.set_value(text, cx));
        f
    };
    let error = |id: &str, cx: &mut TestAppContext| {
        env.runner.read_with(cx, |r, _| r.errors.get(&ActionId(id.into())).map(|e| e.message.clone()))
    };
    let port = field("abc", cx);
    env.nav.update(cx, |n, _| n.sheet = Some(Sheet::Port { name: "redis".into(), port: port.clone() }));
    cx.update(|cx| super::forms::move_service(&env, "redis", &port, cx));
    assert_eq!(
        error("service:redis:port", cx).as_deref(),
        Some("abc is not a port: give a number from 1 to 65535")
    );
    assert!(env.nav.read_with(cx, |n, _| n.sheet.is_some()), "the sheet stays open");

    let port = field("0", cx);
    cx.update(|cx| super::forms::move_unit(&env, "nginx-http", &port, cx));
    assert_eq!(
        error("port:nginx-http:set", cx).as_deref(),
        Some("0 is not a port: give a number from 1 to 65535")
    );

    let (line, port) = (field("", cx), field("33o7", cx));
    cx.update(|cx| super::forms::add_service(&env, 2, &line, &port, cx));
    assert_eq!(
        error("service:add", cx).as_deref(),
        Some("33o7 is not a port: give a number from 1 to 65535")
    );
    cx.run_until_parked();
    assert_eq!(fake.count("services.port") + fake.count("ports.set") + fake.count("services.add"), 0);
}

fn services_on(env: &AppEnv, selected: Option<&str>, cx: &mut TestAppContext) {
    env.state.update(cx, |s, _| s.services.value = serde_json::from_value(services()).ok());
    let screen = yard_app_model::hint::Screen::Services(selected.map(str::to_string));
    env.nav.update(cx, |n, _| n.screen = screen);
}

/// A row keeps one control; Port…, Sleep after…, Remove… and the rest are in the selected
/// instance's detail, with their ids as before (spec §4: 8 buttons on one row).
#[gpui::test]
async fn service_actions_live_in_the_detail(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    services_on(&env, None, cx);
    let (_v, vcx) = cx.add_window_view(|_, _| Host(env.clone()));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("service:redis:toggle").is_some(), "the row keeps Stop");
    assert!(vcx.debug_bounds("service:redis:port").is_none(), "Port… waits for a selection");
    vcx.update(|_, cx| {
        env.nav.update(cx, |n, cx| {
            n.screen = yard_app_model::hint::Screen::Services(Some("redis".into()));
            cx.notify();
        })
    });
    vcx.run_until_parked();
    for id in ["service:redis:port", "service:redis:sleep", "service:redis:remove", "service:redis:restart"] {
        assert!(vcx.debug_bounds(id).is_some(), "{id} is in the detail");
    }
    assert!(vcx.debug_bounds("service:mysql-8.4:port").is_none(), "only the selected instance's");
}

/// A selected instance that is gone leaves no detail acting on nothing.
#[gpui::test]
async fn a_removed_selection_falls_back_to_none(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    services_on(&env, Some("gone"), cx);
    let (_v, vcx) = cx.add_window_view(|_, _| Host(env.clone()));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("service:gone:detail").is_none());
    assert!(vcx.debug_bounds("service:redis:toggle").is_some(), "the table still draws");
}

/// A removed instance is no longer selected, so one added later under its name is not either
/// (final review).
#[gpui::test]
async fn a_removed_instance_is_no_longer_selected(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    services_on(&env, Some("redis"), cx);
    fake.answer("services.remove", serde_json::json!({}));
    cx.update(|cx| super::table::remove(&env, "redis".into(), false, cx));
    cx.run_until_parked();
    let screen = env.nav.read_with(cx, |n, _| n.screen.clone());
    assert_eq!(screen, yard_app_model::hint::Screen::Services(None));
}

/// A remove that fails keeps the selection, beside its error.
#[gpui::test]
async fn a_failed_remove_keeps_the_selection(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    services_on(&env, Some("redis"), cx);
    cx.update(|cx| super::table::remove(&env, "redis".into(), false, cx));
    cx.run_until_parked();
    let screen = env.nav.read_with(cx, |n, _| n.screen.clone());
    assert_eq!(screen, yard_app_model::hint::Screen::Services(Some("redis".into())));
}

// --- Databases, Connect, Create, Dump and Import (db spec §10) ---------------------------------

use crate::env::Sheet;
use gpui::AppContext;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use yard_app_model::refresh::ResourceKey;

/// MySQL and PostgreSQL running, a MySQL asleep, and Redis.
fn db_services() -> serde_json::Value {
    let instance = |name: &str, engine: &str, version: Option<&str>, port: u16, state: serde_json::Value| {
        json!({"name": name, "engine": engine, "version": version, "formula": engine, "ports": [{"name": name, "port": port}],
               "state": state, "stopped_by_user": false, "account": {"user": "root", "password": null}, "used_by": [],
               "sleep_after_secs": 900})
    };
    let mut asleep = instance("mysql-8.0", "mysql", Some("8.0"), 3308, json!("stopped"));
    asleep["sleeping_since"] = json!("2026-11-15T10:00:00Z");
    json!([
        instance("mysql-8.4", "mysql", Some("8.4"), 3307, json!("running")),
        instance("postgres-17", "postgres", Some("17"), 5433, json!("running")),
        asleep,
        instance("redis", "redis", None, 6380, json!("running")),
    ])
}

fn db_list(instance: &str, names: &[&str]) -> serde_json::Value {
    let dbs: Vec<_> = names.iter().map(|n| json!({"name": n, "size_bytes": 3145728, "tables": 12})).collect();
    json!([{"instance": instance, "engine": "mysql", "databases": dbs}])
}

fn connect_info(instance: &str) -> serde_json::Value {
    json!([{"instance": instance, "engine": "mysql", "state": "running", "sleeping": false, "host": "127.0.0.1",
            "port": 3307, "user": "root", "password": null, "socket": "/Users/me/.yard/run/mysql-8.4.sock",
            "url": "mysql://root@127.0.0.1:3307"}])
}

fn on_instance(env: &AppEnv, name: &str, cx: &mut TestAppContext) {
    env.state.update(cx, |s, _| s.services.value = serde_json::from_value(db_services()).ok());
    env.nav.update(cx, |n, _| n.screen = yard_app_model::hint::Screen::Services(Some(name.into())));
}

/// Lets the state's 150 ms coalescing pass, so what was asked for is fetched.
fn settle(cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
}

struct SheetHost(AppEnv);

impl Render for SheetHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet = self.0.nav.read(cx).sheet.clone();
        let drawn = sheet.and_then(|s| super::db::sheet(&self.0, &s, window, cx));
        use gpui::{ParentElement, Styled};
        gpui::div().size_full().relative().children(drawn)
    }
}

/// `debug_bounds` takes a `'static` selector.
fn sel(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn error_of(env: &AppEnv, id: &str, cx: &mut TestAppContext) -> Option<String> {
    env.runner.read_with(cx, |r, _| r.errors.get(&ActionId(id.into())).map(|e| e.message.clone()))
}

/// Connect… asks yardd without waking anything and shows each field of `yard db connect` with a
/// Copy button that says what it copies, Copy URL and Open.
#[gpui::test]
async fn connect_sheet_shows_every_field(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    on_instance(&env, "mysql-8.4", cx);
    fake.answer("db.connect", connect_info("mysql-8.4"));
    cx.update(|cx| super::db::connect(&env, "mysql-8.4", cx));
    cx.run_until_parked();
    let (_, params) = fake.calls().into_iter().find(|(m, _)| m == "db.connect").unwrap();
    assert_eq!(params, json!({"service": "mysql-8.4", "wake": false}), "Connect… wakes nothing");
    let info = env.nav.read_with(cx, |n, _| match &n.sheet {
        Some(Sheet::Connect { info: Some(info), .. }) => info.clone(),
        _ => panic!("the Connect sheet with yardd's answer"),
    });
    let fields = yard_app_model::db::connect_fields(&info);
    let shown: Vec<&str> = fields.iter().map(|f| f.value.as_str()).collect();
    assert_eq!(
        shown,
        [
            "127.0.0.1",
            "3307",
            "root",
            "(none)",
            "/Users/me/.yard/run/mysql-8.4.sock",
            "mysql://root@127.0.0.1:3307"
        ]
    );
    assert!(fields.iter().all(|f| f.description.starts_with("Copy the ")), "each Copy says what it copies");
    let (_v, vcx) = cx.add_window_view(|_, _| SheetHost(env.clone()));
    vcx.run_until_parked();
    for key in ["host", "port", "user", "password", "socket", "url"] {
        assert!(vcx.debug_bounds(sel(format!("sheet:connect:{key}:copy"))).is_some(), "{key} has Copy");
    }
    for id in ["sheet:connect:copy-url", "sheet:connect:submit", "sheet:connect"] {
        assert!(vcx.debug_bounds(id).is_some(), "{id}");
    }
}

/// A selected MySQL instance lists its databases as a list of items named for each, with Open
/// and Dump… on each row; Redis has only Connect….
#[gpui::test]
async fn databases_list_reads_aloud(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    on_instance(&env, "mysql-8.4", cx);
    fake.answer("db.list", db_list("mysql-8.4", &["blog", "shop"]));
    env.state.update(cx, |s, cx| s.refresh([ResourceKey::Databases("mysql-8.4".into())], cx));
    settle(cx);
    let (_, params) = fake.calls().into_iter().find(|(m, _)| m == "db.list").unwrap();
    assert_eq!(params, json!({"service": "mysql-8.4"}));
    let dbs: Vec<yard_proto::DbInfo> =
        serde_json::from_value(db_list("mysql-8.4", &["blog", "shop"])[0]["databases"].clone()).unwrap();
    let table = super::db::databases_table(&env, "mysql-8.4", &dbs);
    assert_eq!(table.a11y().role, gpui::Role::List, "VoiceOver walks a list; a table reads as empty");
    let keys: Vec<String> = table.row_ids().into_iter().map(|(k, _)| k.to_string()).collect();
    assert_eq!(keys, ["blog", "shop"]);
    let (_v, vcx) = cx.add_window_view(|_, _| Host(env.clone()));
    vcx.run_until_parked();
    for db in ["blog", "shop"] {
        for id in [
            format!("service:mysql-8.4:databases:row:{db}"),
            format!("service:mysql-8.4:db:{db}:open"),
            format!("service:mysql-8.4:db:{db}:dump"),
        ] {
            assert!(vcx.debug_bounds(sel(id.clone())).is_some(), "{id}");
        }
    }
    for id in ["service:mysql-8.4:create-db", "service:mysql-8.4:import", "service:mysql-8.4:connect"] {
        assert!(vcx.debug_bounds(id).is_some(), "{id}");
    }
    vcx.update(|_, cx| {
        env.nav.update(cx, |n, cx| {
            n.screen = yard_app_model::hint::Screen::Services(Some("redis".into()));
            cx.notify();
        })
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("service:redis:connect").is_some(), "Redis has Connect…");
    assert!(vcx.debug_bounds("service:redis:create-db").is_none(), "and no databases");
}

/// Listing wakes an instance, so a sleeping one is listed only when asked (List databases), and
/// a Redis selection asks for nothing.
#[gpui::test]
async fn a_sleeping_instance_is_listed_only_when_asked(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    on_instance(&env, "mysql-8.0", cx);
    fake.answer("db.list", db_list("mysql-8.0", &["legacy"]));
    env.state.update(cx, |s, cx| {
        s.refresh([ResourceKey::Databases("mysql-8.0".into()), ResourceKey::Databases("redis".into())], cx)
    });
    settle(cx);
    assert_eq!(fake.count("db.list"), 0, "nothing woke it");
    let (_v, vcx) = cx.add_window_view(|_, _| Host(env.clone()));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("service:mysql-8.0:databases:list").is_some(), "List databases offered");
    vcx.update(|_, cx| super::db::list(&env, "mysql-8.0", cx));
    vcx.run_until_parked();
    assert_eq!(fake.count("db.list"), 1);
    assert!(vcx.debug_bounds("service:mysql-8.0:databases:row:legacy").is_some());
}

/// A list asked for before its instance was up is fetched once the services list says it runs.
#[gpui::test]
async fn a_list_waits_for_its_instance_to_run(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    on_instance(&env, "mysql-8.0", cx);
    fake.answer("db.list", db_list("mysql-8.0", &["legacy"]));
    env.state.update(cx, |s, cx| s.refresh([ResourceKey::Databases("mysql-8.0".into())], cx));
    settle(cx);
    assert_eq!(fake.count("db.list"), 0);
    let mut awake = db_services();
    awake[2]["state"] = json!("running");
    awake[2]["sleeping_since"] = serde_json::Value::Null;
    fake.answer("services.list", awake);
    env.state.update(cx, |s, cx| s.refresh([ResourceKey::Services], cx));
    settle(cx);
    assert_eq!(fake.count("db.list"), 1, "fetched once it runs");
}

/// The Dump… sheet runs `yard db dump` with exactly the sheet's choices; its progress reaches
/// Activity and the sheet closes once the dump runs.
#[gpui::test]
async fn dump_runs_the_cli(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let yard = stub_yard(
        dir.path(),
        r#"echo '{"bytes":1048576,"total":null}' >&2
printf '%s' '{"instance":"mysql-8.4","name":"shop","file":"/tmp/x.sql.gz","bytes":2097152}'"#,
    );
    let (_fake, env) = env(cx);
    env.cli.set_yard(Some(yard.clone()));
    on_instance(&env, "mysql-8.4", cx);
    let extra =
        cx.new(|cx| crate::widgets::text_field::TextField::new("sheet:dump:extra", "Extra options", "", cx));
    extra.update(cx, |f, cx| f.set_value("--routines", cx));
    env.nav.update(cx, |n, _| {
        n.sheet = Some(Sheet::Dump {
            name: "mysql-8.4".into(),
            database: "shop".into(),
            file: PathBuf::from("/tmp/x.sql.gz"),
            overwrite: false,
            compress: true,
            extra,
            starting: false,
        })
    });
    cx.update(|cx| super::db::confirm_dump(&env, cx));
    wait_for(cx, |cx| env.state.read_with(cx, |s, _| s.transfers.running().count() == 0));
    let args = recorded_args(&yard);
    assert_eq!(
        args[3..],
        [
            "db",
            "dump",
            "shop",
            "/tmp/x.sql.gz",
            "--compress",
            "--progress-json",
            "--service",
            "mysql-8.4",
            "--",
            "--routines"
        ]
    );
    assert_eq!(args[..3], ["--root", "/tmp/yard-test-root", "--json"]);
    let record = env.state.read_with(cx, |s, _| s.transfers.all()[0].clone());
    assert_eq!(record.title, "Dump shop from mysql-8.4");
    assert_eq!(record.phase, yard_app_model::jobs::JobPhase::Succeeded(None));
    assert_eq!(record.progress.map(|p| p.bytes), Some(1048576));
    assert_eq!(record.lines, ["Dumped shop from mysql-8.4 to /tmp/x.sql.gz (2.0 MB)."]);
    assert!(env.nav.read_with(cx, |n, _| n.sheet.is_none()), "the sheet closed");
}

/// Extra options with an unclosed quote stay in the sheet as its error; nothing runs.
#[gpui::test]
async fn bad_extra_options_stay_in_the_sheet(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    on_instance(&env, "mysql-8.4", cx);
    let extra =
        cx.new(|cx| crate::widgets::text_field::TextField::new("sheet:dump:extra", "Extra options", "", cx));
    extra.update(cx, |f, cx| f.set_value("--where='x", cx));
    env.nav.update(cx, |n, _| {
        n.sheet = Some(Sheet::Dump {
            name: "mysql-8.4".into(),
            database: "shop".into(),
            file: PathBuf::from("/tmp/x.sql"),
            overwrite: false,
            compress: false,
            extra,
            starting: false,
        })
    });
    cx.update(|cx| super::db::confirm_dump(&env, cx));
    cx.run_until_parked();
    assert_eq!(
        error_of(&env, "service:mysql-8.4:dump", cx).as_deref(),
        Some("Extra options: a quote is not closed")
    );
    assert!(env.nav.read_with(cx, |n, _| matches!(n.sheet, Some(Sheet::Dump { starting: false, .. }))));
    assert!(env.state.read_with(cx, |s, _| s.transfers.all().is_empty()), "nothing ran");
    let (_v, vcx) = cx.add_window_view(|_, _| SheetHost(env.clone()));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("sheet:dump").is_some());
}

/// A dump the CLI or yardd refuses (a reserved flag) says so in the sheet, which stays open
/// and can be sent again (spec §10: §7's refusals show in the sheet).
#[gpui::test]
async fn a_refused_dump_stays_in_the_sheet(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let yard = stub_yard(
        dir.path(),
        r#"printf '%s' '{"error":{"code":"DB_FLAG_RESERVED","message":"--host names another server: Yard picks the server","hint":null}}'
exit 1"#,
    );
    let (_fake, env) = env(cx);
    env.cli.set_yard(Some(yard));
    on_instance(&env, "mysql-8.4", cx);
    cx.update(|cx| {
        super::db::open_dump_sheet(&env, "mysql-8.4", "shop", PathBuf::from("/nonexistent/x.sql"), cx)
    });
    cx.update(|cx| super::db::confirm_dump(&env, cx));
    wait_for(cx, |cx| env.state.read_with(cx, |s, _| s.transfers.running().count() == 0));
    assert_eq!(
        error_of(&env, "service:mysql-8.4:dump", cx).as_deref(),
        Some("--host names another server: Yard picks the server")
    );
    assert!(env.nav.read_with(cx, |n, _| matches!(n.sheet, Some(Sheet::Dump { starting: false, .. }))));
}

/// Cancel sends the CLI Ctrl-C; the dump ends cancelled, with no error left behind.
#[gpui::test]
async fn cancel_stops_a_dump(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let yard = stub_yard(
        dir.path(),
        r#"trap 'exit 130' INT
echo '{"bytes":10,"total":null}' >&2
while :; do sleep 0.05; done"#,
    );
    let (_fake, env) = env(cx);
    env.cli.set_yard(Some(yard));
    on_instance(&env, "mysql-8.4", cx);
    cx.update(|cx| {
        super::db::open_dump_sheet(&env, "mysql-8.4", "shop", PathBuf::from("/nonexistent/x.sql"), cx)
    });
    cx.update(|cx| super::db::confirm_dump(&env, cx));
    wait_for(cx, |cx| env.nav.read_with(cx, |n, _| n.sheet.is_none()));
    let id = env.state.read_with(cx, |s, _| s.transfers.all()[0].id);
    let (_v, vcx) = cx.add_window_view(|_, _| ActivityHost(env.clone()));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds(sel(format!("transfer:{id}:cancel"))).is_some(), "Activity offers Cancel");
    vcx.update(|_, cx| env.state.update(cx, |s, _| s.interrupt(id)));
    wait_for(cx, |cx| env.state.read_with(cx, |s, _| s.transfers.running().count() == 0));
    let phase = env.state.read_with(cx, |s, _| s.transfers.all()[0].phase.clone());
    assert_eq!(phase, yard_app_model::jobs::JobPhase::Cancelled);
    assert_eq!(error_of(&env, "service:mysql-8.4:dump", cx), None);
}

/// Import… runs `yard db import` with the name and switches the sheet holds, and lists the
/// databases again afterwards; Keep owners is offered for PostgreSQL only.
#[gpui::test]
async fn import_runs_the_cli(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let yard = stub_yard(
        dir.path(),
        r#"printf '%s' '{"instance":"postgres-17","name":"app","file":"/tmp/app.dump","created":true,"bytes":10,"skipped":0}'"#,
    );
    let (fake, env) = env(cx);
    env.cli.set_yard(Some(yard.clone()));
    on_instance(&env, "postgres-17", cx);
    fake.answer("db.list", db_list("postgres-17", &["app"]));
    cx.update(|cx| super::db::open_import_sheet(&env, "postgres-17", PathBuf::from("/tmp/app.dump"), cx));
    let (_v, vcx) = cx.add_window_view(|_, _| SheetHost(env.clone()));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("sheet:import:keep-owners").is_some(), "PostgreSQL names owners");
    env.nav.update(vcx, |n, _| {
        if let Some(Sheet::Import { into_existing, keep_owners, .. }) = &mut n.sheet {
            (*into_existing, *keep_owners) = (true, true);
        }
    });
    vcx.update(|_, cx| super::db::confirm_import(&env, cx));
    wait_for(vcx, |cx| env.state.read_with(cx, |s, _| s.transfers.running().count() == 0));
    settle(vcx);
    assert_eq!(
        recorded_args(&yard)[3..],
        [
            "db",
            "import",
            "app",
            "/tmp/app.dump",
            "--into-existing",
            "--keep-owners",
            "--progress-json",
            "--service",
            "postgres-17"
        ]
    );
    assert_eq!(fake.count("db.list"), 1, "listed again after the import");

    vcx.update(|_, cx| super::db::open_import_sheet(&env, "mysql-8.4", PathBuf::from("/tmp/shop.sql"), cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("sheet:import:keep-owners").is_none(), "a MySQL dump has no owners");
}

/// Create database… sends `db.create` and closes; a name Yard does not take stays in the sheet.
#[gpui::test]
async fn create_database_sends_db_create(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    on_instance(&env, "mysql-8.4", cx);
    fake.answer("db.create", json!({"instance": "mysql-8.4", "name": "shop", "created": true}));
    fake.answer("db.list", db_list("mysql-8.4", &["shop"]));
    let field = cx.new(|cx| {
        crate::widgets::text_field::TextField::new("sheet:create-db:mysql-8.4", "Database name", "", cx)
    });
    let sheet = |f: &gpui::Entity<crate::widgets::text_field::TextField>| Sheet::CreateDatabase {
        name: "mysql-8.4".into(),
        database: f.clone(),
    };
    env.nav.update(cx, |n, _| n.sheet = Some(sheet(&field)));
    field.update(cx, |f, cx| f.set_value("my shop", cx));
    cx.update(|cx| super::db::create(&env, "mysql-8.4", &field, cx));
    cx.run_until_parked();
    assert!(error_of(&env, "service:mysql-8.4:create-db", cx).unwrap().contains("is not a database name"));
    assert_eq!(fake.count("db.create"), 0);
    assert!(env.nav.read_with(cx, |n, _| n.sheet.is_some()), "the sheet stays");
    field.update(cx, |f, cx| f.set_value("shop", cx));
    cx.update(|cx| super::db::create(&env, "mysql-8.4", &field, cx));
    settle(cx);
    let (_, params) = fake.calls().into_iter().find(|(m, _)| m == "db.create").unwrap();
    assert_eq!(params, json!({"service": "mysql-8.4", "name": "shop"}));
    assert!(env.nav.read_with(cx, |n, _| n.sheet.is_none()), "closed once created");
    assert_eq!(error_of(&env, "service:mysql-8.4:create-db", cx), None);
    assert_eq!(fake.count("db.list"), 1, "listed again");
}

/// Open on a row wakes the instance and opens its URL with the database as its path.
#[gpui::test]
async fn open_wakes_and_opens_the_database(cx: &mut TestAppContext) {
    let (fake, env) = env(cx);
    on_instance(&env, "mysql-8.4", cx);
    fake.answer("db.connect", connect_info("mysql-8.4"));
    cx.update(|cx| super::db::open(&env, "mysql-8.4", Some("shop".into()), cx));
    cx.run_until_parked();
    let (_, params) = fake.calls().into_iter().find(|(m, _)| m == "db.connect").unwrap();
    assert_eq!(params, json!({"service": "mysql-8.4", "wake": true}));
    assert_eq!(cx.opened_url().as_deref(), Some("mysql://root@127.0.0.1:3307/shop"));
}

/// The database actions' errors show on the Services screen, not in the strip above it.
#[gpui::test]
async fn database_errors_show_on_the_services_screen(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    on_instance(&env, "mysql-8.4", cx);
    for verb in ["databases", "create-db", "connect", "open-db", "dump", "import"] {
        env.runner.update(cx, |r, _| {
            r.errors
                .record(ActionId(format!("service:mysql-8.4:{verb}")), yard_proto::YardError::internal("x"))
        });
    }
    let screen = yard_app_model::hint::Screen::Services(Some("mysql-8.4".into()));
    let elsewhere = cx.update(|cx| crate::window::main::elsewhere_error_ids(&env, &screen, cx));
    assert!(elsewhere.is_empty(), "{elsewhere:?}");
}

/// Quitting while a dump runs asks first; Cancel leaves it running.
#[gpui::test]
async fn quitting_asks_while_a_dump_runs(cx: &mut TestAppContext) {
    let (_fake, env) = env(cx);
    env.state.update(cx, |s, _| {
        s.transfers.start("Dump shop from mysql-8.4".into(), time::OffsetDateTime::now_utc())
    });
    cx.update(|cx| env.quit(cx));
    cx.run_until_parked();
    let prompt = env.runner.read_with(cx, |r, _| r.prompt());
    assert!(
        matches!(&prompt, Some(crate::runner::Prompt::Confirm { title, .. }) if title == "Dump shop from mysql-8.4 is still running. Stop it and quit?"),
        "{prompt:?}"
    );
    env.runner.update(cx, |r, cx| r.answer(1, cx));
    cx.run_until_parked();
    assert_eq!(env.state.read_with(cx, |s, _| s.transfers.running().count()), 1, "still running");
}

struct ActivityHost(AppEnv);

impl Render for ActivityHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::activity::activity(&self.0, window, cx)
    }
}

/// A stand-in `yard` that writes its arguments beside itself, then runs `body`.
fn stub_yard(dir: &std::path::Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let yard = dir.join("yard");
    let script =
        format!("#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$0.args\"\n{body}\n");
    std::fs::write(&yard, script).unwrap();
    std::fs::set_permissions(&yard, std::fs::Permissions::from_mode(0o755)).unwrap();
    yard
}

fn recorded_args(yard: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(yard.with_extension("args")).unwrap().lines().map(str::to_string).collect()
}

/// Runs the app until `done`, for up to 10 s of real time: the CLI is a real process.
fn wait_for(cx: &mut TestAppContext, done: impl Fn(&mut TestAppContext) -> bool) {
    for _ in 0..200 {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out");
}

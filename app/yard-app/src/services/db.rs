//! A selected instance's databases (db spec §10): the Databases list with Open and Dump… on each
//! row, Create database…, Import… and Connect…, and their sheets. Listing, creating and the
//! connection details go through yardd; dumps and imports run the `yard` CLI, which moves the
//! bytes (the open and save panels give it the file), and show in Activity.

use crate::daemon::call;
use crate::env::{AppEnv, Sheet};
use crate::platform::open::Opener;
use crate::runner::ActionId;
use crate::widgets::button::Button;
use crate::widgets::error_view::ErrorView;
use crate::widgets::form::Form;
use crate::widgets::group::Group;
use crate::widgets::table::{Column, Row, Table};
use crate::widgets::text;
use crate::widgets::text_field::TextField;
use crate::widgets::theme::{Theme, Typography, space};
use crate::widgets::toggle::Toggle;
use gpui::{
    AnyElement, App, AppContext, Entity, IntoElement, ParentElement, PathPromptOptions, SharedString, Styled,
    Window, div,
};
use std::path::PathBuf;
use yard_app_model::db::{self as model, DumpRequest, ImportRequest};
use yard_app_model::hint::HintAction;
use yard_app_model::refresh::ResourceKey;
use yard_client::api;
use yard_proto::methods::{DbConnectParams, DbCreateParams, DbTarget};
use yard_proto::{DbInfo, ServiceInfo, ServiceState};

fn id(name: &str, verb: &str) -> String {
    format!("service:{name}:{verb}")
}

fn target(name: &str) -> DbTarget {
    DbTarget { path: None, site: None, service: Some(name.to_string()), engine: None }
}

fn error(env: &AppEnv, id: &str, cx: &App) -> Option<SharedString> {
    env.runner.read(cx).errors.get(&ActionId(id.into())).map(|e| SharedString::from(e.message.clone()))
}

fn record(env: &AppEnv, id: &str, e: yard_proto::YardError, cx: &mut App) {
    env.runner.update(cx, |r, cx| {
        r.errors.record(ActionId(id.into()), e);
        cx.notify();
    });
}

fn set_sheet(env: &AppEnv, sheet: Option<Sheet>, cx: &mut App) {
    env.nav.update(cx, |n, cx| {
        n.sheet = sheet;
        cx.notify();
    });
}

/// Changes the open sheet when `f` finds it is still the one it means.
fn update_sheet(env: &AppEnv, cx: &mut App, f: impl FnOnce(&mut Sheet) -> bool) {
    env.nav.update(cx, |n, cx| {
        if let Some(sheet) = n.sheet.as_mut()
            && f(sheet)
        {
            cx.notify();
        }
    });
}

fn field(id: String, label: &str, placeholder: &str, value: &str, cx: &mut App) -> Entity<TextField> {
    let value = value.to_string();
    cx.new(|cx| {
        let mut f = TextField::new(id, label.to_string(), placeholder.to_string(), cx);
        f.set_value(value, cx);
        f
    })
}

/// Connect…: opens its sheet and asks yardd for the details, without waking the instance.
pub fn connect(env: &AppEnv, name: &str, cx: &mut App) {
    set_sheet(env, Some(Sheet::Connect { name: name.to_string(), info: None }), cx);
    let (daemon, env2, n) = (env.daemon.clone(), env.clone(), name.to_string());
    let task = env.runner.update(cx, |r, cx| {
        r.perform(
            ActionId(id(name, "connect")),
            async move {
                let params = DbConnectParams { target: target(&n), wake: false };
                call(&*daemon, api::DB_CONNECT, params).await
            },
            cx,
        )
    });
    let n = name.to_string();
    cx.spawn(async move |cx| {
        let Some(info) = task.await.and_then(|v| v.into_iter().next()) else { return };
        cx.update(|cx| {
            update_sheet(&env2, cx, |s| match s {
                Sheet::Connect { name, info: shown } if *name == n => {
                    *shown = Some(Box::new(info));
                    true
                }
                _ => false,
            })
        });
    })
    .detach();
}

/// Open: wakes the instance (`db.connect` with `wake`) and opens its URL, with the database as
/// its path, in the app macOS has for it. Its error shows beside the instance and in Connect….
pub fn open(env: &AppEnv, name: &str, database: Option<String>, cx: &mut App) {
    let action = id(name, "open-db");
    let (daemon, n) = (env.daemon.clone(), name.to_string());
    let task = env.runner.update(cx, |r, cx| {
        r.perform(
            ActionId(action.clone()),
            async move {
                let params = DbConnectParams { target: target(&n), wake: true };
                call(&*daemon, api::DB_CONNECT, params).await
            },
            cx,
        )
    });
    let env = env.clone();
    cx.spawn(async move |cx| {
        let Some(info) = task.await.and_then(|v| v.into_iter().next()) else { return };
        let url = match &database {
            Some(db) => model::database_url(&info.url, db),
            None => info.url,
        };
        cx.update(|cx| {
            if let Err(e) = Opener::open_db_url(cx, &url) {
                record(&env, &action, e, cx);
            }
        });
    })
    .detach();
}

/// List databases: `db.list` as an action, which wakes a sleeping instance.
pub fn list(env: &AppEnv, name: &str, cx: &mut App) {
    let (daemon, n) = (env.daemon.clone(), name.to_string());
    let task = env.runner.update(cx, |r, cx| {
        r.perform(
            ActionId(id(name, "databases")),
            async move {
                let params = yard_proto::methods::DbListParams { target: target(&n) };
                call(&*daemon, api::DB_LIST, params).await
            },
            cx,
        )
    });
    let (state, n) = (env.state.clone(), name.to_string());
    cx.spawn(async move |cx| {
        if let Some(lists) = task.await {
            state.update(cx, |s, cx| s.put_databases(&n, lists, cx));
        }
    })
    .detach();
}

fn refresh_databases(env: &AppEnv, name: &str, cx: &mut App) {
    env.state.update(cx, |s, cx| s.refresh([ResourceKey::Databases(name.to_string())], cx));
}

/// Create database…: `db.create`; the sheet closes once it is done, and stays with the error.
pub fn create(env: &AppEnv, name: &str, field: &Entity<TextField>, cx: &mut App) {
    let action = id(name, "create-db");
    let database = match model::database_name(field.read(cx).value()) {
        Ok(d) => d,
        Err(e) => return record(env, &action, e, cx),
    };
    let (daemon, n) = (env.daemon.clone(), name.to_string());
    let task = env.runner.update(cx, |r, cx| {
        r.perform(
            ActionId(action),
            async move {
                call(&*daemon, api::DB_CREATE, DbCreateParams { target: target(&n), name: database }).await
            },
            cx,
        )
    });
    let (env, n) = (env.clone(), name.to_string());
    cx.spawn(async move |cx| {
        if task.await.is_some() {
            cx.update(|cx| {
                close_if(&env, cx, |s| matches!(s, Sheet::CreateDatabase { name, .. } if *name == n));
                refresh_databases(&env, &n, cx);
            });
        }
    })
    .detach();
}

fn close_if(env: &AppEnv, cx: &mut App, f: impl FnOnce(&Sheet) -> bool) {
    env.nav.update(cx, |nav, cx| {
        if nav.sheet.as_ref().is_some_and(f) {
            nav.sheet = None;
            cx.notify();
        }
    });
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Dump…: a save panel for the file, then the sheet with Compress and Extra options.
pub fn dump(env: &AppEnv, name: &str, database: &str, cx: &mut App) {
    let picked = cx.prompt_for_new_path(&home(), Some(&format!("{database}.sql")));
    let (env, name, database) = (env.clone(), name.to_string(), database.to_string());
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(file))) = picked.await else { return };
        cx.update(|cx| open_dump_sheet(&env, &name, &database, file, cx));
    })
    .detach();
}

/// The Dump… sheet for `file`; a file that is there was already offered for replacing by the
/// save panel, so the dump replaces it.
pub fn open_dump_sheet(env: &AppEnv, name: &str, database: &str, file: PathBuf, cx: &mut App) {
    let engine = engine_of(env, name, cx);
    let tool = if engine == "postgres" { "pg_dump" } else { "mysqldump" };
    let extra = field(
        "sheet:dump:extra".into(),
        "Extra options",
        &format!("passed to {tool}, e.g. --single-transaction --routines"),
        "",
        cx,
    );
    let overwrite = file.exists();
    let sheet = Sheet::Dump {
        name: name.to_string(),
        database: database.to_string(),
        file,
        overwrite,
        compress: false,
        extra,
        starting: false,
    };
    set_sheet(env, Some(sheet), cx);
}

fn engine_of(env: &AppEnv, name: &str, cx: &App) -> String {
    let services = env.state.read(cx).services.value.clone().unwrap_or_default();
    services.into_iter().find(|s| s.name == name).map(|s| s.engine).unwrap_or_default()
}

/// The Dump… sheet's Dump: Extra options that do not split stay in the sheet as its error;
/// otherwise the CLI runs, and the sheet closes once the dump is under way.
pub fn confirm_dump(env: &AppEnv, cx: &mut App) {
    let Some(Sheet::Dump { name, database, file, overwrite, compress, extra, starting: false }) =
        env.nav.read(cx).sheet.clone()
    else {
        return;
    };
    let request = DumpRequest {
        instance: name.clone(),
        database: database.clone(),
        file: file.display().to_string(),
        compress,
        overwrite,
        extra: extra.read(cx).value().to_string(),
    };
    let action = id(&name, "dump");
    let args = match model::dump_args(&request) {
        Ok(args) => args,
        Err(e) => return record(env, &action, e, cx),
    };
    let title = format!("Dump {database} from {name}");
    run(env, &name, &action, title, args, cx);
}

/// Import…: an open panel for the dump (a file, or pg_dump -Fd's folder), then its sheet.
pub fn import(env: &AppEnv, name: &str, cx: &mut App) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: true,
        multiple: false,
        prompt: Some("Import".into()),
    });
    let (env, name) = (env.clone(), name.to_string());
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = picked.await else { return };
        let Some(file) = paths.into_iter().next() else { return };
        cx.update(|cx| open_import_sheet(&env, &name, file, cx));
    })
    .detach();
}

pub fn open_import_sheet(env: &AppEnv, name: &str, file: PathBuf, cx: &mut App) {
    let engine = engine_of(env, name, cx);
    let tools = if engine == "postgres" { "psql or pg_restore" } else { "mysql" };
    let suggested = model::suggested_name(&file.display().to_string());
    let database =
        field("sheet:import:name".into(), "Database", "the database to import into", &suggested, cx);
    let extra = field("sheet:import:extra".into(), "Extra options", &format!("passed to {tools}"), "", cx);
    let sheet = Sheet::Import {
        name: name.to_string(),
        engine,
        file,
        database,
        into_existing: false,
        keep_owners: false,
        extra,
        starting: false,
    };
    set_sheet(env, Some(sheet), cx);
}

pub fn confirm_import(env: &AppEnv, cx: &mut App) {
    let Some(Sheet::Import {
        name, file, database, into_existing, keep_owners, extra, starting: false, ..
    }) = env.nav.read(cx).sheet.clone()
    else {
        return;
    };
    let request = ImportRequest {
        instance: name.clone(),
        database: database.read(cx).value().to_string(),
        file: file.display().to_string(),
        into_existing,
        keep_owners,
        extra: extra.read(cx).value().to_string(),
    };
    let action = id(&name, "import");
    let args = match model::import_args(&request) {
        Ok(args) => args,
        Err(e) => return record(env, &action, e, cx),
    };
    let title = format!("Import {} into {} on {name}", file.display(), args[2]);
    run(env, &name, &action, title, args, cx);
}

/// Runs a dump or import: the sheet waits, closes once the tool is running (so a refusal from
/// the CLI or yardd shows in it), and the databases are listed again after an import.
fn run(env: &AppEnv, name: &str, action: &str, title: String, args: Vec<String>, cx: &mut App) {
    let set_starting = |env: &AppEnv, on: bool, cx: &mut App| {
        update_sheet(env, cx, |s| match s {
            Sheet::Dump { starting, .. } | Sheet::Import { starting, .. } => {
                *starting = on;
                true
            }
            _ => false,
        })
    };
    set_starting(env, true, cx);
    let n = name.to_string();
    let close_env = env.clone();
    let started = move |cx: &mut App| {
        close_if(
            &close_env,
            cx,
            |s| matches!(s, Sheet::Dump { name, starting: true, .. } | Sheet::Import { name, starting: true, .. } if *name == n),
        )
    };
    let task = env.run_transfer(action, title, args, started, cx);
    let (env, n, import) = (env.clone(), name.to_string(), action.ends_with(":import"));
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            if result.is_err() {
                set_starting(&env, false, cx);
            }
            if import {
                refresh_databases(&env, &n, cx);
            }
        });
    })
    .detach();
}

/// The Databases list as a table (an AccessKit list of items named for each database), with
/// Open and Dump… on each row.
pub fn databases_table(env: &AppEnv, name: &str, databases: &[DbInfo]) -> Table {
    let rows = databases.iter().map(|d| {
        let (e1, n1, d1) = (env.clone(), name.to_string(), d.name.clone());
        let (e2, n2, d2) = (env.clone(), name.to_string(), d.name.clone());
        Row::new(
            d.name.clone(),
            vec![d.name.clone().into(), model::bytes_text(d.size_bytes).into(), d.tables.to_string().into()],
        )
        .extra(
            Button::new(format!("service:{name}:db:{}:open", d.name), "Open")
                .description(format!("Open {} in the app for its URL, waking {name}", d.name))
                .small()
                .on_click(move |_, _, cx| open(&e1, &n1, Some(d1.clone()), cx)),
        )
        .extra(
            Button::new(format!("service:{name}:db:{}:dump", d.name), "Dump…")
                .description(format!("Dump {} to a file", d.name))
                .small()
                .on_click(move |_, _, cx| dump(&e2, &n2, &d2, cx)),
        )
    });
    Table::new(
        format!("service:{name}:databases"),
        [Column::flex("Database"), Column::fixed("Size", 90.).end(), Column::fixed("Tables", 70.).end()],
        rows.collect(),
    )
    .actions(130.)
    .empty("No databases yet. Create one, or import a dump.", None)
}

/// What stands in for the list while there is none: how to get one.
fn waiting(env: &AppEnv, s: &ServiceInfo, loading: bool) -> AnyElement {
    let name = &s.name;
    let words = match &s.state {
        ServiceState::Stopped if s.sleeping_since.is_some() => {
            format!("{name} is asleep; listing its databases wakes it.")
        }
        ServiceState::Stopped => format!("{name} is stopped. Start it to list its databases."),
        ServiceState::Starting => format!("{name} is starting…"),
        ServiceState::Failed { .. } => format!("{name} failed; it says why below."),
        _ if loading => "Listing databases…".to_string(),
        _ => String::new(),
    };
    let asleep = matches!(s.state, ServiceState::Stopped) && s.sleeping_since.is_some();
    let (env, n) = (env.clone(), name.clone());
    crate::widgets::bar()
        .child(text(format!("service:{name}:databases:waiting"), words))
        .children(asleep.then(|| {
            Button::new(format!("service:{name}:databases:list"), "List databases")
                .description(format!("Wake {n} and list its databases"))
                .small()
                .on_click(move |_, _, cx| list(&env, &n, cx))
        }))
        .into_any_element()
}

/// A MySQL or PostgreSQL instance's Databases section; `None` for other engines.
pub fn section(env: &AppEnv, s: &ServiceInfo, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    if !model::has_databases(&s.engine) || s.managed_by != yard_proto::ServiceManager::Yard {
        return None;
    }
    let t = Theme::of(window);
    let name = s.name.clone();
    let resource = env.state.read(cx).databases.get(&name).cloned().unwrap_or_default();
    let (e1, n1) = (env.clone(), name.clone());
    let (e2, n2) = (env.clone(), name.clone());
    let heading = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(space::SM)
        .child(div().type_section().child(text(format!("service:{name}:databases:title"), "Databases")))
        .child(
            crate::widgets::bar()
                .child(Button::new(format!("service:{name}:create-db"), "Create database…").small().on_click(
                    move |_, _, cx| {
                        let database = field(
                            format!("sheet:create-db:{n1}"),
                            "Database name",
                            "letters, digits, _ and -",
                            "",
                            cx,
                        );
                        set_sheet(&e1, Some(Sheet::CreateDatabase { name: n1.clone(), database }), cx);
                    },
                ))
                .child(
                    Button::new(format!("service:{name}:import"), "Import…")
                        .small()
                        .on_click(move |_, _, cx| import(&e2, &n2, cx)),
                ),
        );
    let databases: Option<Vec<DbInfo>> =
        resource.value.as_ref().map(|lists| lists.iter().flat_map(|l| l.databases.clone()).collect());
    let body = match databases {
        Some(dbs) => databases_table(env, &name, &dbs).into_any_element(),
        None => waiting(env, s, resource.loading || resource.error.is_none()),
    };
    let fetch_error = resource.error.map(|e| {
        let env = env.clone();
        ErrorView::new(
            format!("service:{name}:databases:fetch"),
            &e,
            HintAction::parse(e.hint.as_deref().unwrap_or("")),
        )
        .on_action(move |a, _, cx| env.perform(a, cx))
    });
    Some(
        div()
            .flex()
            .flex_col()
            .gap(space::SM)
            .text_color(t.c.text)
            .child(heading)
            .children(fetch_error)
            .child(body)
            .into_any_element(),
    )
}

/// The Connect…, Create database…, Dump… and Import… sheets.
pub fn sheet(env: &AppEnv, sheet: &Sheet, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let close = env.clone();
    let form = match sheet {
        Sheet::Connect { name, info } => connect_form(env, name, info.as_deref(), window, cx),
        Sheet::CreateDatabase { name, database } => {
            let (submit, n, f) = (env.clone(), name.clone(), database.clone());
            Form::new("sheet:create-db", format!("Create a database on {name}"))
                .field(database.clone())
                .error(error(env, &id(name, "create-db"), cx))
                .submit("Create", false, move |_, _, cx| create(&submit, &n, &f, cx))
        }
        Sheet::Dump { name, database, file, compress, extra, starting, .. } => {
            let toggle = env.clone();
            let submit = env.clone();
            Form::new("sheet:dump", format!("Dump {database} from {name}"))
                .field(text("sheet:dump:file", format!("To {}", file.display())))
                .field(Toggle::new("sheet:dump:compress", "Compress", *compress).on_toggle(
                    move |on, _, cx| {
                        update_sheet(&toggle, cx, |s| match s {
                            Sheet::Dump { compress, .. } => {
                                *compress = on;
                                true
                            }
                            _ => false,
                        })
                    },
                ))
                .field(extra.clone())
                .error(error(env, &id(name, "dump"), cx))
                .submit(if *starting { "Starting…" } else { "Dump" }, *starting, move |_, _, cx| {
                    confirm_dump(&submit, cx)
                })
        }
        Sheet::Import { name, engine, file, database, into_existing, keep_owners, extra, starting } => {
            let (t1, t2, submit) = (env.clone(), env.clone(), env.clone());
            let mut form = Form::new("sheet:import", format!("Import into {name}"))
                .field(text("sheet:import:file", format!("From {}", file.display())))
                .field(database.clone())
                .field(Toggle::new("sheet:import:into-existing", "Into existing", *into_existing).on_toggle(
                    move |on, _, cx| {
                        update_sheet(&t1, cx, |s| match s {
                            Sheet::Import { into_existing, .. } => {
                                *into_existing = on;
                                true
                            }
                            _ => false,
                        })
                    },
                ));
            if model::has_owners(engine) {
                form = form.field(
                    Toggle::new("sheet:import:keep-owners", "Keep owners", *keep_owners).on_toggle(
                        move |on, _, cx| {
                            update_sheet(&t2, cx, |s| match s {
                                Sheet::Import { keep_owners, .. } => {
                                    *keep_owners = on;
                                    true
                                }
                                _ => false,
                            })
                        },
                    ),
                );
            }
            form.field(extra.clone()).error(error(env, &id(name, "import"), cx)).submit(
                if *starting { "Starting…" } else { "Import" },
                *starting,
                move |_, _, cx| confirm_import(&submit, cx),
            )
        }
        _ => return None,
    };
    Some(form.on_cancel(move |_, _, cx| set_sheet(&close, None, cx)).into_any_element())
}

/// Connect…'s sheet: the state line, each field with Copy, Copy URL, and Open.
fn connect_form(
    env: &AppEnv,
    name: &str,
    info: Option<&yard_proto::ConnectInfo>,
    window: &mut Window,
    cx: &mut App,
) -> Form {
    let t = Theme::of(window);
    let shown_error = error(env, &id(name, "connect"), cx).or_else(|| error(env, &id(name, "open-db"), cx));
    let form = Form::new("sheet:connect", format!("Connect to {name}")).cancel_label("Done");
    let Some(info) = info else {
        let words = if shown_error.is_some() { "" } else { "Asking yardd…" };
        return form.field(text("sheet:connect:asking", words)).error(shown_error);
    };
    let mut fields = Group::new("sheet:connect:fields", None);
    for f in model::connect_fields(info) {
        let copy = f.copy.clone();
        fields = fields.row(
            f.key,
            f.label,
            crate::widgets::bar()
                .child(
                    div()
                        .flex_1()
                        .min_w(gpui::px(0.))
                        .child(text(format!("sheet:connect:{}", f.key), f.value)),
                )
                .child(
                    Button::new(format!("sheet:connect:{}:copy", f.key), "Copy")
                        .description(f.description)
                        .small()
                        .on_click(move |_, _, cx| Opener::copy(cx, &copy)),
                ),
        );
    }
    let clients = model::connect_clients(&info.engine);
    let (url, open_env, n) = (info.url.clone(), env.clone(), name.to_string());
    form.field(
        div()
            .type_body()
            .text_color(t.c.text_secondary)
            .child(text("sheet:connect:state", format!("{name}  {}", model::connect_state(info)))),
    )
    .field(fields)
    .field(
        div()
            .flex()
            .flex_col()
            .gap(space::XS)
            .type_secondary()
            .text_color(t.c.text_secondary)
            .child(text("sheet:connect:clients:0", clients[0]))
            .child(text("sheet:connect:clients:1", clients[1])),
    )
    .field(
        crate::widgets::bar().child(
            Button::new("sheet:connect:copy-url", "Copy URL")
                .description(format!("Copy {url}"))
                .on_click(move |_, _, cx| Opener::copy(cx, &url)),
        ),
    )
    .error(shown_error)
    .submit("Open", false, move |_, _, cx| open(&open_env, &n, None, cx))
}

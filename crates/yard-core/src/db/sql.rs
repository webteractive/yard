//! The SQL `yard mysql` and `yard postgres` run through an instance's own client (db spec §6.1, §6.2, §6.6).

use crate::services::engines::DbFlavor;
use yard_proto::DbInfo;

/// `name` as an identifier. Names are checked first (`super::valid_name`), so no quote can be in one.
pub fn quote_ident(engine: DbFlavor, name: &str) -> String {
    match engine {
        DbFlavor::Postgres => format!("\"{name}\""),
        _ => format!("`{name}`"),
    }
}

/// One row per database: name, size in bytes, number of tables, tab-separated. PostgreSQL
/// cannot count another database's tables from one connection, so its third column is 0 and
/// yardd asks each database with `table_count`.
pub fn list(engine: DbFlavor) -> &'static str {
    match engine {
        DbFlavor::Postgres => {
            "SELECT datname, pg_database_size(datname), 0 FROM pg_database \
             WHERE NOT datistemplate AND datname NOT IN ('template0', 'template1', 'postgres') ORDER BY datname"
        }
        _ => {
            "SELECT s.schema_name, COALESCE(SUM(t.data_length + t.index_length), 0), COUNT(t.table_name) \
             FROM information_schema.schemata s LEFT JOIN information_schema.tables t ON t.table_schema = s.schema_name \
             WHERE s.schema_name NOT IN ('mysql', 'sys', 'information_schema', 'performance_schema') \
             GROUP BY s.schema_name ORDER BY s.schema_name"
        }
    }
}

/// `CREATE DATABASE` with the instance's character set (db spec §6.2). PostgreSQL has no
/// `IF NOT EXISTS`, so yardd asks `exists` first.
pub fn create(engine: DbFlavor, name: &str) -> String {
    let quoted = quote_ident(engine, name);
    match engine {
        DbFlavor::Postgres => format!("CREATE DATABASE {quoted} ENCODING 'UTF8' TEMPLATE template0"),
        _ => {
            format!("CREATE DATABASE IF NOT EXISTS {quoted} CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci")
        }
    }
}

/// 1 when the database exists, else 0.
pub fn exists(engine: DbFlavor, name: &str) -> String {
    match engine {
        DbFlavor::Postgres => format!("SELECT COUNT(*) FROM pg_database WHERE datname = '{name}'"),
        _ => format!("SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name = '{name}'"),
    }
}

/// The number of tables in `name`. PostgreSQL's runs connected to that database and counts
/// every schema but its own.
pub fn table_count(engine: DbFlavor, name: &str) -> String {
    match engine {
        DbFlavor::Postgres => format!(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_catalog = '{name}' \
             AND table_schema NOT IN ('pg_catalog', 'information_schema')"
        ),
        _ => format!("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = '{name}'"),
    }
}

/// The rows `list` printed (`mysql -N -B`, `psql -At -F '\t'`), sorted by name; a line that is
/// not three tab-separated fields is skipped.
pub fn parse_list(out: &str) -> Vec<DbInfo> {
    let mut rows: Vec<DbInfo> = out
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (name, size, tables) = (fields.next()?, fields.next()?, fields.next()?);
            if fields.next().is_some() || name.is_empty() {
                return None;
            }
            Some(DbInfo {
                name: name.to_string(),
                size_bytes: size.trim().parse().ok()?,
                tables: tables.trim().parse().ok()?,
            })
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use yard_proto::DbInfo;

    #[test]
    fn hyphenated_names_are_quoted() {
        assert_eq!(quote_ident(DbFlavor::Mysql, "my-app"), "`my-app`");
        assert_eq!(quote_ident(DbFlavor::Postgres, "my-app"), "\"my-app\"");
        let create = create(DbFlavor::Mysql, "my-app");
        assert!(create.contains("`my-app`") && create.contains("utf8mb4_unicode_ci"), "{create}");
        let create = create_pg();
        assert!(create.contains("\"my-app\"") && create.contains("ENCODING 'UTF8'"), "{create}");
    }

    fn create_pg() -> String {
        create(DbFlavor::Postgres, "my-app")
    }

    #[test]
    fn list_query_leaves_out_system_schemas() {
        let my = list(DbFlavor::Mysql);
        for name in ["'mysql'", "'sys'", "'information_schema'", "'performance_schema'"] {
            assert!(my.contains(name), "{name}: {my}");
        }
        let pg = list(DbFlavor::Postgres);
        for name in ["'template0'", "'template1'", "'postgres'"] {
            assert!(pg.contains(name), "{name}: {pg}");
        }
    }

    #[test]
    fn exists_and_table_count_name_the_database() {
        assert!(exists(DbFlavor::Mysql, "shop").contains("'shop'"));
        assert!(exists(DbFlavor::Postgres, "shop").contains("'shop'"));
        assert!(table_count(DbFlavor::Mysql, "shop").contains("'shop'"));
        let pg = table_count(DbFlavor::Postgres, "shop");
        assert!(pg.contains("'pg_catalog'") && pg.contains("'information_schema'"), "{pg}");
    }

    #[test]
    fn parse_list_reads_rows() {
        let rows = parse_list("shop\t0\t0\nblog\t1048576\t12\n\n");
        assert_eq!(
            rows,
            vec![
                DbInfo { name: "blog".into(), size_bytes: 1048576, tables: 12 },
                DbInfo { name: "shop".into(), size_bytes: 0, tables: 0 },
            ]
        );
        assert_eq!(parse_list("odd line\n"), vec![]);
    }
}

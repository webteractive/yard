//! Human output is aligned text on stdout; `--json` is the `yard-proto` value unchanged.
//! Errors go to stderr as `error:` and `  hint:` lines, or to stdout as `{"error": …}`.

use serde::Serialize;
use yard_proto::YardError;

/// Columns padded to their widest cell, two spaces apart, no borders.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let mut out = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if i + 1 == cells.len() {
                out.push_str(cell);
            } else {
                out.push_str(&format!("{cell:<width$}  ", width = widths[i]));
            }
        }
        out.trim_end().to_string() + "\n"
    };
    let mut out = line(headers.to_vec());
    for row in rows {
        out.push_str(&line(row.iter().map(String::as_str).collect()));
    }
    out
}

/// The closing line of every human list, pointing scripts at the JSON form.
pub fn for_agents(command: &str) -> String {
    format!("For agents and scripts: {command} --json\n")
}

pub fn print_json<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(e) => eprintln!("error: cannot print JSON: {e}"),
    }
}

/// One compact JSON value per line, for streams such as a job's events (NDJSON).
pub fn print_json_line<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(text) => println!("{text}"),
        Err(e) => eprintln!("error: cannot print JSON: {e}"),
    }
}

pub fn print_error(json: bool, e: &YardError) {
    if json {
        print_json(&serde_json::json!({ "error": e }));
        return;
    }
    eprintln!("error: {}", e.message);
    if let Some(hint) = &e.hint {
        eprintln!("  hint: {hint}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_columns_without_trailing_space() {
        let rows =
            vec![vec!["nginx".to_string(), "8080".into()], vec!["dnsmasq-long".into(), "53535".into()]];
        assert_eq!(
            table(&["UNIT", "PORT"], &rows),
            "UNIT          PORT\nnginx         8080\ndnsmasq-long  53535\n"
        );
    }
}

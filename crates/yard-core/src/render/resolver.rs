//! `/etc/resolver/<tld>`. The marker line is how the helper knows the file is Yard's.

use crate::paths::RESOLVER_MARKER;

pub fn resolver_file(port: u16) -> String {
    format!("{RESOLVER_MARKER}\nnameserver 127.0.0.1\nport {port}\n")
}

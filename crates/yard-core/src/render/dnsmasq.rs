//! dnsmasq answering `*.<tld>` with loopback (spec §5.3).

use super::{RenderCtx, header};

pub fn dnsmasq_conf(ctx: &RenderCtx, port: u16) -> String {
    let tld = ctx.tld;
    format!(
        "{header}listen-address=127.0.0.1
port={port}
bind-interfaces
no-resolv
no-hosts
# yardd supervises dnsmasq in the foreground; no pid file is needed.
pid-file=
address=/{tld}/127.0.0.1
address=/{tld}/::1
",
        header = header(&ctx.paths.yard_toml()),
    )
}

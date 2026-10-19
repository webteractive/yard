mod cli;

use clap::Parser;
use cli::{Cli, Command, LONG_VERSION};

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => version(cli.json),
    }
}

fn version(json: bool) {
    if json {
        let info = serde_json::json!({
            "cli": env!("CARGO_PKG_VERSION"),
            "daemon": null,
            "helper": null,
            "protocol": yard_proto::PROTOCOL_VERSION,
            "commit": env!("YARD_BUILD_COMMIT"),
            "build_date": env!("YARD_BUILD_DATE"),
        });
        println!("{info}");
    } else {
        println!("yard {LONG_VERSION}");
    }
}

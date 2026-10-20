//! Command-line surface of `yard`. Parsing only; commands live in `commands/`.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// `0.1.0 (abc1234, 2026-10-19)` — the first line of every version report.
pub const LONG_VERSION: &str =
    concat!(env!("CARGO_PKG_VERSION"), " (", env!("YARD_BUILD_COMMIT"), ", ", env!("YARD_BUILD_DATE"), ")");

#[derive(Debug, Parser)]
#[command(
    name = "yard",
    about = "Local PHP development environment for macOS",
    after_help = "For agents and scripts: every command accepts --json.",
    // `--version` also asks yardd and the helper, so it is ours rather than clap's.
    disable_version_flag = true
)]
pub struct Cli {
    /// Print machine-readable JSON instead of human output.
    #[arg(long, global = true)]
    pub json: bool,

    /// Use this data root instead of YARD_ROOT or ~/Library/Application Support/Yard.
    #[arg(long, global = true, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Print the CLI, daemon, helper and protocol versions.
    #[arg(short = 'V', long)]
    pub version: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install yardd's LaunchAgent and start it.
    Start,
    /// Stop yardd and unload its LaunchAgent; sites stop being served.
    Stop,
    /// Show the profile, TLD, units and site count.
    Status,
    /// Show the CLI, daemon, helper and protocol versions.
    Version,
    /// Check the installation and print the command that fixes each problem.
    Doctor,
    /// List the sites Yard serves.
    Sites,
    /// Serve the current directory as NAME.<tld>.
    Link {
        /// The site name; defaults to the current directory's name.
        name: Option<String>,
    },
    /// Stop serving a linked site.
    Unlink {
        /// The site name; defaults to the current directory's name.
        name: Option<String>,
    },
    /// Serve NAME.<tld> by proxying to URL, e.g. http://127.0.0.1:3000.
    Proxy { name: String, url: String },
    /// Stop serving a proxied site.
    Unproxy { name: String },
    /// Set the directory a site serves, relative to the site's path.
    Docroot {
        /// The docroot, e.g. public or web.
        path: String,
        /// The site; defaults to the site that contains the current directory.
        #[arg(long)]
        site: Option<String>,
    },
    /// List the registered ports, or change one.
    Ports {
        #[command(subcommand)]
        action: Option<PortsAction>,
    },
    /// List the PHP versions brew has installed, or install, remove or choose one.
    Php {
        #[command(subcommand)]
        action: Option<PhpAction>,
    },
    /// Restart a unit: nginx, dnsmasq or php-fpm-<version>.
    Restart { unit: String },
    /// Install or remove the root helper; run with sudo.
    Helper {
        #[command(subcommand)]
        action: HelperAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum PortsAction {
    /// Probe every registered port now; works without yardd.
    Check,
    /// Give a unit a new port.
    Set { unit: String, port: u16 },
    /// Keep a unit on its port: Yard fails rather than move it.
    Pin { unit: String },
    /// Let Yard move a unit to a free port when its port is taken.
    Unpin { unit: String },
    /// Set the range automatic ports come from, e.g. 33000-33999.
    Range { range: String },
    /// Switch the port profile: standard (nginx on 8080/8443) or privileged (80/443).
    Profile { profile: ProfileArg },
}

#[derive(Debug, Subcommand)]
pub enum PhpAction {
    /// The installed versions, the default, the sites on each and its pool.
    List,
    /// Install a PHP version (7.0–8.5) with brew, showing brew's output.
    Install { version: String },
    /// Remove a PHP version with brew; refused while it is the default.
    Uninstall { version: String },
    /// Make an installed version the default for sites that are not isolated.
    Default { version: String },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ProfileArg {
    /// nginx on 8080/8443; no root for nginx.
    #[value(alias = "trial")]
    Standard,
    /// nginx on 80/443, through sockets the root helper opens.
    #[value(alias = "default")]
    Privileged,
}

#[derive(Debug, Subcommand)]
pub enum HelperAction {
    /// Copy yard-helper to /Library/PrivilegedHelperTools and load its LaunchDaemon.
    Install,
    /// Unload the helper and remove it and Yard's /etc/resolver files.
    Uninstall,
}

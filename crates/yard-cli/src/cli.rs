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
        /// Install what the project's .yard needs without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Write the current project's .yard (kept in Yard's data root unless --local), link
    /// it, and install what it needs after asking.
    Init {
        /// The site name when the project is not linked yet; defaults to the directory's name.
        name: Option<String>,
        /// Rewrite the settings Yard detects in an existing .yard, keeping the rest.
        #[arg(long)]
        force: bool,
        /// Move a site with this name here even though this directory has its own .yard
        /// (or the name is a proxy). Without a .yard here, init moves it without asking.
        #[arg(long)]
        replace: bool,
        /// Install what the .yard needs without asking.
        #[arg(long)]
        yes: bool,
        /// Write .yard into the project instead of Yard's data root (sites/<name>/.yard).
        #[arg(long)]
        local: bool,
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
    /// Run a site on its own PHP version instead of the default.
    Isolate {
        /// The version, e.g. 7.1; it must be installed (yard php install).
        version: String,
        /// The site; defaults to the site that contains the current directory.
        #[arg(long)]
        site: Option<String>,
    },
    /// Run a site on the default PHP version again.
    Unisolate {
        /// The site; defaults to the site that contains the current directory.
        #[arg(long)]
        site: Option<String>,
    },
    /// Serve a site over HTTPS, with a certificate from Yard's local CA.
    Secure {
        /// The site; defaults to the site that contains the current directory.
        site: Option<String>,
    },
    /// Serve a site over plain HTTP again.
    Unsecure {
        /// The site; defaults to the site that contains the current directory.
        site: Option<String>,
    },
    /// Trust Yard's CA in your login keychain, so browsers accept its certificates.
    ///
    /// Runs macOS's security add-trusted-cert as you; macOS asks for your password.
    Trust,
    /// Remove Yard's CA from your login keychain.
    Untrust,
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
    /// PHP extensions: list, add, enable, disable and remove them.
    Ext {
        #[command(subcommand)]
        action: ExtAction,
    },
    /// Turn Xdebug on or off for a PHP version (add it first with yard ext add xdebug).
    Xdebug {
        state: OnOff,
        /// The PHP version; defaults to the default version.
        #[arg(long)]
        php: Option<String>,
    },
    /// Print the PATH line that puts Yard's php, composer and pecl shims first, for your
    /// shell profile; warns when another php (Herd's) would still win. Edits nothing.
    InstallShims,
    /// List the service instances (MySQL, PostgreSQL, Redis, MinIO), their ports and logins.
    Services,
    /// Add, remove, start, stop or inspect one service instance.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Print the .env lines for a project's services (or one instance). Writes no file.
    ///
    /// In a linked project, or with --site, it prints the services the project's .yard
    /// lists. Outside a project, with no flag, it prints every instance.
    Env {
        /// The site; defaults to the linked project that contains the current directory.
        #[arg(long)]
        site: Option<String>,
        /// Print only this instance, e.g. mysql-8.4 (see yard services).
        #[arg(long)]
        service: Option<String>,
    },
    /// Show what a site runs on: PHP, docroot, HTTPS, extensions, services and unmet needs.
    Info {
        /// The site; defaults to the linked project that contains the current directory.
        site: Option<String>,
    },
    /// List the brew taps Yard installs from, and what brew trusts in them.
    Sources,
    /// Change what brew trusts from Yard's taps.
    Source {
        #[command(subcommand)]
        action: SourceAction,
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
pub enum ServiceAction {
    /// Add a service: install its formula with brew if needed, give it a port, initialize
    /// its data, start it and print its .env lines. Shows brew's output; Ctrl-C cancels
    /// and undoes what the add created.
    Add {
        /// mysql, postgres, redis or minio.
        engine: String,
        /// The release line, e.g. 8.4 for MySQL or 17 for PostgreSQL; the engine's default
        /// when omitted. Redis and MinIO have none.
        #[arg(long)]
        version: Option<String>,
        /// The port to pin; defaults to the engine's usual port, else the next free one.
        #[arg(long)]
        port: Option<u16>,
        /// Accepted for scripts. Adding asks nothing but the trust question, which --yes
        /// never answers: trust is granted only by yard source trust or by answering it.
        #[arg(long)]
        yes: bool,
    },
    /// Remove an instance and its data (or keep the data), after asking.
    ///
    /// Refused while a linked site's .yard lists the instance. Never runs brew uninstall.
    Remove {
        instance: String,
        /// Keep services/<instance>/data in place; everything else in the directory goes.
        #[arg(long)]
        keep_data: bool,
        /// Skip the question; required when there is no terminal.
        #[arg(long)]
        yes: bool,
    },
    /// Start an instance (and keep it started across yardd restarts), then wait until it is up.
    Start { instance: String },
    /// Stop an instance; yardd leaves it stopped until yard service start.
    Stop { instance: String },
    /// Restart a running instance, then wait until it is up.
    Restart { instance: String },
    /// Print the end of an instance's log (100 lines), and with -f keep following it.
    Logs {
        instance: String,
        /// Keep printing new lines until Ctrl-C.
        #[arg(short, long)]
        follow: bool,
        /// How many lines of the tail to print (at most 10000).
        #[arg(short = 'n', long, value_name = "N")]
        lines: Option<usize>,
    },
    /// Move an instance to another port (a running instance restarts on it).
    Port { instance: String, port: u16 },
}

#[derive(Debug, Subcommand)]
pub enum SourceAction {
    /// Have brew trust a formula (tap/repo/name), or with --tap a whole tap.
    Trust {
        /// A formula such as minio/stable/minio, or a tap such as minio/stable with --tap.
        target: String,
        /// Trust every formula in the tap.
        #[arg(long)]
        tap: bool,
    },
    /// Have brew stop trusting a formula, or with --tap a whole tap.
    ///
    /// What is installed keeps serving, since Yard runs kegs by path, but brew will no
    /// longer load the formula to install, upgrade or remove it.
    Untrust {
        target: String,
        /// Untrust the whole tap.
        #[arg(long)]
        tap: bool,
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
    /// Install a PHP version (7.0–8.5) with brew, showing brew's output; Ctrl-C cancels.
    Install { version: String },
    /// Remove a PHP version with brew; refused while it is the default, a site is
    /// isolated to it, or it has extensions you added.
    Uninstall { version: String },
    /// Make an installed version the default for sites that are not isolated.
    Default { version: String },
    /// Read or change your php.ini overrides (php/<ver>/conf.d/99-overrides.ini).
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

#[derive(Debug, Subcommand)]
pub enum ExtAction {
    /// The extensions Yard manages for a version, and the ones brew's PHP loads itself.
    List {
        /// The PHP version; defaults to the default version.
        #[arg(long)]
        php: Option<String>,
    },
    /// Install an extension and load it: brew's shivammathur/extensions formula if there
    /// is one, else pecl; or a path to a built .so, which Yard copies. Checked with
    /// php -m, and rolled back if it does not load or you press Ctrl-C.
    Add {
        /// An extension name (redis, xdebug), a pinned pecl release (xdebug-2.9.8, for
        /// legacy PHP), or the path of a .so.
        name: String,
        /// The PHP version; defaults to the default version.
        #[arg(long)]
        php: Option<String>,
    },
    /// Load an extension again, without reinstalling it.
    Enable {
        name: String,
        /// The PHP version; defaults to the default version.
        #[arg(long)]
        php: Option<String>,
    },
    /// Stop loading an extension, without uninstalling it; works for ones PHP bundles.
    Disable {
        name: String,
        /// The PHP version; defaults to the default version.
        #[arg(long)]
        php: Option<String>,
    },
    /// Uninstall an extension (brew or pecl) or delete its copied .so, and forget it.
    Remove {
        name: String,
        /// The PHP version; defaults to the default version.
        #[arg(long)]
        php: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print an override's value.
    Get {
        key: String,
        /// The PHP version; defaults to the default version.
        #[arg(long)]
        php: Option<String>,
    },
    /// Set an override and reload that version's pool, e.g. memory_limit 1G.
    Set {
        key: String,
        value: String,
        /// The PHP version; defaults to the default version.
        #[arg(long)]
        php: Option<String>,
    },
    /// Remove an override and reload that version's pool.
    Unset {
        key: String,
        /// The PHP version; defaults to the default version.
        #[arg(long)]
        php: Option<String>,
    },
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

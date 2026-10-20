//! The port registry (spec §7): every port Yard binds or routes to, in `ports.toml`.
//! Pure: callers pass `is_free` so probing stays in the daemon.

pub mod holder;

use crate::Paths;
use crate::config::{TomlFile, config_error, read_optional};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;
use toml_edit::{InlineTable, Item, Value, value};
use yard_proto::{ErrorCode, PortMode, Profile, YardError};

pub const NGINX_HTTP: &str = "nginx-http";
pub const NGINX_HTTPS: &str = "nginx-https";
pub const DNSMASQ: &str = "dnsmasq";
pub const DNSMASQ_PORT: u16 = 53535;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

impl FromStr for PortRange {
    type Err = YardError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || {
            YardError::new(ErrorCode::InvalidArgument, format!("invalid port range {s:?}"))
                .with_hint("use <start>-<end> with 1024 <= start <= end, e.g. 33000-33999")
        };
        let (a, b) = s.split_once('-').ok_or_else(bad)?;
        let range = PortRange {
            start: a.trim().parse().map_err(|_| bad())?,
            end: b.trim().parse().map_err(|_| bad())?,
        };
        if range.start < 1024 || range.start > range.end {
            return Err(bad());
        }
        Ok(range)
    }
}

impl fmt::Display for PortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.start, self.end)
    }
}

impl PortRange {
    pub fn contains(&self, port: u16) -> bool {
        (self.start..=self.end).contains(&port)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortEntry {
    pub port: u16,
    pub mode: PortMode,
    #[serde(default)]
    pub preferred: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub range: PortRange,
    pub auto_fallback: bool,
    pub profile: Profile,
    pub entries: BTreeMap<String, PortEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    range: Option<String>,
    auto_fallback: Option<bool>,
    profile: Option<Profile>,
    ports: Option<BTreeMap<String, PortEntry>>,
}

const DEFAULT_RANGE: PortRange = PortRange { start: 33000, end: 33999 };

fn pinned(port: u16) -> PortEntry {
    PortEntry { port, mode: PortMode::Pinned, preferred: None }
}

pub fn mode_str(mode: PortMode) -> &'static str {
    match mode {
        PortMode::Pinned => "pinned",
        PortMode::Preferred => "preferred",
        PortMode::Auto => "auto",
        PortMode::External => "external",
    }
}

impl Registry {
    pub fn load(paths: &Paths) -> Result<Registry, YardError> {
        let file = paths.ports_toml();
        Self::parse(read_optional(&file)?.as_deref(), &file)
    }

    /// A missing file seeds the standard profile's nginx ports and dnsmasq.
    pub fn parse(text: Option<&str>, file: &Path) -> Result<Registry, YardError> {
        let raw: Raw = match text {
            Some(text) => {
                toml::from_str(text).map_err(|e| config_error(file, text, e.span(), e.message()))?
            }
            None => Raw { range: None, auto_fallback: None, profile: None, ports: None },
        };
        let range = match raw.range {
            Some(r) => r.parse().map_err(|e: YardError| {
                config_error(file, text.unwrap_or_default(), None, &format!("range: {}", e.message))
            })?,
            None => DEFAULT_RANGE,
        };
        let profile = raw.profile.unwrap_or_default();
        let entries = raw.ports.unwrap_or_else(|| {
            BTreeMap::from([
                (NGINX_HTTP.to_string(), pinned(profile.http_port())),
                (NGINX_HTTPS.to_string(), pinned(profile.https_port())),
                (DNSMASQ.to_string(), pinned(DNSMASQ_PORT)),
            ])
        });
        // Two units binding one port can only come from a hand edit; Yard never allocates
        // that. External entries may share a port: two proxies can target one app.
        let mut seen: BTreeMap<u16, &str> = BTreeMap::new();
        for (unit, e) in entries.iter().filter(|(_, e)| e.mode != PortMode::External) {
            if let Some(first) = seen.insert(e.port, unit) {
                let text = text.unwrap_or_default();
                let at = [first, unit.as_str()]
                    .iter()
                    .filter_map(|u| text.find(&format!("{u} ")).map(|i| i..i + u.len()))
                    .max_by_key(|r| r.start);
                let message = format!("port {} is registered to both {first} and {unit}", e.port);
                return Err(config_error(file, text, at, &message));
            }
        }
        Ok(Registry { range, auto_fallback: raw.auto_fallback.unwrap_or(false), profile, entries })
    }

    /// Human-readable problems: duplicate ports, missing `preferred`, `auto` outside the range.
    pub fn validate(&self) -> Vec<String> {
        let mut by_port: BTreeMap<u16, Vec<&str>> = BTreeMap::new();
        for (unit, e) in &self.entries {
            by_port.entry(e.port).or_default().push(unit);
        }
        let mut problems: Vec<String> = by_port
            .iter()
            .filter(|(_, units)| units.len() > 1)
            .map(|(port, units)| format!("port {port} is registered to {}", units.join(", ")))
            .collect();
        for (unit, e) in &self.entries {
            if e.mode == PortMode::Preferred && e.preferred.is_none() {
                problems.push(format!("{unit} is preferred but has no preferred port"));
            }
            if e.mode == PortMode::Auto && !self.range.contains(e.port) {
                problems.push(format!("{unit} is auto but port {} is outside {}", e.port, self.range));
            }
        }
        problems
    }

    /// The unit other than `except` that holds `port`, if any.
    pub fn holder_of(&self, port: u16, except: &str) -> Option<&str> {
        self.entries.iter().find(|(u, e)| u.as_str() != except && e.port == port).map(|(u, _)| u.as_str())
    }

    pub fn allocate(
        &mut self,
        unit: &str,
        mode: PortMode,
        preferred: Option<u16>,
        is_free: &dyn Fn(u16) -> bool,
    ) -> Result<u16, YardError> {
        let need = || {
            preferred.ok_or_else(|| {
                YardError::new(
                    ErrorCode::InvalidArgument,
                    format!("{unit}: a {} port needs a number", mode_str(mode)),
                )
            })
        };
        let port = match mode {
            PortMode::Pinned => {
                let p = need()?;
                self.check_unregistered(unit, p)?;
                if !is_free(p) {
                    return Err(in_use(unit, p));
                }
                p
            }
            PortMode::Preferred => {
                let p = need()?;
                if self.holder_of(p, unit).is_none() && is_free(p) {
                    p
                } else if self.auto_fallback {
                    self.lowest_free(unit, is_free)?
                } else {
                    return Err(in_use(unit, p));
                }
            }
            PortMode::Auto => self.lowest_free(unit, is_free)?,
            PortMode::External => {
                let p = need()?;
                self.check_unregistered(unit, p)?;
                p
            }
        };
        let preferred = if mode == PortMode::Preferred { preferred } else { None };
        self.entries.insert(unit.to_string(), PortEntry { port, mode, preferred });
        Ok(port)
    }

    /// The unit's port, allocating on first use. An existing entry keeps its mode, so
    /// a user's edit to `ports.toml` wins; `preferred` retries its preferred port, and
    /// an `external` entry follows a changed proxy target.
    pub fn ensure(
        &mut self,
        unit: &str,
        mode: PortMode,
        preferred: Option<u16>,
        is_free: &dyn Fn(u16) -> bool,
    ) -> Result<u16, YardError> {
        let Some(e) = self.entries.get(unit).copied() else {
            return self.allocate(unit, mode, preferred, is_free);
        };
        match e.mode {
            PortMode::Pinned => Ok(e.port),
            PortMode::External => match preferred {
                Some(p) if mode == PortMode::External && p != e.port => {
                    self.allocate(unit, PortMode::External, Some(p), is_free)
                }
                _ => Ok(e.port),
            },
            PortMode::Preferred => {
                let want = e.preferred.unwrap_or(e.port);
                let usable = |p: u16| self.holder_of(p, unit).is_none() && is_free(p);
                if usable(want) {
                    self.entries.insert(unit.to_string(), PortEntry { port: want, ..e });
                    Ok(want)
                } else if e.port != want && usable(e.port) {
                    Ok(e.port)
                } else {
                    self.allocate(unit, PortMode::Preferred, Some(want), is_free)
                }
            }
            PortMode::Auto => {
                if self.range.contains(e.port) && self.holder_of(e.port, unit).is_none() && is_free(e.port) {
                    Ok(e.port)
                } else {
                    self.allocate(unit, PortMode::Auto, None, is_free)
                }
            }
        }
    }

    pub fn set(&mut self, unit: &str, port: u16) -> Result<(), YardError> {
        self.entry(unit)?;
        self.check_unregistered(unit, port)?;
        if let Some(e) = self.entries.get_mut(unit) {
            e.port = port;
            if e.mode == PortMode::Preferred {
                e.preferred = Some(port);
            }
        }
        Ok(())
    }

    pub fn pin(&mut self, unit: &str) -> Result<(), YardError> {
        let e = self.entry(unit)?;
        self.entries.insert(unit.to_string(), pinned(e.port));
        Ok(())
    }

    pub fn unpin(&mut self, unit: &str) -> Result<(), YardError> {
        let e = self.entry(unit)?;
        if e.mode == PortMode::Pinned {
            self.entries.insert(
                unit.to_string(),
                PortEntry { port: e.port, mode: PortMode::Preferred, preferred: Some(e.port) },
            );
        }
        Ok(())
    }

    pub fn set_range(&mut self, range: PortRange) {
        self.range = range;
    }

    /// Checks both of the new profile's ports first and changes nothing unless all are usable.
    pub fn set_profile(&mut self, profile: Profile, is_free: &dyn Fn(u16) -> bool) -> Result<(), YardError> {
        let targets = [(NGINX_HTTP, profile.http_port()), (NGINX_HTTPS, profile.https_port())];
        let blocked: Vec<u16> = targets
            .iter()
            .filter(|(unit, port)| {
                let other = self.holder_of(*port, unit).filter(|o| !targets.iter().any(|(t, _)| t == o));
                other.is_some() || !is_free(*port)
            })
            .map(|(_, port)| *port)
            .collect();
        if !blocked.is_empty() {
            let list = blocked.iter().map(u16::to_string).collect::<Vec<_>>().join(", ");
            return Err(YardError::new(
                ErrorCode::PortInUse,
                format!("cannot switch to the {} profile: port {list} in use", profile.name()),
            )
            .with_hint("free those ports first, or stay on the standard profile")
            .with_data(serde_json::json!({ "ports": blocked })));
        }
        for (unit, port) in targets {
            self.entries.insert(unit.to_string(), pinned(port));
        }
        self.profile = profile;
        Ok(())
    }

    /// Writes the registry into `file`, touching only values that changed so comments
    /// and alignment on untouched lines survive.
    pub fn save(&self, file: &mut TomlFile) {
        let current: Raw = toml::from_str(&file.doc().to_string()).unwrap_or(Raw {
            range: None,
            auto_fallback: None,
            profile: None,
            ports: None,
        });
        if current.range.as_deref().and_then(|r| r.parse::<PortRange>().ok()) != Some(self.range) {
            file.set(&["range"], value(self.range.to_string()));
        }
        if current.auto_fallback != Some(self.auto_fallback) {
            file.set(&["auto_fallback"], value(self.auto_fallback));
        }
        if current.profile != Some(self.profile) {
            file.set(&["profile"], value(self.profile.name()));
        }
        let old = current.ports.unwrap_or_default();
        for unit in old.keys().filter(|u| !self.entries.contains_key(*u)) {
            file.remove(&["ports", unit]);
        }
        for (unit, e) in &self.entries {
            if old.get(unit) != Some(e) {
                let mut t = InlineTable::new();
                t.insert("port", i64::from(e.port).into());
                t.insert("mode", mode_str(e.mode).into());
                if let Some(p) = e.preferred {
                    t.insert("preferred", i64::from(p).into());
                }
                file.set(&["ports", unit], Item::Value(Value::InlineTable(t)));
            }
        }
    }

    fn entry(&self, unit: &str) -> Result<PortEntry, YardError> {
        self.entries.get(unit).copied().ok_or_else(|| {
            YardError::new(ErrorCode::InvalidArgument, format!("no port registered for {unit:?}"))
                .with_hint("yard ports lists the registered units")
        })
    }

    fn check_unregistered(&self, unit: &str, port: u16) -> Result<(), YardError> {
        match self.holder_of(port, unit) {
            Some(other) => Err(YardError::new(
                ErrorCode::PortConflict,
                format!("port {port} is already registered to {other}"),
            )
            .with_hint(format!("pick another port, or move {other} with yard ports set {other} <port>"))),
            None => Ok(()),
        }
    }

    fn lowest_free(&self, unit: &str, is_free: &dyn Fn(u16) -> bool) -> Result<u16, YardError> {
        (self.range.start..=self.range.end)
            .find(|p| self.holder_of(*p, unit).is_none() && is_free(*p))
            .ok_or_else(|| {
                YardError::new(ErrorCode::PortInUse, format!("no free port left in {}", self.range))
                    .with_hint("widen it with yard ports range <start>-<end>")
            })
    }
}

fn in_use(unit: &str, port: u16) -> YardError {
    YardError::new(ErrorCode::PortInUse, format!("port {port} for {unit} is in use"))
        .with_hint(format!(
            "yard ports set {unit} <port>, or set auto_fallback = true in ports.toml for preferred ports"
        ))
        .with_data(serde_json::json!({ "unit": unit, "port": port }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TomlFile;
    use std::path::Path;
    use yard_proto::ErrorCode;

    const SPEC_EXAMPLE: &str = r#"range = "33000-33999"
auto_fallback = false
profile = "standard"   # or "privileged"

[ports]
nginx-http   = { port = 8080,  mode = "pinned" }
nginx-https  = { port = 8443,  mode = "pinned" }
dnsmasq      = { port = 53535, mode = "pinned" }
mysql-8-0    = { port = 3306,  mode = "preferred", preferred = 3306 }
redis        = { port = 6379,  mode = "preferred", preferred = 6379 }
mailpit-smtp = { port = 1025,  mode = "preferred", preferred = 1025 }
mailpit-http = { port = 33001, mode = "auto" }
"proxy:api"  = { port = 3000,  mode = "external" }
"#;

    fn file() -> &'static Path {
        Path::new("/r/ports.toml")
    }

    fn all_free(_: u16) -> bool {
        true
    }

    fn small(range: &str) -> Registry {
        let mut r = Registry::parse(Some(&format!("range = \"{range}\"\n")), file()).unwrap();
        r.auto_fallback = false;
        r
    }

    #[test]
    fn parses_spec_example() {
        let r = Registry::parse(Some(SPEC_EXAMPLE), file()).unwrap();
        assert_eq!(r.entries.len(), 8);
        assert_eq!(
            r.entries["proxy:api"],
            PortEntry { port: 3000, mode: PortMode::External, preferred: None }
        );
        assert_eq!(r.entries["mailpit-http"].mode, PortMode::Auto);
        assert_eq!(r.entries["mysql-8-0"].preferred, Some(3306));
        assert_eq!(r.profile, Profile::Standard);
        assert!(r.validate().is_empty(), "{:?}", r.validate());
    }

    #[test]
    fn seeds_standard_profile() {
        let r = Registry::parse(None, file()).unwrap();
        assert_eq!(r.profile, Profile::Standard);
        assert_eq!(r.range, "33000-33999".parse().unwrap());
        assert!(!r.auto_fallback);
        assert_eq!(
            r.entries["nginx-http"],
            PortEntry { port: 8080, mode: PortMode::Pinned, preferred: None }
        );
        assert_eq!(r.entries["nginx-https"].port, 8443);
        assert_eq!(r.entries["dnsmasq"], PortEntry { port: 53535, mode: PortMode::Pinned, preferred: None });
    }

    #[test]
    fn range_parsing() {
        assert_eq!("33000-33002".parse::<PortRange>().unwrap(), PortRange { start: 33000, end: 33002 });
        for bad in ["5-4", "80-90", "x", "33000"] {
            assert!(bad.parse::<PortRange>().is_err(), "{bad}");
        }
    }

    #[test]
    fn rejects_duplicate_ports() {
        let text =
            "[ports]\na = { port = 3306, mode = \"pinned\" }\nb = { port = 3306, mode = \"external\" }\n";
        let problems = Registry::parse(Some(text), file()).unwrap().validate();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("a") && problems[0].contains("b") && problems[0].contains("3306"));
    }

    #[test]
    fn validate_flags_preferred_without_value_and_auto_outside_range() {
        let text = "range = \"33000-33010\"\n[ports]\na = { port = 3306, mode = \"preferred\" }\nb = { port = 4000, mode = \"auto\" }\n";
        assert_eq!(Registry::parse(Some(text), file()).unwrap().validate().len(), 2);
    }

    #[test]
    fn auto_takes_lowest_free_in_range() {
        let mut r = small("33000-33002");
        r.entries.insert("x".into(), PortEntry { port: 33000, mode: PortMode::Auto, preferred: None });
        let p = r.allocate("y", PortMode::Auto, None, &|p| p != 33001).unwrap();
        assert_eq!(p, 33002);
        assert_eq!(r.entries["y"].port, 33002);
    }

    #[test]
    fn auto_with_full_range_is_port_in_use() {
        let mut r = small("33000-33000");
        assert_eq!(r.allocate("y", PortMode::Auto, None, &|_| false).unwrap_err().code, ErrorCode::PortInUse);
    }

    #[test]
    fn preferred_without_fallback_reports_port_in_use() {
        let mut r = small("33000-33009");
        let e = r.allocate("redis", PortMode::Preferred, Some(6379), &|p| p != 6379).unwrap_err();
        assert_eq!(e.code, ErrorCode::PortInUse);
        assert!(e.hint.unwrap().contains("yard ports set"));
        assert!(!r.entries.contains_key("redis"));
    }

    #[test]
    fn preferred_with_fallback_records_new_port() {
        let mut r = small("33000-33009");
        r.auto_fallback = true;
        let p = r.allocate("redis", PortMode::Preferred, Some(6379), &|p| p != 6379).unwrap();
        assert_eq!(p, 33000);
        assert_eq!(
            r.entries["redis"],
            PortEntry { port: 33000, mode: PortMode::Preferred, preferred: Some(6379) }
        );
    }

    #[test]
    fn pinned_unavailable_does_not_move() {
        let mut r = Registry::parse(None, file()).unwrap();
        let e = r.ensure("nginx-http", PortMode::Pinned, Some(8080), &|p| p != 8080);
        assert_eq!(e.unwrap(), 8080, "ensure keeps an existing pinned entry; pre-start reports the conflict");
        let mut fresh = small("33000-33009");
        let e = fresh.allocate("web", PortMode::Pinned, Some(9000), &|p| p != 9000).unwrap_err();
        assert_eq!(e.code, ErrorCode::PortInUse);
        assert!(!fresh.entries.contains_key("web"));
    }

    #[test]
    fn pinned_on_a_registered_port_is_conflict() {
        let mut r = Registry::parse(None, file()).unwrap();
        let e = r.allocate("other", PortMode::Pinned, Some(53535), &all_free).unwrap_err();
        assert_eq!(e.code, ErrorCode::PortConflict);
    }

    #[test]
    fn external_is_never_allocated() {
        let mut r = small("2999-3001");
        r.allocate("proxy:api", PortMode::External, Some(3000), &all_free).unwrap();
        assert_eq!(r.allocate("a", PortMode::Auto, None, &all_free).unwrap(), 2999);
        assert_eq!(r.allocate("b", PortMode::Auto, None, &all_free).unwrap(), 3001);
    }

    #[test]
    fn ensure_moves_external_when_target_changes() {
        let mut r = small("33000-33009");
        r.ensure("proxy:api", PortMode::External, Some(3000), &all_free).unwrap();
        r.ensure("proxy:api", PortMode::External, Some(5173), &all_free).unwrap();
        assert_eq!(r.entries["proxy:api"].port, 5173);
    }

    #[test]
    fn ensure_retries_preferred_first() {
        let mut r = small("33000-33009");
        r.auto_fallback = true;
        r.allocate("redis", PortMode::Preferred, Some(6379), &|p| p != 6379).unwrap();
        assert_eq!(r.ensure("redis", PortMode::Preferred, Some(6379), &all_free).unwrap(), 6379);
        assert_eq!(r.entries["redis"].port, 6379);
    }

    #[test]
    fn set_pin_unpin() {
        let mut r = Registry::parse(None, file()).unwrap();
        r.set("nginx-http", 8081).unwrap();
        assert_eq!(r.entries["nginx-http"].port, 8081);
        assert_eq!(r.set("nginx-http", 53535).unwrap_err().code, ErrorCode::PortConflict);
        assert_eq!(r.set("nope", 9000).unwrap_err().code, ErrorCode::InvalidArgument);
        r.unpin("dnsmasq").unwrap();
        assert_eq!(
            r.entries["dnsmasq"],
            PortEntry { port: 53535, mode: PortMode::Preferred, preferred: Some(53535) }
        );
        r.pin("dnsmasq").unwrap();
        assert_eq!(r.entries["dnsmasq"], PortEntry { port: 53535, mode: PortMode::Pinned, preferred: None });
    }

    #[test]
    fn duplicate_port_is_a_config_error() {
        let text = "[ports]\nnginx-http = { port = 8080, mode = \"pinned\" }\ndnsmasq = { port = 8080, mode = \"pinned\" }\n";
        let e = Registry::parse(Some(text), file()).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert!(
            e.message.contains("8080") && e.message.contains("both dnsmasq and nginx-http"),
            "{}",
            e.message
        );
        assert_eq!(e.data.unwrap()["line"], 3);
    }

    #[test]
    fn profile_switch_reports_holders() {
        let mut r = Registry::parse(None, file()).unwrap();
        let e = r.set_profile(Profile::Privileged, &|p| p != 80 && p != 443).unwrap_err();
        assert_eq!(e.code, ErrorCode::PortInUse);
        assert_eq!(e.data.unwrap()["ports"], serde_json::json!([80, 443]));
        assert_eq!(r.profile, Profile::Standard);
        assert_eq!(r.entries["nginx-http"].port, 8080);
        assert_eq!(r.entries["nginx-https"].port, 8443);
    }

    #[test]
    fn profile_switch_moves_pinned_ports() {
        let mut r = Registry::parse(None, file()).unwrap();
        r.set_profile(Profile::Privileged, &all_free).unwrap();
        assert_eq!(
            (r.profile, r.entries["nginx-http"].port, r.entries["nginx-https"].port),
            (Profile::Privileged, 80, 443)
        );
    }

    #[test]
    fn old_profile_names_still_parse() {
        let standard = Registry::parse(Some("profile = \"trial\"\n"), file()).unwrap();
        assert_eq!(standard.profile, Profile::Standard);
        let privileged = Registry::parse(Some("profile = \"default\"\n"), file()).unwrap();
        assert_eq!(privileged.profile, Profile::Privileged);
    }

    #[test]
    fn saving_writes_new_profile_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ports.toml");
        std::fs::write(&path, "profile = \"trial\"\n").unwrap();
        let mut r = Registry::parse(Some("profile = \"trial\"\n"), &path).unwrap();
        r.set_profile(Profile::Privileged, &|_| true).unwrap();
        let mut f = TomlFile::load(&path).unwrap();
        r.save(&mut f);
        f.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("profile = \"privileged\""), "{text}");
    }

    #[test]
    fn save_round_trips_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ports.toml");
        std::fs::write(&path, SPEC_EXAMPLE).unwrap();
        let mut r = Registry::parse(Some(SPEC_EXAMPLE), &path).unwrap();
        r.set("redis", 6380).unwrap();
        r.entries.remove("mailpit-http");
        let mut f = TomlFile::load(&path).unwrap();
        r.save(&mut f);
        f.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# or \"privileged\""), "{text}");
        let back = Registry::parse(Some(&text), &path).unwrap();
        assert_eq!(back, r);
    }
}

//! Node versions and the specs that pin them (M4c spec §4): parsing, LTS names, and
//! matching a spec against installed versions or nodejs.org's release list.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;
use yard_proto::{ErrorCode, YardError};

/// A released Node version, `22.17.1`. fnm names its directories `v22.17.1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl fmt::Display for NodeVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for NodeVersion {
    type Err = YardError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || {
            YardError::new(ErrorCode::InvalidArgument, format!("{s:?} is not a Node version such as 22.17.1"))
        };
        let parts = numbers(s.strip_prefix('v').unwrap_or(s)).ok_or_else(bad)?;
        match parts[..] {
            [major, minor, patch] => Ok(NodeVersion { major, minor, patch }),
            _ => Err(bad()),
        }
    }
}

/// `22` → `[22]`; `None` unless every dot-separated part is a plain number.
fn numbers(s: &str) -> Option<Vec<u32>> {
    s.split('.')
        .map(|part| {
            if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) {
                part.parse().ok()
            } else {
                None
            }
        })
        .collect()
}

/// What a project or command asks for. Ranges are never accepted: a pin names one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum NodeSpec {
    Major(u32),
    Minor(u32, u32),
    Exact(NodeVersion),
    /// `lts/<codename>`, lowercased.
    Lts(String),
    /// `lts/*`: the newest LTS line.
    LtsLatest,
}

impl fmt::Display for NodeSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeSpec::Major(major) => write!(f, "{major}"),
            NodeSpec::Minor(major, minor) => write!(f, "{major}.{minor}"),
            NodeSpec::Exact(v) => write!(f, "{v}"),
            NodeSpec::Lts(name) => write!(f, "lts/{name}"),
            NodeSpec::LtsLatest => f.write_str("lts/*"),
        }
    }
}

impl TryFrom<String> for NodeSpec {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        parse_spec(&s)
    }
}

impl From<NodeSpec> for String {
    fn from(spec: NodeSpec) -> Self {
        spec.to_string()
    }
}

/// The oldest major with an arm64 macOS build.
pub const MIN_MAJOR: u32 = 16;
/// What runs where nothing pins a version and `yard.toml` sets no `default_node`.
pub const DEFAULT_SPEC: NodeSpec = NodeSpec::Major(22);

/// Words nvm and `.node-version` files accept that name no fixed version.
const NOT_VERSIONS: [&str; 5] = ["node", "stable", "latest", "system", "iojs"];

/// Parses a spec as a command, `.yard`, `.nvmrc` or `.node-version` gives it. The error
/// says why Yard refuses it.
pub fn parse_spec(s: &str) -> Result<NodeSpec, String> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if NOT_VERSIONS.contains(&lower.as_str()) {
        return Err(format!("{s:?} is not a version: name one, such as 22"));
    }
    if lower == "lts/*" {
        return Ok(NodeSpec::LtsLatest);
    }
    if let Some(name) = lower.strip_prefix("lts/")
        && !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_alphabetic())
    {
        return Ok(NodeSpec::Lts(name.to_string()));
    }
    let bare = lower.strip_prefix('v').unwrap_or(&lower);
    if s.contains(['<', '>', '=', '^', '~', '|', ' ', '*']) || bare.split('.').any(|part| part == "x") {
        return Err(format!("{s:?} is a range; Yard runs one version, such as 22"));
    }
    let not_node = || format!("{s:?} is not a Node version");
    let parts = numbers(bare).ok_or_else(not_node)?;
    let spec = match parts[..] {
        [major] => NodeSpec::Major(major),
        [major, minor] => NodeSpec::Minor(major, minor),
        [major, minor, patch] => NodeSpec::Exact(NodeVersion { major, minor, patch }),
        _ => return Err(not_node()),
    };
    if parts[0] < MIN_MAJOR {
        return Err(format!(
            "Node {} is older than {MIN_MAJOR}, the oldest Node built for Apple Silicon",
            parts[0]
        ));
    }
    Ok(spec)
}

/// The 1-based line and trimmed text of the first line that is neither blank nor a `#`
/// comment, as nvm reads `.nvmrc`. Handles CRLF.
pub fn spec_line(text: &str) -> Option<(usize, &str)> {
    text.lines()
        .map(str::trim)
        .enumerate()
        .find(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(i, line)| (i + 1, line))
}

/// LTS codenames to majors. Built in, then refreshed into `node/lts.toml` from each
/// release listing, so a new LTS name resolves once Yard has seen it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LtsMap(pub BTreeMap<String, u32>);

pub const LTS_HEADER: &str = "# Generated by Yard from nodejs.org's release list; rewritten by yard node install and yard node list --available.\n";

impl LtsMap {
    pub fn builtin() -> LtsMap {
        let names = [
            ("argon", 4),
            ("boron", 6),
            ("carbon", 8),
            ("dubnium", 10),
            ("erbium", 12),
            ("fermium", 14),
            ("gallium", 16),
            ("hydrogen", 18),
            ("iron", 20),
            ("jod", 22),
            ("krypton", 24),
        ];
        LtsMap(names.into_iter().map(|(name, major)| (name.to_string(), major)).collect())
    }

    /// The major an LTS spec names; `None` for the other forms and an unknown codename.
    pub fn major(&self, spec: &NodeSpec) -> Option<u32> {
        match spec {
            NodeSpec::Lts(name) => self.0.get(name).copied(),
            NodeSpec::LtsLatest => self.0.values().max().copied(),
            _ => None,
        }
    }

    /// `lts.toml`; `None` when it does not parse.
    pub fn parse(text: &str) -> Option<LtsMap> {
        toml::from_str(text).ok().map(LtsMap)
    }

    pub fn render(&self) -> String {
        let mut names: Vec<(&String, &u32)> = self.0.iter().collect();
        names.sort_by_key(|(name, major)| (**major, name.as_str()));
        let mut out = LTS_HEADER.to_string();
        for (name, major) in names {
            out.push_str(&format!("{name} = {major}\n"));
        }
        out
    }
}

pub fn matches(spec: &NodeSpec, v: NodeVersion, lts: &LtsMap) -> bool {
    match spec {
        NodeSpec::Major(major) => v.major == *major,
        NodeSpec::Minor(major, minor) => v.major == *major && v.minor == *minor,
        NodeSpec::Exact(exact) => v == *exact,
        NodeSpec::Lts(_) | NodeSpec::LtsLatest => lts.major(spec) == Some(v.major),
    }
}

/// The newest installed version the spec accepts.
pub fn pick(spec: &NodeSpec, installed: &[NodeVersion], lts: &LtsMap) -> Option<NodeVersion> {
    installed.iter().copied().filter(|v| matches(spec, *v, lts)).max()
}

/// One line of `fnm ls-remote`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: NodeVersion,
    /// The LTS codename, lowercased.
    pub lts: Option<String>,
}

/// `fnm ls-remote`'s lines, `v22.18.0 (Jod)` or `v25.1.0`; anything else is skipped.
pub fn parse_ls_remote(text: &str) -> Vec<Release> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (version, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let version = version.parse().ok()?;
            let lts = rest
                .trim()
                .strip_prefix('(')
                .and_then(|r| r.strip_suffix(')'))
                .map(|name| name.trim().to_ascii_lowercase())
                .filter(|name| !name.is_empty());
            Some(Release { version, lts })
        })
        .collect()
}

/// The built-in map with every codename in `releases` laid over it.
pub fn lts_from(releases: &[Release]) -> LtsMap {
    let mut map = LtsMap::builtin();
    for release in releases {
        if let Some(name) = &release.lts {
            map.0.insert(name.clone(), release.version.major);
        }
    }
    map
}

/// The newest release the spec accepts, from Node 16 up.
pub fn newest(spec: &NodeSpec, releases: &[Release], lts: &LtsMap) -> Option<NodeVersion> {
    releases.iter().map(|r| r.version).filter(|v| v.major >= MIN_MAJOR && matches(spec, *v, lts)).max()
}

/// The newest release of each major from 16 up, newest major first; only `major` when given.
pub fn newest_per_line(releases: &[Release], major: Option<u32>) -> Vec<Release> {
    let mut lines: BTreeMap<u32, &Release> = BTreeMap::new();
    for release in releases.iter().filter(|r| r.version.major >= MIN_MAJOR) {
        if major.is_some_and(|m| m != release.version.major) {
            continue;
        }
        let newest = lines.entry(release.version.major).or_insert(release);
        if release.version > newest.version {
            *newest = release;
        }
    }
    lines.into_values().rev().cloned().collect()
}

/// The versions under fnm's `node-versions/` that have `installation/bin/node`, oldest
/// first. A directory a cancelled download left behind has no `node` and is not counted.
pub fn installed(
    versions_dir: &Path,
    list: &dyn Fn(&Path) -> Vec<String>,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<NodeVersion> {
    let mut found: Vec<NodeVersion> = list(versions_dir)
        .iter()
        .filter(|name| name.starts_with('v'))
        .filter_map(|name| Some((name.parse().ok()?, name)))
        .filter(|(_, name)| exists(&versions_dir.join(name).join("installation/bin/node")))
        .map(|(v, _)| v)
        .collect();
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn specs_parse_to_their_normal_form() {
        for (input, shown) in [
            ("22", "22"),
            ("v22", "22"),
            (" 22.17 ", "22.17"),
            ("16.14.0", "16.14.0"),
            ("v16.14.0", "16.14.0"),
            ("lts/iron", "lts/iron"),
            ("lts/Iron", "lts/iron"),
            ("lts/*", "lts/*"),
        ] {
            assert_eq!(parse_spec(input).unwrap().to_string(), shown, "{input}");
        }
    }

    #[test]
    fn specs_yard_refuses_say_why() {
        for (input, says) in [
            ("node", "not a version"),
            ("stable", "not a version"),
            ("latest", "not a version"),
            ("system", "not a version"),
            ("iojs", "not a version"),
            (">=18", "range"),
            ("^20", "range"),
            ("20.x", "range"),
            ("18 || 20", "range"),
            ("14", "older than 16"),
            ("14.21.3", "older than 16"),
            ("banana", "not a Node version"),
            ("", "not a Node version"),
        ] {
            let e = parse_spec(input).unwrap_err();
            assert!(e.contains(says), "{input}: {e}");
        }
    }

    #[test]
    fn spec_line_skips_blanks_comments_and_crlf() {
        assert_eq!(spec_line("v16.14.0\r\n"), Some((1, "v16.14.0")));
        assert_eq!(spec_line("# pinned for node-sass\n\n16.14.0\n"), Some((3, "16.14.0")));
        assert_eq!(spec_line("# nothing\n\n"), None);
    }

    #[test]
    fn pick_takes_the_newest_match() {
        let installed: Vec<NodeVersion> = ["16.14.0", "20.19.4", "22.17.1", "22.18.0", "24.5.0"]
            .iter()
            .map(|v| v.parse().unwrap())
            .collect();
        let lts = LtsMap::builtin();
        let at = |s: &str| pick(&parse_spec(s).unwrap(), &installed, &lts).map(|v| v.to_string());
        assert_eq!(at("22").as_deref(), Some("22.18.0"));
        assert_eq!(at("22.17").as_deref(), Some("22.17.1"));
        assert_eq!(at("16.14.0").as_deref(), Some("16.14.0"));
        assert_eq!(at("16.20.2"), None); // an exact pin means exactly that version
        assert_eq!(at("lts/iron").as_deref(), Some("20.19.4"));
        assert_eq!(at("lts/*").as_deref(), Some("24.5.0")); // krypton, the built-in map's highest
        assert_eq!(at("lts/argon"), None);
        assert_eq!(at("18"), None);
    }

    #[test]
    fn lts_toml_round_trips_and_a_broken_one_is_none() {
        let map = lts_from(&parse_ls_remote("v26.1.0 (Lithium)\n"));
        let text = map.render();
        assert!(text.starts_with(LTS_HEADER), "{text}");
        assert_eq!(LtsMap::parse(&text), Some(map.clone()));
        assert_eq!(map.major(&NodeSpec::LtsLatest), Some(26));
        assert_eq!(map.major(&NodeSpec::Lts("iron".into())), Some(20));
        assert_eq!(LtsMap::parse("iron = "), None);
    }

    #[test]
    fn ls_remote_listing_parses_and_newest_resolves() {
        let text = "v14.21.3 (Fermium)\nv16.14.0\nv16.20.2 (Gallium)\nv22.17.1\nv22.18.0 (Jod)\nv25.1.0\nsome banner\n";
        let releases = parse_ls_remote(text);
        assert_eq!(releases.len(), 6);
        assert_eq!(releases[4].lts.as_deref(), Some("jod"));
        let lts = lts_from(&releases);
        let newest = |s: &str| newest(&parse_spec(s).unwrap(), &releases, &lts).map(|v| v.to_string());
        assert_eq!(newest("22").as_deref(), Some("22.18.0"));
        assert_eq!(newest("16.14.0").as_deref(), Some("16.14.0"));
        assert_eq!(newest("lts/gallium").as_deref(), Some("16.20.2"));
        assert_eq!(newest("19"), None);
        let lines: Vec<String> =
            newest_per_line(&releases, None).iter().map(|r| r.version.to_string()).collect();
        assert_eq!(lines, ["25.1.0", "22.18.0", "16.20.2"]); // 14 is below the floor
        assert_eq!(newest_per_line(&releases, Some(16)).len(), 1);
    }

    #[test]
    fn installed_needs_a_node_binary() {
        let dir = Path::new("/r/node/node-versions");
        let names = vec!["v22.18.0".to_string(), "v16.14.0".into(), "v20.0.0".into(), ".DS_Store".into()];
        let list = |_: &Path| names.clone();
        let exists = |p: &Path| !p.starts_with("/r/node/node-versions/v20.0.0"); // half-installed
        let got: Vec<String> = installed(dir, &list, &exists).iter().map(|v| v.to_string()).collect();
        assert_eq!(got, ["16.14.0", "22.18.0"]);
    }
}

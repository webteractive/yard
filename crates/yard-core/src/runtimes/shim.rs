//! What a recipe runtime's shim runs (runtimes spec §8, §9): the version a project gets,
//! the command for it, and the arguments Yard refuses.

use super::recipe::{Refuse, RuntimeRecipe, ShimDef, Vars, expand};
use super::resolve::PinSource;
use super::select::{InstallRecord, fill_candidate, fitting, needs_pin, offer_spec, requires_php};
use super::version::{RuntimeVersion, VersionSpec, pick};
use crate::Paths;
use crate::shim::{ShimCommand, child_path};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use yard_proto::{ErrorCode, YardError};

/// Whether an option word may take the next word as its value: `--working-dir`, or a
/// short-option word ending in `d` (`-d`, Composer's working directory). Symfony's console
/// skips the next word when it equals the value bound to the word's last letter, wherever that
/// value came from (`-nd x`, and `-dd d`, whose `-d` binds the cluster's `d`), so the shim skips
/// it whenever it might: skipping too much only refuses too much.
fn takes_next(word: &str) -> bool {
    word == "--working-dir" || (!word.starts_with("--") && word.len() > 1 && word.ends_with('d'))
}

/// The `refuse` entry `args` meet. The command is the first argument that is neither an
/// option nor an option's value (`-d <dir>`), past `global`, which runs any command again;
/// it matches a refused word it abbreviates, as Composer's console accepts `self-u` or
/// `SELF-UPDATE`. Any further words of the entry follow it exactly. A word taken as `-d`'s
/// value that names a refused command is refused too: when an option Composer does not bind
/// globally (`-r`, `--rollback`) makes its first pass fail, that word is the command after all.
/// After `global`, an option word is split at whitespace, as Composer's second parse splits it.
/// The install root is read-only besides, so a spelling this misses still cannot replace the phar.
pub fn refused<'a>(def: &'a ShimDef, args: &[OsString]) -> Option<&'a Refuse> {
    let mut words: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let names = |word: &str, r: &Refuse| {
        let word = word.to_lowercase();
        r.args.first().is_some_and(|first| !word.is_empty() && first.to_lowercase().starts_with(&word))
    };
    let mut at = 0;
    loop {
        while at < words.len() && words[at].starts_with('-') {
            if takes_next(&words[at]) {
                let value = words.get(at + 1).map(String::as_str).unwrap_or_default();
                if let Some(r) = def.refuse.iter().find(|r| names(value, r)) {
                    return Some(r);
                }
                at += 2;
            } else {
                at += 1;
            }
        }
        let command = words.get(at)?.to_lowercase();
        if !command.is_empty() && "global".starts_with(&command) && at + 1 < words.len() {
            // `global` runs the rest again from one string, in which Symfony writes an option
            // word unquoted up to its first `=`, so `"-n self-update"` comes back as two words.
            let rest: Vec<String> = words
                .drain(at + 1..)
                .flat_map(|w| match w.starts_with('-') {
                    true => w.split_whitespace().map(String::from).collect(),
                    false => vec![w],
                })
                .collect();
            words.extend(rest);
            at += 1;
            continue;
        }
        let rest = &words[at + 1..];
        return def.refuse.iter().find(|r| names(&command, r) && rest.starts_with(&r.args[1..]));
    }
}

/// The command shim `shim` of `recipe` runs for `version`, installed at `home`: its `exec`
/// expanded, then the caller's arguments. With `runs_on = "php"`, `php` is the `php` shim's
/// command for the project: `{php}` is its program and its `PHPRC` and `PHP_INI_SCAN_DIR`
/// carry over. PATH is the version's `path` dirs, the PHP keg's `bin`, Yard's `bin`, then
/// `inherited` without Herd's directories.
pub fn recipe_command(
    recipe: &RuntimeRecipe,
    shim: &str,
    home: &Path,
    version: &RuntimeVersion,
    php: Option<&ShimCommand>,
    paths: &Paths,
    inherited: &OsStr,
) -> Result<ShimCommand, YardError> {
    let def = recipe.shims.get(shim).ok_or_else(|| {
        YardError::new(ErrorCode::InvalidArgument, format!("{} has no {shim} command", recipe.name))
    })?;
    let version_text = version.to_string();
    let cache = paths.runtime_cache(&recipe.key);
    let vars = Vars {
        home,
        version: &version_text,
        cache: &cache,
        root: paths.root(),
        php: php.map(|(program, ..)| program.as_path()),
        node: None,
    };
    let mut words = def.exec.iter().map(|w| expand(w, &vars));
    let program = PathBuf::from(words.next().unwrap_or_default());
    let pre: Vec<OsString> = words.map(OsString::from).collect();

    let mut env: Vec<(String, String)> =
        php.map(|(.., env)| env.iter().filter(|(k, _)| k != "PATH").cloned().collect()).unwrap_or_default();
    env.extend(recipe.env.iter().map(|(k, v)| (k.clone(), expand(v, &vars))));

    let mut first: Vec<PathBuf> = recipe.path.iter().map(|d| home.join(d)).collect();
    if let Some(keg_bin) = php.and_then(|(program, ..)| program.parent()) {
        first.push(keg_bin.to_path_buf());
    }
    first.push(paths.bin_dir());
    let rest: Vec<PathBuf> =
        std::env::split_paths(&child_path(None, inherited)).filter(|p| !first.contains(p)).collect();
    let path = std::env::join_paths(first.into_iter().chain(rest)).unwrap_or_default();
    env.push(("PATH".to_string(), path.to_string_lossy().into_owned()));
    Ok((program, pre, env))
}

/// What the `composer` shim runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// A Yard-installed version and its install root.
    Run(RuntimeVersion, PathBuf),
    /// Brew's `bin/composer`, for one release, while Yard has no Composer at all
    /// (runtimes spec §9.3).
    BrewComposer(PathBuf),
}

/// The Composer a project runs (runtimes spec §9.2, explicit-defaults spec §2). A pin
/// (`[runtimes] composer`, the site's setting, or `yard.toml`'s default passed as
/// [`PinSource::Default`]) is a prefix, and an exact one is itself; the newest installed
/// match runs, unless it needs a newer PHP than the project's (`INVALID_ARGUMENT`, naming
/// where the PHP came from, `php_from`, and the version that fits). With no pin nothing is
/// picked by rule: while Yard has no Composer installed at all, brew's `bin/composer` runs
/// where the project's PHP can run it; otherwise it is `RUNTIME_MISSING`.
pub fn pick_composer(
    recipe: &RuntimeRecipe,
    pin: Option<(VersionSpec, PinSource)>,
    php_full: &RuntimeVersion,
    php_from: &str,
    installed: &[(RuntimeVersion, InstallRecord)],
    brew_composer: Option<PathBuf>,
    paths: &Paths,
) -> Result<Pick, YardError> {
    let versions: Vec<RuntimeVersion> = installed.iter().map(|(v, _)| v.clone()).collect();
    let key = &recipe.key;
    let Some((spec, source)) = pin else {
        let newest_needs = recipe.requires.iter().map(|r| &r.php).max();
        if installed.is_empty()
            && let Some(brew) = brew_composer
            && newest_needs.is_none_or(|needs| php_full >= needs)
        {
            return Ok(Pick::BrewComposer(brew));
        }
        let (message, hint) = if needs_pin(recipe, None, &versions, php_full) {
            let message = format!(
                "{} has no pin for this project, whose PHP {php_full} ({php_from}) is older than its default needs",
                recipe.name
            );
            (message, fit_hint(recipe, &versions, php_full))
        } else {
            let message = format!("no {} default is written in yard.toml", recipe.name);
            let hint = match fill_candidate(recipe, &versions) {
                Some(v) => format!("yard runtime default {key} {v}"),
                None => format!("yard runtime install {key}"),
            };
            (message, hint)
        };
        return Err(YardError::new(ErrorCode::RuntimeMissing, message).with_hint(hint));
    };
    if let Some(v) = pick(&spec, &versions) {
        let record = installed.iter().find(|(iv, _)| *iv == v).map(|(_, r)| r);
        let needs = record.and_then(|r| r.requires_php.clone()).or_else(|| requires_php(recipe, &v));
        if let Some(needs) = needs.filter(|needs| php_full < needs) {
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "{} {v} needs PHP {needs} or later; this project runs PHP {php_full} ({php_from})",
                    recipe.name
                ),
            )
            .with_hint(fit_hint(recipe, &versions, php_full))
            .with_data(serde_json::json!({ "version": v.to_string(), "needs_php": needs.to_string() })));
        }
        return Ok(Pick::Run(v.clone(), paths.install_root(key, &v.to_string())));
    }
    Err(missing(recipe, &spec, &source))
}

/// What a project on `php_full` should run instead: `yard runtime use` of the newest
/// installed version that runs on it, else the install of the one Yard offers it.
pub fn fit_hint(recipe: &RuntimeRecipe, installed: &[RuntimeVersion], php_full: &RuntimeVersion) -> String {
    let key = &recipe.key;
    match fitting(recipe, installed, php_full) {
        Some(v) => format!("yard runtime use {key} {v}"),
        None => match offer_spec(recipe, php_full, true) {
            Some(spec) => format!("yard runtime install {key} {spec}"),
            None => format!("yard runtime install {key}"),
        },
    }
}

/// `RUNTIME_MISSING`: `<Name> <spec> is not installed (<where the spec came from>)`.
pub fn missing(recipe: &RuntimeRecipe, spec: &VersionSpec, source: &PinSource) -> YardError {
    YardError::new(ErrorCode::RuntimeMissing, format!("{} {spec} is not installed ({source})", recipe.name))
        .with_hint(format!("yard runtime install {} {spec}", recipe.key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtimes::Runtimes;
    use crate::runtimes::select::InstallRecord;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use yard_proto::ErrorCode;

    fn v(s: &str) -> RuntimeVersion {
        s.parse().unwrap()
    }

    fn spec(s: &str) -> VersionSpec {
        VersionSpec::parse(s).unwrap()
    }

    fn composer() -> RuntimeRecipe {
        Runtimes::carried().get("composer").unwrap().recipe().clone()
    }

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn installs(versions: &[&str]) -> Vec<(RuntimeVersion, InstallRecord)> {
        let r = composer();
        versions
            .iter()
            .map(|s| {
                let version = v(s);
                let record = InstallRecord {
                    requires_php: crate::runtimes::select::requires_php(&r, &version),
                    version: version.clone(),
                    url: String::new(),
                    sha256: "a".repeat(64),
                    installed_at: String::new(),
                };
                (version, record)
            })
            .collect()
    }

    fn pick(
        pin: Option<(&str, PinSource)>,
        php: &str,
        installed: &[&str],
        brew: Option<&str>,
    ) -> Result<Pick, YardError> {
        pick_composer(
            &composer(),
            pin.map(|(s, src)| (spec(s), src)),
            &v(php),
            "the default",
            &installs(installed),
            brew.map(PathBuf::from),
            &Paths::new("/r"),
        )
    }

    /// A short-option cluster ending in `d` takes the next word as `-d`'s directory, as
    /// Symfony's console reads it, so `-nd /tmp self-update` is still `self-update`; one with
    /// `d` before its end carries the directory itself (`-d/tmp`, `-dn`).
    #[test]
    fn a_cluster_ending_in_d_takes_the_next_word() {
        let r = composer();
        let def = &r.shims["composer"];
        for refused_args in [
            &["-nd", "/tmp", "self-update"][..],
            &["-qnd", "/tmp", "selfupdate"],
            &["-d/tmp", "self-update"],
            &["-dn", "self-update"],
        ] {
            assert!(refused(def, &args(refused_args)).is_some(), "{refused_args:?}");
        }
        // Composer binds only its global options first, and when another option (`-r`, a
        // self-update one) makes that fail, the word after `-d` is the command after all.
        for refused_args in [
            &["-rd", "self-update"][..],
            &["--rollback", "-nd", "self-update"],
            &["--stable", "-nd", "SELF-UPDATE", "--set-channel-only"],
            &["--working-dir", "selfupdate"],
        ] {
            assert!(refused(def, &args(refused_args)).is_some(), "{refused_args:?}");
        }
        assert!(refused(def, &args(&["-d", "/code/self-update", "install"])).is_none());
    }

    #[test]
    fn refuse_matches_the_first_non_option_argument() {
        let r = composer();
        let def = &r.shims["composer"];
        for refused_args in
            [&["self-update"][..], &["-n", "self-update"], &["--no-ansi", "selfupdate", "--2.2"]]
        {
            let got = refused(def, &args(refused_args));
            assert_eq!(
                got.map(|r| r.hint.as_str()),
                Some("yard runtime install composer <version>"),
                "{refused_args:?}"
            );
        }
        for ok in [&["install"][..], &["require", "self-update/x"], &[]] {
            assert!(refused(def, &args(ok)).is_none(), "{ok:?}");
        }
    }

    #[test]
    fn the_phar_runs_under_the_projects_php() {
        let paths = Paths::new("/r");
        let opt = Path::new("/b/opt/php@7.1");
        let ini = paths.php_ini("7.1".parse().unwrap());
        let exists = |p: &Path| p == ini;
        let inherited = "/x/Herd/bin:/usr/bin:/r/bin";
        let php =
            crate::shim::command("php", "7.1".parse().unwrap(), opt, &paths, inherited.as_ref(), &exists)
                .unwrap();
        let home = paths.install_root("composer", "2.2.32");
        let (program, pre, env) = recipe_command(
            &composer(),
            "composer",
            &home,
            &v("2.2.32"),
            Some(&php),
            &paths,
            inherited.as_ref(),
        )
        .unwrap();
        assert_eq!(program, Path::new("/b/opt/php@7.1/bin/php"));
        assert_eq!(pre, [OsString::from("/r/installs/composer/2.2.32/composer.phar")]);
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("PHPRC").as_deref(), Some("/r/php/7.1"));
        assert!(get("PHP_INI_SCAN_DIR").is_some());
        assert_eq!(
            get("PATH").as_deref(),
            Some("/r/installs/composer/2.2.32/bin:/b/opt/php@7.1/bin:/r/bin:/usr/bin")
        );
        assert!(!get("PATH").unwrap().contains("Herd"));
        let e =
            recipe_command(&composer(), "npm", &home, &v("2.2.32"), Some(&php), &paths, inherited.as_ref())
                .unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn pick_composer_with_no_pin_and_installs_is_missing_not_a_rule() {
        let e = pick(None, "8.4.27", &["2.11.2"], None).unwrap_err();
        assert_eq!(e.code, ErrorCode::RuntimeMissing);
        assert_eq!(e.message, "no Composer default is written in yard.toml");
        assert_eq!(e.hint.as_deref(), Some("yard runtime default composer 2.11.2"));
        let e = pick(None, "8.4.27", &["2.2.32"], None).unwrap_err();
        assert_eq!(e.hint.as_deref(), Some("yard runtime install composer"));
        let e = pick(None, "7.1.33", &["2.2.32", "2.11.2"], None).unwrap_err();
        assert_eq!(e.hint.as_deref(), Some("yard runtime use composer 2.2.32"));
    }

    #[test]
    fn the_written_default_runs_exactly() {
        let default = Some(("2.11.2", PinSource::Default));
        assert_eq!(
            pick(default, "8.4.27", &["2.2.32", "2.11.2", "2.12.0"], None).unwrap(),
            Pick::Run(v("2.11.2"), PathBuf::from("/r/installs/composer/2.11.2"))
        );
    }

    #[test]
    fn a_pin_beats_the_default() {
        let pin = Some(("2.2", PinSource::Project("/code/shop/.yard".into())));
        assert_eq!(
            pick(pin, "8.4.27", &["2.2.32", "2.11.2"], None).unwrap(),
            Pick::Run(v("2.2.32"), PathBuf::from("/r/installs/composer/2.2.32"))
        );
        let pin = Some(("2.11", PinSource::Project("/code/shop/.yard".into())));
        let e = pick(pin, "8.4.27", &["2.2.32"], None).unwrap_err();
        assert_eq!(e.message, "Composer 2.11 is not installed (/code/shop/.yard)");
    }

    #[test]
    fn a_pin_too_new_for_the_php_is_refused() {
        let pin = Some(("2", PinSource::Project("/code/old/.yard".into())));
        let e = pick(pin, "7.1.33", &["2.11.2"], None).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument);
        assert_eq!(
            e.message,
            "Composer 2.11.2 needs PHP 7.2.5 or later; this project runs PHP 7.1.33 (the default)"
        );
        assert_eq!(e.hint.as_deref(), Some("yard runtime install composer 2.2"));
        let pin = Some(("2.11.2", PinSource::Default));
        let e = pick(pin, "7.1.33", &["2.2.32", "2.11.2"], None).unwrap_err();
        assert_eq!(e.hint.as_deref(), Some("yard runtime use composer 2.2.32"));
    }

    #[test]
    fn brews_composer_runs_only_when_yard_has_none() {
        let brew = Some("/b/bin/composer");
        assert_eq!(
            pick(None, "8.4.27", &[], brew).unwrap(),
            Pick::BrewComposer(PathBuf::from("/b/bin/composer"))
        );
        let e = pick(None, "7.1.33", &[], brew).unwrap_err();
        assert_eq!(e.code, ErrorCode::RuntimeMissing);
        assert_eq!(e.hint.as_deref(), Some("yard runtime install composer 2.2"));
        assert_eq!(pick(None, "8.4.27", &["2.2.32"], brew).unwrap_err().code, ErrorCode::RuntimeMissing);
    }
}

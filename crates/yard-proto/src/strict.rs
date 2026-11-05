//! Strict readers for enums that carry a fallback (`Other`) for values a newer yardd may send.
//! A fallback serves a client reading answers; the daemon's own inputs (`ports.toml`, the
//! `ports.profile` params) must still refuse a name they do not know.

use crate::{PortMode, Profile};
use serde::de::{Deserialize, Deserializer, Error};

const PROFILES: &[&str] = &["standard", "privileged"];
const PORT_MODES: &[&str] = &["pinned", "preferred", "auto", "external"];

/// A `Profile` that is never `Other`; `trial` and `default` still read as their aliases.
pub fn profile<'de, D: Deserializer<'de>>(d: D) -> Result<Profile, D::Error> {
    known(d, PROFILES, |p| p == Profile::Other)
}

/// As [`profile`], for an optional field.
pub fn opt_profile<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Profile>, D::Error> {
    profile(d).map(Some)
}

/// A `PortMode` that is never `Other`.
pub fn port_mode<'de, D: Deserializer<'de>>(d: D) -> Result<PortMode, D::Error> {
    known(d, PORT_MODES, |m| m == PortMode::Other)
}

fn known<'de, D, T>(d: D, names: &'static [&'static str], is_other: fn(T) -> bool) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Copy + serde::de::DeserializeOwned,
{
    let name = String::deserialize(d)?;
    match serde_json::from_value::<T>(serde_json::Value::String(name.clone())) {
        Ok(v) if !is_other(v) => Ok(v),
        _ => Err(D::Error::unknown_variant(&name, names)),
    }
}

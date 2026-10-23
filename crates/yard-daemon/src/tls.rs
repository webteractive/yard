//! Yard's local CA and the certificates it issues to secured sites (spec §5.4).
//! `yard_core::tls` decides when a site needs one; this makes them. Keys are written
//! mode 0600 from the start, never briefly readable by others.

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer,
    KeyPair, KeyUsagePurpose, PublicKeyData,
};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use time::{Duration, OffsetDateTime};
use x509_parser::extensions::GeneralName;
use x509_parser::pem::parse_x509_pem;
use yard_core::Paths;
use yard_core::tls::{CA_DAYS, CertFacts, ca_needs_renewal};
use yard_proto::YardError;

/// Makes the CA when `ca/yard-ca.pem` or its key is missing, does not parse, or does not
/// match, or when the CA would expire before a leaf issued now. A CA it replaces is kept
/// as `yard-ca.pem.bak` and `yard-ca.key.bak`, so one still trusted in the keychain can be
/// found. A file that cannot be read is an error, never a reason to replace the CA.
/// `Ok(true)` when it made one.
pub fn ensure_ca(paths: &Paths, user: &str, now: OffsetDateTime) -> Result<bool, YardError> {
    if let Some(ca) = load_ca(paths)?
        && !ca_needs_renewal(ca.not_after, now.unix_timestamp())
    {
        return Ok(false);
    }
    let mut params = CertificateParams::default();
    params.distinguished_name = name(&format!("Yard CA ({user})"));
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages =
        vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(CA_DAYS);
    let key = KeyPair::generate().map_err(|e| failed("generate the CA key", e))?;
    let cert = params.self_signed(&key).map_err(|e| failed("make the CA", e))?;
    private_dir(&paths.ca_dir())?;
    for old in [paths.ca_cert(), paths.ca_key()] {
        if old.exists() {
            std::fs::rename(&old, bak(&old)).map_err(|e| io(&old, e))?;
        }
    }
    write_private(&paths.ca_key(), &key.serialize_pem())?;
    write_file(&paths.ca_cert(), &cert.pem())?;
    Ok(true)
}

/// Whether `ensure_ca` would make a new CA at `now`: it is missing or unusable, or a leaf
/// issued now would outlive it. A CA that cannot be read is not due; reconcile reports it.
pub fn ca_due(paths: &Paths, now: OffsetDateTime) -> bool {
    match load_ca(paths) {
        Ok(Some(ca)) => ca_needs_renewal(ca.not_after, now.unix_timestamp()),
        Ok(None) => true,
        Err(_) => false,
    }
}

/// Issues `site`'s certificate for `host` and `*.host`, valid `days` from `now`, with a
/// fresh P-256 key, signed by the stored CA.
pub fn issue(paths: &Paths, site: &str, host: &str, now: OffsetDateTime, days: i64) -> Result<(), YardError> {
    issue_at(paths, &paths.certs_dir(), &leaf_files(&paths.certs_dir(), site), host, now, days)
}

/// As `issue`, for a service's hostname (`minio.<tld>`): `certs/services/<name>.pem`, kept
/// apart from the sites' so `prune_certs` and a site of the same name never meet.
pub fn issue_service(
    paths: &Paths,
    name: &str,
    host: &str,
    now: OffsetDateTime,
    days: i64,
) -> Result<(), YardError> {
    let dir = paths.services_certs_dir();
    private_dir(&paths.certs_dir())?;
    issue_at(paths, &dir, &leaf_files(&dir, name), host, now, days)
}

/// `(<dir>/<name>.pem, <dir>/<name>.key)`.
fn leaf_files(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    (dir.join(format!("{name}.pem")), dir.join(format!("{name}.key")))
}

fn issue_at(
    paths: &Paths,
    dir: &Path,
    (cert_file, key_file): &(PathBuf, PathBuf),
    host: &str,
    now: OffsetDateTime,
    days: i64,
) -> Result<(), YardError> {
    let ca = load_ca(paths)?.ok_or_else(|| YardError::internal("there is no usable CA to sign with"))?;
    let issuer = Issuer::from_ca_cert_pem(&ca.pem, ca.key).map_err(|e| failed("read the CA", e))?;
    let mut params = CertificateParams::new(vec![host.to_string(), format!("*.{host}")])
        .map_err(|e| failed(&format!("name a certificate for {host}"), e))?;
    params.distinguished_name = name(host);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.use_authority_key_identifier_extension = true;
    // A day back, so a clock slightly behind still accepts it.
    params.not_before = now - Duration::days(1);
    // Never past the CA, which would fail every client the day the CA expires.
    let ca_end =
        OffsetDateTime::from_unix_timestamp(ca.not_after).map_err(|e| failed("read the CA's expiry", e))?;
    params.not_after = (now + Duration::days(days)).min(ca_end);
    let key = KeyPair::generate().map_err(|e| failed("generate a site key", e))?;
    let cert =
        params.signed_by(&key, &issuer).map_err(|e| failed(&format!("sign {host}'s certificate"), e))?;
    private_dir(dir)?;
    write_private(key_file, &key.serialize_pem())?;
    write_file(cert_file, &cert.pem())
}

/// What reconcile needs to know about `site`'s certificate; `None` when it or its key is
/// missing or does not parse, or the key is not the certificate's: nginx refuses all three.
pub fn facts(paths: &Paths, site: &str) -> Option<CertFacts> {
    facts_against(paths, site, std::fs::read(paths.ca_cert()).ok().as_deref())
}

/// As `facts`, against `ca_pem` read once by the caller, e.g. for every secured site.
pub fn facts_against(paths: &Paths, site: &str, ca_pem: Option<&[u8]>) -> Option<CertFacts> {
    facts_of(&paths.cert(site), &paths.cert_key(site), ca_pem)
}

/// As `facts_against`, for a service's certificate under `certs/services/`.
pub fn service_facts_against(paths: &Paths, name: &str, ca_pem: Option<&[u8]>) -> Option<CertFacts> {
    let (cert, key) = leaf_files(&paths.services_certs_dir(), name);
    facts_of(&cert, &key, ca_pem)
}

fn facts_of(cert: &Path, key: &Path, ca_pem: Option<&[u8]>) -> Option<CertFacts> {
    let leaf_pem = std::fs::read(cert).ok()?;
    let (_, leaf) = parse_x509_pem(&leaf_pem).ok()?;
    let leaf = leaf.parse_x509().ok()?;
    let key = KeyPair::from_pem(&std::fs::read_to_string(key).ok()?).ok()?;
    if key.subject_public_key_info() != leaf.public_key().raw {
        return None;
    }
    let dns_names = leaf
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    GeneralName::DNSName(d) => Some(d.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let signed_by_ca = ca_pem.and_then(|p| parse_x509_pem(p).ok()).is_some_and(|(_, ca)| {
        ca.parse_x509().is_ok_and(|ca| leaf.verify_signature(Some(ca.public_key())).is_ok())
    });
    Some(CertFacts { dns_names, not_after: leaf.validity().not_after.timestamp(), signed_by_ca })
}

/// How long `security verify-cert` may take; a stalled trustd would otherwise hold a
/// doctor or `ca.info` call forever.
pub const SECURITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether `security` (`/usr/bin/security` outside tests) trusts the CA at `ca` for TLS,
/// as the login keychain's settings decide. `None` when `security` could not run or did
/// not answer within [`SECURITY_TIMEOUT`].
pub async fn trusted(security: &Path, ca: &Path) -> Option<bool> {
    let mut cmd = tokio::process::Command::new(security);
    cmd.args(["verify-cert", "-c"])
        .arg(ca)
        .args(["-l", "-L", "-q", "-p", "basic"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // SAFETY: the hook makes only async-signal-safe calls.
    unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
    crate::fds::settle_before_fork();
    let status = cmd.status();
    match tokio::time::timeout(SECURITY_TIMEOUT, status).await {
        Ok(Ok(status)) => Some(status.success()),
        Ok(Err(_)) | Err(_) => None,
    }
}

/// Upper-case hex SHA-256 of the CA certificate's DER, as `security -Z` takes it.
pub fn ca_sha256(paths: &Paths) -> Result<String, YardError> {
    let der = ca_der(paths)?;
    Ok(Sha256::digest(&der).iter().map(|b| format!("{b:02X}")).collect())
}

/// The day the CA expires, `YYYY-MM-DD`.
pub fn ca_expires(paths: &Paths) -> Result<String, YardError> {
    let der = ca_der(paths)?;
    let (_, ca) = x509_parser::parse_x509_certificate(&der).map_err(|e| failed("read the CA", e))?;
    let at = OffsetDateTime::from_unix_timestamp(ca.validity().not_after.timestamp())
        .map_err(|e| failed("read the CA's expiry", e))?;
    Ok(at.date().to_string())
}

fn ca_der(paths: &Paths) -> Result<Vec<u8>, YardError> {
    let pem = std::fs::read(paths.ca_cert()).map_err(|e| io(&paths.ca_cert(), e))?;
    let (_, parsed) = parse_x509_pem(&pem).map_err(|e| failed("read the CA", e))?;
    Ok(parsed.contents)
}

struct Ca {
    pem: String,
    key: KeyPair,
    /// Unix seconds.
    not_after: i64,
}

/// The stored CA; `None` when its certificate or key is missing, does not parse, or the
/// key is not the certificate's. A file that exists but cannot be read is an error.
fn load_ca(paths: &Paths) -> Result<Option<Ca>, YardError> {
    let (Some(pem), Some(key_pem)) = (read_if_present(&paths.ca_cert())?, read_if_present(&paths.ca_key())?)
    else {
        return Ok(None);
    };
    let Ok((_, parsed)) = parse_x509_pem(pem.as_bytes()) else { return Ok(None) };
    let Ok(cert) = parsed.parse_x509() else { return Ok(None) };
    let Ok(key) = KeyPair::from_pem(&key_pem) else { return Ok(None) };
    if key.subject_public_key_info() != cert.public_key().raw {
        return Ok(None);
    }
    let not_after = cert.validity().not_after.timestamp();
    Ok(Some(Ca { pem, key, not_after }))
}

fn read_if_present(path: &Path) -> Result<Option<String>, YardError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        // Not UTF-8 is a corrupt PEM, which is replaced like any other.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Ok(Some(String::new())),
        Err(e) => Err(io(path, e)),
    }
}

fn bak(path: &Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    name.into()
}

fn name(common_name: &str) -> DistinguishedName {
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    dn.push(DnType::OrganizationName, "Yard");
    dn
}

fn private_dir(dir: &Path) -> Result<(), YardError> {
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| io(dir, e))
}

/// Written beside `path` with mode 0600 from creation, then renamed into place.
fn write_private(path: &Path, text: &str) -> Result<(), YardError> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".yard-tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| io(path, e))?;
    file.write_all(text.as_bytes()).map_err(|e| io(path, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io(path, e))
}

fn write_file(path: &Path, text: &str) -> Result<(), YardError> {
    yard_core::generated::write_atomic(path, text).map_err(|e| io(path, e))
}

fn io(path: &Path, e: std::io::Error) -> YardError {
    YardError::internal(format!("cannot use {}: {e}", path.display()))
}

fn failed(what: &str, e: impl std::fmt::Display) -> YardError {
    YardError::internal(format!("cannot {what}: {e}"))
}

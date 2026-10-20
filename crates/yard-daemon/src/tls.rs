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
use std::path::Path;
use std::process::Stdio;
use time::{Duration, OffsetDateTime};
use x509_parser::extensions::GeneralName;
use x509_parser::pem::parse_x509_pem;
use yard_core::Paths;
use yard_core::tls::{CA_DAYS, CertFacts};
use yard_proto::YardError;

/// Makes the CA when `ca/yard-ca.pem` or its key is missing or does not parse.
/// `Ok(true)` when it made one.
pub fn ensure_ca(paths: &Paths, user: &str, now: OffsetDateTime) -> Result<bool, YardError> {
    if read_ca(paths).is_ok() {
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
    write_private(&paths.ca_key(), &key.serialize_pem())?;
    write_file(&paths.ca_cert(), &cert.pem())?;
    Ok(true)
}

/// Issues `site`'s certificate for `host` and `*.host`, valid `days` from `now`, with a
/// fresh P-256 key, signed by the stored CA.
pub fn issue(paths: &Paths, site: &str, host: &str, now: OffsetDateTime, days: i64) -> Result<(), YardError> {
    let (ca_pem, ca_key) = read_ca(paths)?;
    let issuer = Issuer::from_ca_cert_pem(&ca_pem, ca_key).map_err(|e| failed("read the CA", e))?;
    let mut params = CertificateParams::new(vec![host.to_string(), format!("*.{host}")])
        .map_err(|e| failed(&format!("name a certificate for {host}"), e))?;
    params.distinguished_name = name(host);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.use_authority_key_identifier_extension = true;
    // A day back, so a clock slightly behind still accepts it.
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(days);
    let key = KeyPair::generate().map_err(|e| failed("generate a site key", e))?;
    let cert =
        params.signed_by(&key, &issuer).map_err(|e| failed(&format!("sign {host}'s certificate"), e))?;
    private_dir(&paths.certs_dir())?;
    write_private(&paths.cert_key(site), &key.serialize_pem())?;
    write_file(&paths.cert(site), &cert.pem())
}

/// What reconcile needs to know about `site`'s certificate; `None` when it or its key is
/// missing or does not parse, or the key is not the certificate's: nginx refuses all three.
pub fn facts(paths: &Paths, site: &str) -> Option<CertFacts> {
    let leaf_pem = std::fs::read(paths.cert(site)).ok()?;
    let ca_pem = std::fs::read(paths.ca_cert()).ok();
    let (_, leaf) = parse_x509_pem(&leaf_pem).ok()?;
    let leaf = leaf.parse_x509().ok()?;
    let key = KeyPair::from_pem(&std::fs::read_to_string(paths.cert_key(site)).ok()?).ok()?;
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
    let signed_by_ca = ca_pem.as_deref().and_then(|p| parse_x509_pem(p).ok()).is_some_and(|(_, ca)| {
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
    let status = tokio::process::Command::new(security)
        .args(["verify-cert", "-c"])
        .arg(ca)
        .args(["-l", "-L", "-q", "-p", "basic"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
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

/// The CA certificate's PEM and its key, both parsed.
fn read_ca(paths: &Paths) -> Result<(String, KeyPair), YardError> {
    let pem = std::fs::read_to_string(paths.ca_cert()).map_err(|e| io(&paths.ca_cert(), e))?;
    let (_, parsed) = parse_x509_pem(pem.as_bytes()).map_err(|e| failed("read the CA", e))?;
    parsed.parse_x509().map_err(|e| failed("read the CA", e))?;
    let key_pem = std::fs::read_to_string(paths.ca_key()).map_err(|e| io(&paths.ca_key(), e))?;
    let key = KeyPair::from_pem(&key_pem).map_err(|e| failed("read the CA key", e))?;
    Ok((pem, key))
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

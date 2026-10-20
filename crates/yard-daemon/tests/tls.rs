use std::os::unix::fs::PermissionsExt;
use time::{Duration, OffsetDateTime};
use x509_parser::pem::parse_x509_pem;
use yard_core::Paths;
use yard_core::tls::{CA_DAYS, LEAF_DAYS};
use yard_daemon::tls;

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn root() -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    (dir, paths)
}

#[test]
fn ensure_ca_makes_a_ca_once() {
    let (_dir, paths) = root();
    assert!(tls::ensure_ca(&paths, "me", now()).unwrap());
    let pem = std::fs::read(paths.ca_cert()).unwrap();
    assert!(!tls::ensure_ca(&paths, "me", now()).unwrap(), "the second call keeps it");
    assert_eq!(std::fs::read(paths.ca_cert()).unwrap(), pem);
    assert_eq!(mode(&paths.ca_key()), 0o600);
    assert_eq!(mode(&paths.ca_dir()), 0o700);

    let (_, parsed) = parse_x509_pem(&pem).unwrap();
    let cert = parsed.parse_x509().unwrap();
    assert!(cert.is_ca());
    let cn = cert.subject().iter_common_name().next().unwrap().as_str().unwrap().to_string();
    assert_eq!(cn, "Yard CA (me)");
    let years = (cert.validity().not_after.timestamp() - now().unix_timestamp()) / 86_400;
    assert!((3649..=3650).contains(&years), "{years} days");
    assert_eq!(tls::ca_sha256(&paths).unwrap().len(), 64);
    assert_eq!(tls::ca_expires(&paths).unwrap().len(), "2036-10-18".len());
}

#[test]
fn a_corrupt_ca_is_made_again() {
    let (_dir, paths) = root();
    tls::ensure_ca(&paths, "me", now()).unwrap();
    std::fs::write(paths.ca_cert(), "not a certificate").unwrap();
    assert!(tls::ensure_ca(&paths, "me", now()).unwrap());
}

#[test]
fn a_replaced_ca_is_kept_as_bak() {
    let (_dir, paths) = root();
    tls::ensure_ca(&paths, "me", now()).unwrap();
    let key = std::fs::read(paths.ca_key()).unwrap();
    std::fs::write(paths.ca_cert(), "not a certificate").unwrap();
    assert!(tls::ensure_ca(&paths, "me", now()).unwrap());
    let bak =
        |p: std::path::PathBuf| p.with_extension(format!("{}.bak", p.extension().unwrap().to_str().unwrap()));
    assert_eq!(std::fs::read(bak(paths.ca_cert())).unwrap(), b"not a certificate");
    assert_eq!(std::fs::read(bak(paths.ca_key())).unwrap(), key);
    assert_eq!(mode(&bak(paths.ca_key())), 0o600);
}

#[test]
fn an_unreadable_ca_is_an_error_not_a_new_ca() {
    let (_dir, paths) = root();
    tls::ensure_ca(&paths, "me", now()).unwrap();
    let pem = std::fs::read(paths.ca_cert()).unwrap();
    std::fs::set_permissions(paths.ca_cert(), std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = tls::ensure_ca(&paths, "me", now());
    std::fs::set_permissions(paths.ca_cert(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(result.is_err(), "{result:?}");
    assert_eq!(std::fs::read(paths.ca_cert()).unwrap(), pem, "the CA is left alone");
}

#[test]
fn a_ca_near_expiry_is_made_again() {
    let (_dir, paths) = root();
    let lasting = now() - Duration::days(CA_DAYS - LEAF_DAYS - 2);
    tls::ensure_ca(&paths, "me", lasting).unwrap();
    assert!(!tls::ensure_ca(&paths, "me", now()).unwrap(), "more than a leaf's life left");

    std::fs::remove_dir_all(paths.ca_dir()).unwrap();
    let ending = now() - Duration::days(CA_DAYS - LEAF_DAYS + 2);
    tls::ensure_ca(&paths, "me", ending).unwrap();
    assert!(tls::ensure_ca(&paths, "me", now()).unwrap(), "a leaf would outlive it");
}

#[test]
fn a_leaf_never_outlives_its_ca() {
    let (_dir, paths) = root();
    let made = now() - Duration::days(CA_DAYS - 100);
    tls::ensure_ca(&paths, "me", made).unwrap();
    tls::issue(&paths, "blog", "blog.yard", now(), LEAF_DAYS).unwrap();
    let ca_end = (made + Duration::days(CA_DAYS)).unix_timestamp();
    assert!(tls::facts(&paths, "blog").unwrap().not_after <= ca_end);
}

#[test]
fn issued_leaf_names_the_host_and_its_subdomains() {
    let (_dir, paths) = root();
    tls::ensure_ca(&paths, "me", now()).unwrap();
    tls::issue(&paths, "blog", "blog.yard", now(), 825).unwrap();
    let facts = tls::facts(&paths, "blog").expect("the leaf parses");
    assert_eq!(facts.dns_names, ["blog.yard", "*.blog.yard"]);
    assert!(facts.signed_by_ca);
    let days = (facts.not_after - now().unix_timestamp()) / 86_400;
    assert!((824..=825).contains(&days), "{days} days");
    assert_eq!(mode(&paths.cert_key("blog")), 0o600);
}

#[test]
fn a_leaf_from_another_ca_is_not_signed_by_ca() {
    let (_dir, paths) = root();
    tls::ensure_ca(&paths, "me", now()).unwrap();
    tls::issue(&paths, "blog", "blog.yard", now() - Duration::days(1), 825).unwrap();
    std::fs::remove_file(paths.ca_cert()).unwrap();
    tls::ensure_ca(&paths, "me", now()).unwrap();
    assert!(!tls::facts(&paths, "blog").unwrap().signed_by_ca);
}

#[test]
fn facts_of_a_missing_or_corrupt_cert_is_none() {
    let (_dir, paths) = root();
    tls::ensure_ca(&paths, "me", now()).unwrap();
    assert_eq!(tls::facts(&paths, "blog"), None);
    std::fs::create_dir_all(paths.certs_dir()).unwrap();
    std::fs::write(paths.cert("blog"), "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n")
        .unwrap();
    assert_eq!(tls::facts(&paths, "blog"), None);
}

#[test]
fn facts_of_a_cert_without_its_key_is_none() {
    let (_dir, paths) = root();
    tls::ensure_ca(&paths, "me", now()).unwrap();
    tls::issue(&paths, "blog", "blog.yard", now(), 30).unwrap();
    assert!(tls::facts(&paths, "blog").is_some());
    std::fs::write(paths.cert_key("blog"), "not a key").unwrap();
    assert!(tls::facts(&paths, "blog").is_none(), "an unreadable key");
    std::fs::remove_file(paths.cert_key("blog")).unwrap();
    assert!(tls::facts(&paths, "blog").is_none(), "a missing key");
}

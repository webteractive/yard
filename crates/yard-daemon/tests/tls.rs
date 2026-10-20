use std::os::unix::fs::PermissionsExt;
use time::{Duration, OffsetDateTime};
use x509_parser::pem::parse_x509_pem;
use yard_core::Paths;
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

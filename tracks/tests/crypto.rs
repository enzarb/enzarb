//! The binary's dependency graph enables two rustls providers; without an explicit choice the
//! first TLS client panics at runtime (seen on oldie). This builds one the way the binary does.

#[test]
fn a_tls_client_config_builds_after_installing_the_provider() {
    enzarb_tracks::install_crypto_provider();
    let roots = rustls::RootCertStore::empty();
    let _ = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    // Idempotent: a second call (another test, or a re-init) must not panic.
    enzarb_tracks::install_crypto_provider();
}

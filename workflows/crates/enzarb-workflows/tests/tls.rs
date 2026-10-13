//! Regression test for enzarb-workflows#18: with both `aws-lc-rs` (reqwest 0.13 via
//! `enzarb-connector-github`) and `ring` (kube) linked in, kube-client's `ClientConfig::builder()`
//! panics without a process default provider. Building a real `kube::Client` offline reaches that
//! exact call.

#[tokio::test]
async fn builds_a_kube_client_after_init_tls() {
    enzarb_workflows::init_tls();
    enzarb_workflows::init_tls(); // idempotent
    let config = kube::Config::new("https://127.0.0.1:1".parse().unwrap());
    kube::Client::try_from(config).expect("kube client builds with the ring provider installed");
}

#[test]
fn builds_a_rustls_client_after_init_tls() {
    enzarb_workflows::init_tls();
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    drop(config);
}

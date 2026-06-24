// Kubelet image credential provider for the in-cluster Zot registry.
//
// The kubelet execs this plugin when it needs to pull an image, sending a
// `CredentialProviderRequest` (credentialprovider.kubelet.k8s.io/v1) on stdin.
// On Kubernetes v1.33+ the request carries a pod-bound ServiceAccount token
// (audience `registry-pull.enzarb.dev`, configured via the CredentialProvider
// `tokenAttributes`). We hand that token back to the kubelet as the registry
// password; the kubelet then runs the standard Docker v2 bearer-token flow
// against authd, which mints a *pull-only* JWT scoped to the pod's project.
//
// This removes the need for any imagePullSecret: the credential is a live,
// short-lived SA token the kubelet fetches per pull, never stored on disk.
use std::io::{self, Read, Write};

const API_VERSION: &str = "credentialprovider.kubelet.k8s.io/v1";
const REGISTRY_HOST: &str = "registry.enzarb.dev";
// Short cache: SA tokens are short-lived and the kubelet scopes the cache by
// service account (tokenAttributes.cacheType), so same-project pods on a node
// may reuse it but it expires well within the token TTL.
const CACHE_DURATION: &str = "2m0s";
// Cache by registry so all pods pulling from registry.enzarb.dev on the same node
// share one cached credential per token (the kubelet further scopes by SA token
// value when tokenAttributes.cacheType=Token is set in the CredentialProviderConfig).
const CACHE_KEY_TYPE: &str = "Registry";

fn main() {
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        fail(&format!("read request: {e}"));
    }

    let req: serde_json::Value =
        serde_json::from_str(&input).unwrap_or_else(|e| fail(&format!("parse request: {e}")));

    let image = req
        .get("image")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !image_is_ours(image) {
        // Not our registry: return no credentials so the kubelet falls back to
        // its other credential sources (anonymous, node config, etc.).
        respond(serde_json::json!({}));
    }

    let token = req
        .get("serviceAccountToken")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            fail("no serviceAccountToken in request (requires k8s v1.33+ and tokenAttributes)")
        });

    respond(serde_json::json!({
        REGISTRY_HOST: {
            "username": "sa-token",
            "password": token,
        }
    }));
}

// image_is_ours reports whether the image reference targets our registry host.
// The image is a full reference like "registry.enzarb.dev/org/proj/app:tag".
fn image_is_ours(image: &str) -> bool {
    image
        .split('/')
        .next()
        .map(|host| host == REGISTRY_HOST)
        .unwrap_or(false)
}

// respond writes a CredentialProviderResponse with the given auth map and exits.
fn respond(auth: serde_json::Value) -> ! {
    let resp = serde_json::json!({
        "apiVersion": API_VERSION,
        "kind": "CredentialProviderResponse",
        "cacheKeyType": CACHE_KEY_TYPE,
        "cacheDuration": CACHE_DURATION,
        "auth": auth,
    });
    let mut out = io::stdout();
    if let Err(e) = writeln!(out, "{resp}") {
        fail(&format!("write response: {e}"));
    }
    std::process::exit(0);
}

fn fail(msg: &str) -> ! {
    eprintln!("kubelet-credential-enzarb: {msg}");
    std::process::exit(1);
}

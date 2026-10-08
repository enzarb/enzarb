use enzarb_crds::{Project, RepoKey, RunMode, project_crd_yaml};

const EXAMPLE: &str = r#"
apiVersion: enzarb.dev/v1alpha1
kind: Project
metadata: { name: enzarb }
spec:
  owner: ruckc
  displayName: enzarb
  repositories:
    - url: https://github.com/enzarb/enzarb-ui
      connector: github
      defaultBranch: main
  runMode: worktree
  triggers: { issues: true, changeRequests: true, mentions: true }
  sandbox:
    image: ""
    storage: 20Gi
    resources: {}
status:
  observedGeneration: 3
  namespace: enzarb-p-enzarb
  sandbox:
    name: sandbox
    endpoint: ws://sandbox.enzarb-p-enzarb.svc:7070/v1/mux
    authSecretRef: { name: sandbox-auth, key: token }
  conditions:
    - type: TenantReady
      status: "True"
      reason: Created
      message: ""
      lastTransitionTime: "2026-10-08T12:00:00Z"
"#;

fn example() -> Project {
    serde_yaml::from_str(EXAMPLE).unwrap()
}

#[test]
fn round_trip() {
    let p = example();
    assert_eq!(p.spec.owner, "ruckc");
    assert_eq!(p.spec.run_mode, RunMode::Worktree);
    assert!(p.spec.triggers.change_requests);
    let st = p.status.as_ref().unwrap();
    assert_eq!(st.observed_generation, Some(3));
    assert_eq!(st.sandbox.as_ref().unwrap().auth_secret_ref.key, "token");
    assert_eq!(st.conditions[0].type_, "TenantReady");
    let back: Project = serde_yaml::from_str(&serde_yaml::to_string(&p).unwrap()).unwrap();
    assert_eq!(back, p);
    let j: Project = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
    assert_eq!(j, p);
}

#[test]
fn defaults_apply() {
    let p: Project = serde_yaml::from_str(
        "apiVersion: enzarb.dev/v1alpha1\nkind: Project\nmetadata: {name: x}\nspec: {owner: o}",
    )
    .unwrap();
    assert_eq!(p.spec.run_mode, RunMode::Worktree);
    assert!(p.spec.triggers.issues && p.spec.triggers.mentions);
    assert_eq!(p.spec.sandbox.storage, "20Gi");
}

#[test]
fn runtime_class_and_docker_are_not_user_fields() {
    let yaml = "apiVersion: enzarb.dev/v1alpha1\nkind: Project\nmetadata: {name: x}\n\
                spec: {owner: o, sandbox: {runtimeClassName: runc}}";
    let p: Project = serde_yaml::from_str(yaml).unwrap();
    let back = serde_json::to_value(&p).unwrap();
    assert!(back["spec"]["sandbox"].get("runtimeClassName").is_none());
    assert!(back["spec"]["sandbox"].get("docker").is_none());
    assert!(!project_crd_yaml().contains("runtimeClassName"));
    assert!(!project_crd_yaml().contains("docker"));
}

#[test]
fn derived_names() {
    let p = example();
    assert_eq!(p.namespace_name(), "enzarb-p-enzarb");
    assert_eq!(p.tenant_name(), "enzarb-ruckc");
}

#[test]
fn repo_key_normalization() {
    let k = |u: &str| RepoKey::from_url(u).map(|k| k.to_string());
    assert_eq!(k("https://github.com/a/b").unwrap(), "github:a/b");
    assert_eq!(k("https://github.com/a/b.git").unwrap(), "github:a/b");
    assert_eq!(k("https://github.com/a/b/").unwrap(), "github:a/b");
    assert_eq!(
        k("https://GitHub.com/Enzarb/Enzarb-UI").unwrap(),
        "github:enzarb/enzarb-ui"
    );
    assert_eq!(
        k("https://github.com/a/b.github.io.git").unwrap(),
        "github:a/b.github.io"
    );
    assert!(k("http://github.com/a/b").is_err());
    assert!(k("https://example.com/a/b").is_err());
    assert!(k("https://github.com/a").is_err());
    assert!(k("https://github.com/a/b/c").is_err());
    assert!(k("git@github.com:a/b.git").is_err());
    assert_eq!(RepoKey::parse("github:A/B").unwrap().as_str(), "github:a/b");
    assert!(RepoKey::parse("a/b").is_err());
    assert!(RepoKey::parse("github:a").is_err());
}

#[test]
fn matches_repo() {
    let p = example();
    assert!(p.matches_repo("github:enzarb/enzarb-ui"));
    assert!(p.matches_repo("github:Enzarb/Enzarb-UI"));
    assert!(!p.matches_repo("github:enzarb/other"));
    assert!(!p.matches_repo("gitlab:enzarb/enzarb-ui"));
    assert!(!p.matches_repo("garbage"));
}

#[test]
fn crdgen_matches_committed() {
    let committed = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/crds/project.yaml"
    ))
    .expect("deploy/crds/project.yaml");
    assert_eq!(
        project_crd_yaml(),
        committed,
        "run: cargo run -q -p enzarb-crds --bin crdgen > deploy/crds/project.yaml"
    );
}

#[test]
fn crdgen_binary_output_matches() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_crdgen"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), project_crd_yaml());
}

use enzarb_crds::{
    CONDITION_TYPES, MAX_PROJECT_NAME_LEN, Project, REASON_INVALID_SPEC, RepoKey, RunMode,
    project_crd_yaml,
};

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
  namespace: enzarb-sandboxes
  sandbox:
    name: sb-enzarb
    endpoint: ws://sb-enzarb.enzarb-sandboxes.svc:7070/v1/mux
    authSecretRef: { name: sb-enzarb-auth, key: token }
  conditions:
    - type: ResourcesReady
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
    assert_eq!(st.conditions[0].type_, "ResourcesReady");
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
    assert_eq!(p.sandbox_name(), "sb-enzarb");
    assert_eq!(p.auth_secret_name(), "sb-enzarb-auth");
    assert_eq!(p.workspace_pvc_name(), "sb-enzarb-workspace");
    assert_eq!(p.docker_pvc_name(), "sb-enzarb-docker");
    assert_eq!(p.network_policy_name(), "sb-enzarb");
    // The longest name fits a 63-character label at the maximum Project name length.
    let long = "a".repeat(MAX_PROJECT_NAME_LEN);
    let p = Project::new(&long, example().spec);
    assert_eq!(p.workspace_pvc_name().len(), 63);
}

#[test]
fn condition_types_in_checklist_order() {
    assert_eq!(CONDITION_TYPES, ["ResourcesReady", "SandboxReady", "Ready"]);
    assert_eq!(REASON_INVALID_SPEC, "InvalidSpec");
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

#[test]
fn max_runs_is_an_optional_bounded_integer() {
    let crd: serde_json::Value = serde_yaml::from_str(&project_crd_yaml()).unwrap();
    let spec = &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"];
    let m = &spec["properties"]["maxRuns"];
    assert_eq!(m["type"], "integer");
    assert_eq!(m["minimum"].as_f64(), Some(1.0));
    assert_eq!(m["maximum"].as_f64(), Some(16.0));
    let required = spec["required"].as_array().unwrap();
    assert!(!required.iter().any(|r| r == "maxRuns"));
}

#[test]
fn max_runs_absent_deserializes_to_none_and_is_not_serialized() {
    let p = example();
    assert_eq!(p.spec.max_runs, None);
    let mut j = serde_json::to_value(&p.spec).unwrap();
    assert!(j.get("maxRuns").is_none());
    j["maxRuns"] = 4.into();
    let s: enzarb_crds::ProjectSpec = serde_json::from_value(j).unwrap();
    assert_eq!(s.max_runs, Some(4));
}

#[test]
fn crd_enforces_name_limit_with_create_only_cel() {
    let crd: serde_json::Value = serde_yaml::from_str(&project_crd_yaml()).unwrap();
    let rules =
        &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["x-kubernetes-validations"];
    let rules = rules.as_array().unwrap();
    assert_eq!(rules.len(), 2);
    // Create-only: with optionalOldSelf, oldSelf has no value on create, so the name is checked
    // only then; updates (status writes, spec edits) of older Projects always pass.
    assert_eq!(
        rules[0]["rule"],
        "oldSelf.hasValue() || self.metadata.name.size() <= 50"
    );
    assert_eq!(
        rules[1]["rule"],
        "oldSelf.hasValue() || self.metadata.name.matches('^[a-z0-9]([-a-z0-9]*[a-z0-9])?$')"
    );
    for r in rules {
        assert_eq!(r["optionalOldSelf"], true);
        assert!(
            r["message"]
                .as_str()
                .unwrap()
                .starts_with("Project name must be")
        );
    }
}

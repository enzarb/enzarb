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

mod precedence {
    use enzarb_crds::{Project, ProjectSpec, Repository, cmp_precedence, conflict_winner};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
    use std::cmp::Ordering;

    fn proj(name: &str, secs: Option<i64>, repos: &[(&str, &str)]) -> Project {
        let spec: ProjectSpec = serde_json::from_value(serde_json::json!({"owner": "o"})).unwrap();
        let mut p = Project::new(name, spec);
        p.spec.repositories = repos
            .iter()
            .map(|(c, u)| Repository {
                url: (*u).into(),
                connector: (*c).into(),
                default_branch: "main".into(),
            })
            .collect();
        p.metadata.creation_timestamp =
            secs.map(|s| Time(k8s_openapi::jiff::Timestamp::from_second(s).unwrap()));
        p
    }

    const GH: &str = "https://github.com/enzarb/x";

    #[test]
    fn ordering() {
        let old = proj("zzz", Some(1), &[]);
        let new = proj("aaa", Some(2), &[]);
        assert_eq!(cmp_precedence(&old, &new), Ordering::Less);
        let a = proj("a", Some(5), &[]);
        let b = proj("b", Some(5), &[]);
        assert_eq!(cmp_precedence(&a, &b), Ordering::Less);
        assert_eq!(cmp_precedence(&b, &a), Ordering::Greater);
        assert_eq!(cmp_precedence(&a, &a.clone()), Ordering::Equal);
        let none = proj("a", None, &[]);
        assert_eq!(cmp_precedence(&new, &none), Ordering::Less);
        assert_eq!(cmp_precedence(&none, &new), Ordering::Greater);
    }

    #[test]
    fn oldest_wins_and_newer_loses() {
        let w = proj("w", Some(1), &[("github", GH)]);
        let l = proj("l", Some(2), &[("github", GH)]);
        let all = [w.clone(), l.clone()];
        assert!(conflict_winner(&w, all.iter()).is_none());
        let (key, by) = conflict_winner(&l, all.iter()).unwrap();
        assert_eq!(key.to_string(), "github:enzarb/x");
        assert_eq!(by.metadata.name.as_deref(), Some("w"));
    }

    #[test]
    fn keys_are_normalized() {
        let w = proj("w", Some(1), &[("github", GH)]);
        for url in [
            "https://github.com/Enzarb/X",
            "https://github.com/enzarb/x.git",
            "https://github.com/enzarb/x/",
        ] {
            let l = proj("l", Some(2), &[("github", url)]);
            assert!(conflict_winner(&l, [&w].into_iter()).is_some(), "{url}");
        }
        let other = proj("l", Some(2), &[("gitlab", GH)]);
        assert!(conflict_winner(&other, [&w].into_iter()).is_none());
    }

    #[test]
    fn same_repo_twice_does_not_conflict_with_itself() {
        let dup = format!("{GH}.git");
        let p = proj("p", Some(1), &[("github", GH), ("github", &dup)]);
        assert!(conflict_winner(&p, [&p].into_iter()).is_none());
    }

    #[test]
    fn multi_repo_loses_on_any_shared_repo() {
        let w = proj("w", Some(1), &[("github", "https://github.com/enzarb/b")]);
        let l = proj(
            "l",
            Some(2),
            &[
                ("github", "https://github.com/enzarb/a"),
                ("github", "https://github.com/enzarb/b"),
            ],
        );
        let (key, _) = conflict_winner(&l, [&w].into_iter()).unwrap();
        assert_eq!(key.to_string(), "github:enzarb/b");
        // The older Project is not affected by the newer one.
        assert!(conflict_winner(&w, [&l].into_iter()).is_none());
    }

    #[test]
    fn terminating_project_releases_repo_and_malformed_urls_are_skipped() {
        let mut w = proj("w", Some(1), &[("github", GH)]);
        let l = proj("l", Some(2), &[("github", GH)]);
        w.metadata.deletion_timestamp =
            Some(Time(k8s_openapi::jiff::Timestamp::from_second(9).unwrap()));
        assert!(conflict_winner(&l, [&w].into_iter()).is_none());
        let bad = proj("a", Some(1), &[("github", "not a url")]);
        let bad2 = proj("b", Some(2), &[("github", "not a url")]);
        assert!(conflict_winner(&bad2, [&bad].into_iter()).is_none());
    }
}

#[test]
fn cpu_memory_and_pending_change_are_optional_and_bounded() {
    let yaml = enzarb_crds::project_crd_yaml();
    assert!(yaml.contains("maximum: 64.0") && yaml.contains("pattern: ^[1-9][0-9]*(Mi|Gi)$"));
    assert!(yaml.contains("pendingChange:"));
    let s = enzarb_crds::SandboxSpec::default();
    let v = serde_json::to_value(&s).unwrap();
    assert!(v.get("cpu").is_none() && v.get("memory").is_none());
    let st = enzarb_crds::ProjectStatus::default();
    assert!(
        serde_json::to_value(&st)
            .unwrap()
            .get("pendingChange")
            .is_none()
    );
}

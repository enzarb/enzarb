use super::*;
use ez_core::Config;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

const MODES: &str = "default_mode = \"explore\"\n[modes.explore]\ndescription = \"look\"\n[modes.test]\ndescription = \"test\"\n";

struct Env {
    user: tempfile::TempDir,
    proj: tempfile::TempDir,
}

impl Env {
    fn new(with_modes: bool) -> Self {
        let e = Self {
            user: tempfile::tempdir().unwrap(),
            proj: tempfile::tempdir().unwrap(),
        };
        if with_modes {
            std::fs::create_dir_all(e.proj.path().join(".ez")).unwrap();
            std::fs::write(e.proj.path().join(".ez/modes.toml"), MODES).unwrap();
        }
        e
    }

    fn wizard(&self) -> SetupWizard {
        SetupWizard::new(SetupContext::load(self.proj.path(), Some(self.user.path())))
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(w: &mut SetupWizard, codes: &[KeyCode]) -> Effect {
    let mut last = Effect::None;
    for c in codes {
        last = w.handle_key(key(*c));
    }
    last
}

fn typ(w: &mut SetupWizard, s: &str) {
    for c in s.chars() {
        w.handle_key(key(KeyCode::Char(c)));
    }
}

fn probe_ok(kind: &str, ids: &[&str]) -> Result<setup_api::ProbeResult, String> {
    Ok(setup_api::ProbeResult {
        kind: Some(kind.into()),
        models: ids
            .iter()
            .map(|i| setup_api::ModelView {
                id: i.to_string(),
                display_name: None,
            })
            .collect(),
        attempts: vec![],
        message: format!("{kind} ✓ ({} models)", ids.len()),
    })
}

fn probe_fail(msg: &str) -> Result<setup_api::ProbeResult, String> {
    Ok(setup_api::ProbeResult {
        kind: None,
        models: vec![],
        attempts: vec![],
        message: msg.into(),
    })
}

fn draw(w: &SetupWizard) -> String {
    let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
    t.draw(|f| render::render(w, f)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            s.push_str(buf[(x, y)].symbol());
        }
        s.push('\n');
    }
    s
}

/// Paste an OpenRouter key and walk to the Check step; returns the probe request.
fn to_check(w: &mut SetupWizard) -> ProbeRequest {
    assert_eq!(w.step, Step::ApiKey);
    typ(w, "sk-or-v1-secret-1234");
    press(w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::Endpoint);
    // The URL is never pre-filled: the user types it.
    assert_eq!(w.endpoint.value, "");
    typ(w, "https://openrouter.ai/api/v1");
    match press(w, &[KeyCode::Enter]) {
        Effect::Probe(r) => r,
        e => panic!("expected probe, got {e:?}"),
    }
}

/// ... and on to the Models step with the given model ids.
fn to_models(w: &mut SetupWizard, ids: &[&str]) {
    let req = to_check(w);
    w.probed(req.generation, probe_ok("openai", ids));
    assert_eq!(press(w, &[KeyCode::Enter]), Effect::None);
    assert_eq!(w.step, Step::Models);
}

#[test]
fn full_walk_key_first_then_url_check_models_name_modes_apply() {
    let env = Env::new(true);
    let mut w = env.wizard();
    assert_eq!(w.step, Step::ApiKey);
    // A key is required for "paste".
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::ApiKey);
    assert!(w.error.is_some());
    let req = to_check(&mut w);
    assert_eq!(req.api_key.as_deref(), Some("sk-or-v1-secret-1234"));
    assert_eq!(req.base_url, "https://openrouter.ai/api/v1");
    assert_eq!(req.force, None);
    assert!(!format!("{req:?}").contains("secret"), "{req:?}");
    assert!(draw(&w).contains("probing"));
    w.probed(
        req.generation,
        probe_ok("openai", &["a/one", "b/two", "c/three"]),
    );
    assert!(draw(&w).contains("openai ✓"));
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::Models);
    assert_eq!(w.profile_name.value, "openrouter");
    // Nothing selected is refused; select first and third.
    press(&mut w, &[KeyCode::Enter]);
    assert!(w.error.is_some());
    press(
        &mut w,
        &[
            KeyCode::Char(' '),
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Char(' '),
        ],
    );
    press(&mut w, &[KeyCode::Enter]);
    assert!(matches!(w.models, ModelsPhase::Default { .. }));
    press(&mut w, &[KeyCode::Down, KeyCode::Enter]);
    assert_eq!(w.default_model, "c/three");
    assert_eq!(w.step, Step::ProfileName);
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::Modes);
    press(&mut w, &[KeyCode::Enter, KeyCode::Down, KeyCode::Enter]);
    assert_eq!(
        w.assignments[0],
        Some(("openrouter".to_string(), "a/one".to_string()))
    );
    press(&mut w, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    assert_eq!(w.step, Step::Review);
    assert!(!w.preview.contains("sk-or"), "{}", w.preview);
    let plan = w.plan();
    assert_eq!(plan.profile.models, ["a/one", "c/three"]);
    assert_eq!(plan.profile.provider, ProviderKind::Openai);
    assert_eq!(plan.profile.api_key_name.as_deref(), Some("openrouter"));
    assert_eq!(plan.profile.api_key_env, None);
    assert_eq!(plan.key_scope, KeyScope::User);
    assert_eq!(press(&mut w, &[KeyCode::Enter]), Effect::Apply);
    let r = setup::apply(&w.plan(), Some(env.user.path()), env.proj.path()).unwrap();
    w.applied(Ok(r));
    assert!(w.done() && !w.aborted);
    let cfg = Config::load(env.proj.path(), Some(env.user.path())).unwrap();
    assert_eq!(cfg.default_profile, "openrouter");
    assert_eq!(
        ez_core::config::named_key(Some(env.user.path()), "openrouter").as_deref(),
        Some("sk-or-v1-secret-1234")
    );
    let cfg_text = std::fs::read_to_string(env.user.path().join("config.toml")).unwrap();
    assert!(!cfg_text.contains("secret") && !cfg_text.contains("API_KEY"));
    let modes = std::fs::read_to_string(env.proj.path().join(".ez/modes.toml")).unwrap();
    assert!(modes.contains("model = \"a/one\""), "{modes}");
}

#[test]
fn the_endpoint_is_never_prefilled_and_is_required() {
    let env = Env::new(false);
    for key in ["sk-ant-api03-x", "sk-proj-abc", "sk-or-v1-x"] {
        let mut w = env.wizard();
        typ(&mut w, key);
        press(&mut w, &[KeyCode::Enter]);
        assert_eq!(w.endpoint.value, "", "{key}");
    }
    // Unknown shape: empty, required.
    let mut w = env.wizard();
    typ(&mut w, "abc123");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.endpoint.value, "");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::Endpoint);
    assert!(w.error.as_deref().unwrap().contains("required"));
    // Back and forth keeps an edited URL.
    w.endpoint.set("http://gpu-box:8000/v1/");
    w.endpoint_touched = true;
    press(&mut w, &[KeyCode::Esc]);
    typ(&mut w, "x");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.endpoint.value, "http://gpu-box:8000/v1/");
}

#[test]
fn endpoint_is_validated_trimmed_and_has_no_suggestions() {
    let env = Env::new(false);
    let mut w = env.wizard();
    press(&mut w, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]); // no key
    assert_eq!(w.auth_choice(), Some(AuthChoice::NoKey));
    assert_eq!(w.step, Step::Endpoint);
    assert_eq!(w.endpoint.value, "");
    for bad in ["localhost:8000", "ftp://x", "http://", "http://a b"] {
        w.endpoint.set(bad);
        assert_eq!(press(&mut w, &[KeyCode::Enter]), Effect::None);
        assert_eq!(w.step, Step::Endpoint, "{bad}");
        assert!(w.error.is_some(), "{bad}");
    }
    let out = draw(&w);
    assert!(
        !out.contains("OpenRouter") && !out.contains("LM Studio"),
        "{out}"
    );
    w.endpoint.set("  http://localhost:11434/v1/  ");
    let Effect::Probe(r) = press(&mut w, &[KeyCode::Enter]) else {
        panic!()
    };
    assert_eq!(r.base_url, "http://localhost:11434/v1");
    assert_eq!(r.api_key, None);
    w.probed(r.generation, probe_ok("openai", &["llama3"]));
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.profile_name.value, "ollama");
    press(&mut w, &[KeyCode::Char(' '), KeyCode::Enter]); // single model: straight to the name
    assert_eq!(w.step, Step::ProfileName);
    let p = w.plan();
    assert_eq!(p.profile.api_key_name, None);
    assert!(p.new_key.is_none());
    assert_eq!(
        p.profile.base_url.as_deref(),
        Some("http://localhost:11434/v1")
    );
}

#[test]
fn anthropic_flavor_default_url_is_not_stored() {
    let env = Env::new(false);
    let mut w = env.wizard();
    typ(&mut w, "sk-ant-api03-zzz");
    press(&mut w, &[KeyCode::Enter]);
    typ(&mut w, "https://api.anthropic.com");
    let Effect::Probe(r) = press(&mut w, &[KeyCode::Enter]) else {
        panic!()
    };
    w.probed(r.generation, probe_ok("anthropic", &["claude-a"]));
    press(
        &mut w,
        &[KeyCode::Enter, KeyCode::Char(' '), KeyCode::Enter],
    );
    assert_eq!(w.step, Step::ProfileName);
    assert_eq!(w.profile_name.value, "anthropic");
    let p = w.plan();
    assert_eq!(p.profile.provider, ProviderKind::Anthropic);
    assert_eq!(p.profile.base_url, None);
    assert_eq!(p.new_key.as_ref().unwrap().name, "anthropic");
}

#[test]
fn probe_failure_retry_override_and_manual_models() {
    let env = Env::new(false);
    let mut w = env.wizard();
    let req = to_check(&mut w);
    // Stale results are ignored.
    w.probed(req.generation + 7, probe_ok("openai", &["x"]));
    assert_eq!(w.check, CheckPhase::Checking);
    // Esc while checking returns to the URL; the late result can no longer land.
    press(&mut w, &[KeyCode::Esc]);
    assert_eq!(w.step, Step::Endpoint);
    w.probed(req.generation, probe_ok("openai", &["x"]));
    assert_eq!(w.step, Step::Endpoint);
    let Effect::Probe(req) = press(&mut w, &[KeyCode::Enter]) else {
        panic!()
    };
    w.probed(
        req.generation,
        probe_fail("The server rejected the API key (HTTP 401). Check the key."),
    );
    assert!(draw(&w).contains("HTTP 401"));
    // Retry re-probes with a new generation.
    let Effect::Probe(again) = press(&mut w, &[KeyCode::Char('r')]) else {
        panic!()
    };
    assert!(again.generation > req.generation);
    w.probed(
        again.generation,
        probe_fail("Neither API answered (HTTP 404). Try adding /v1."),
    );
    // Override forces the other flavor.
    let Effect::Probe(forced) = press(&mut w, &[KeyCode::Char('o')]) else {
        panic!()
    };
    assert_eq!(forced.force, Some(ProviderKind::Anthropic));
    w.probed(forced.generation, probe_ok("anthropic", &["claude-a"]));
    assert_eq!(w.kind(), ProviderKind::Anthropic);
    assert!(draw(&w).contains("chosen manually"));
    let Effect::Probe(back) = press(&mut w, &[KeyCode::Char('o')]) else {
        panic!()
    };
    assert_eq!(back.force, Some(ProviderKind::Openai));
    w.probed(back.generation, probe_fail("boom"));
    // Continue by typing model ids.
    press(&mut w, &[KeyCode::Char('m')]);
    assert_eq!(w.step, Step::Models);
    assert!(matches!(w.models, ModelsPhase::Manual { .. }));
    press(&mut w, &[KeyCode::Enter]);
    assert!(draw(&w).contains("at least one"));
    typ(&mut w, " one , two,,one");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.chosen, ["one", "two"]);
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.default_model, "one");
    assert_eq!(w.plan().profile.models, ["one", "two"]);
    assert_eq!(w.plan().profile.provider, ProviderKind::Openai);
}

#[test]
fn esc_goes_back_and_first_step_aborts() {
    let env = Env::new(true);
    let mut w = env.wizard();
    to_models(&mut w, &["m1"]);
    press(&mut w, &[KeyCode::Esc]);
    assert_eq!(w.step, Step::Endpoint);
    press(&mut w, &[KeyCode::Esc]);
    assert_eq!(w.step, Step::ApiKey);
    assert!(!w.aborted);
    press(&mut w, &[KeyCode::Esc]);
    assert!(w.aborted && w.done());
}

#[test]
fn going_back_to_models_keeps_the_list_and_selection() {
    let env = Env::new(true);
    let mut w = env.wizard();
    to_models(&mut w, &["m1", "m2"]);
    press(&mut w, &[KeyCode::Char(' '), KeyCode::Enter]); // m1 only -> name
    assert_eq!(w.step, Step::ProfileName);
    press(&mut w, &[KeyCode::Esc]);
    assert_eq!(w.step, Step::Models);
    assert!(matches!(w.models, ModelsPhase::List(_)));
    assert!(w.selected.contains("m1"));
}

#[test]
fn multi_select_all_and_filter() {
    let env = Env::new(false);
    let mut w = env.wizard();
    to_models(
        &mut w,
        &["anthropic/claude-a", "openai/gpt", "openai/gpt-mini"],
    );
    press(&mut w, &[KeyCode::Char('a')]);
    assert_eq!(w.selected.len(), 3);
    press(&mut w, &[KeyCode::Char('a')]);
    assert!(w.selected.is_empty());
    typ(&mut w, "gpt");
    let ModelsPhase::List(st) = &w.models else {
        panic!()
    };
    assert_eq!(st.visible().len(), 2);
    press(&mut w, &[KeyCode::Char(' ')]);
    assert_eq!(w.selected.iter().collect::<Vec<_>>(), ["openai/gpt"]);
    press(&mut w, &[KeyCode::Esc]);
    let ModelsPhase::List(st) = &w.models else {
        panic!()
    };
    assert!(st.filter.is_empty());
    press(&mut w, &[KeyCode::Char('/')]);
    typ(&mut w, "anth ");
    let ModelsPhase::List(st) = &w.models else {
        panic!()
    };
    assert!(st.filter_focus && st.filter == "anth ");
    press(&mut w, &[KeyCode::Backspace, KeyCode::Enter]);
    w.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert!(w.selected.contains("anthropic/claude-a"));
    assert!(draw(&w).contains("anthropic/claude-a"));
}

#[test]
fn missing_modes_offers_init_or_skip() {
    let env = Env::new(false);
    let mut w = env.wizard();
    to_models(&mut w, &["only"]);
    press(
        &mut w,
        &[KeyCode::Char(' '), KeyCode::Enter, KeyCode::Enter],
    );
    assert_eq!(w.step, Step::Modes);
    assert!(draw(&w).contains("ez init"));
    assert_eq!(press(&mut w, &[KeyCode::Enter]), Effect::RunInit);
    assert!(draw(&w).contains("generating"));
    w.modes_initialized(Err("no templates".into()));
    assert!(draw(&w).contains("no templates"));
    assert_eq!(press(&mut w, &[KeyCode::Enter]), Effect::RunInit);
    w.modes_initialized(ModeSet::from_toml_str(MODES).map_err(|e| e.to_string()));
    assert!(matches!(w.modes_phase, ModesPhase::Assign { .. }));
    assert_eq!(w.assignments.len(), 2);

    let mut w = env.wizard();
    to_models(&mut w, &["only"]);
    press(
        &mut w,
        &[
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Enter,
        ],
    );
    assert_eq!(w.step, Step::Review);
    assert!(w.plan().mode_models.is_empty());
}

#[test]
fn existing_assignments_are_prefilled_and_clearable() {
    let env = Env::new(true);
    std::fs::write(
        env.proj.path().join(".ez/modes.toml"),
        "[modes.explore]\nprofile = \"other\"\nmodel = \"fast\"\n[modes.test]\n",
    )
    .unwrap();
    std::fs::write(
        env.user.path().join("config.toml"),
        "[profiles.other]\nprovider = \"openai\"\nmodel = \"fast\"\nmodels = [\"fast\", \"slow\"]\n",
    )
    .unwrap();
    let mut w = env.wizard();
    to_models(&mut w, &["m"]);
    press(
        &mut w,
        &[KeyCode::Char(' '), KeyCode::Enter, KeyCode::Enter],
    );
    assert_eq!(w.assignments[0], Some(("other".into(), "fast".into())));
    let opts = w.model_options();
    assert!(opts.contains(&("other".into(), "slow".into())));
    assert!(opts.contains(&("openrouter".into(), "m".into())));
    press(
        &mut w,
        &[KeyCode::Enter, KeyCode::Up, KeyCode::Up, KeyCode::Up],
    );
    typ(&mut w, "slow");
    assert!(draw(&w).contains("other/slow"));
    press(&mut w, &[KeyCode::Down, KeyCode::Enter]);
    assert_eq!(w.assignments[0], Some(("other".into(), "slow".into())));
}

#[test]
fn profile_overwrite_prompt_and_name_validation() {
    let env = Env::new(false);
    std::fs::write(
        env.user.path().join("config.toml"),
        "[profiles.openrouter]\nprovider = \"openai\"\nmodel = \"old\"\n",
    )
    .unwrap();
    let mut w = env.wizard();
    to_models(&mut w, &["m"]);
    press(&mut w, &[KeyCode::Char(' '), KeyCode::Enter]);
    assert_eq!(w.step, Step::ProfileName);
    assert_eq!(press(&mut w, &[KeyCode::Enter]), Effect::None);
    assert!(w.confirm_overwrite);
    assert!(draw(&w).contains("already exists"));
    press(&mut w, &[KeyCode::Char('n')]);
    typ(&mut w, "-2");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::Modes);
    assert_eq!(w.plan().profile_name, "openrouter-2");
    // Invalid names are refused.
    let mut w = env.wizard();
    to_models(&mut w, &["m"]);
    press(&mut w, &[KeyCode::Char(' '), KeyCode::Enter]);
    w.profile_name.set("bad name");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::ProfileName);
    assert!(w.error.is_some());
}

#[test]
fn existing_saved_key_is_used_for_the_probe_and_the_plan() {
    let env = Env::new(false);
    ez_core::keys::add_key(Some(env.user.path()), "acme", "sk-acme-0000", Some("Acme")).unwrap();
    ez_core::keys::add_key(Some(env.user.path()), "openrouter", "sk-or-old", None).unwrap();
    let mut w = env.wizard();
    press(&mut w, &[KeyCode::Down]);
    assert_eq!(w.auth_choice(), Some(AuthChoice::Existing));
    let out = draw(&w);
    assert!(
        out.contains("acme") && !out.contains("sk-acme-0000"),
        "{out}"
    );
    press(&mut w, &[KeyCode::Enter]);
    w.endpoint.set("https://openrouter.ai/api/v1");
    let Effect::Probe(r) = press(&mut w, &[KeyCode::Enter]) else {
        panic!()
    };
    assert_eq!(r.api_key.as_deref(), Some("sk-acme-0000"));
    w.probed(r.generation, probe_ok("openai", &["m"]));
    press(
        &mut w,
        &[KeyCode::Enter, KeyCode::Char(' '), KeyCode::Enter],
    );
    let p = w.plan();
    assert_eq!(p.profile.api_key_name.as_deref(), Some("acme"));
    assert!(p.new_key.is_none());
    // A pasted key never overwrites an existing key of the same name.
    let mut w = env.wizard();
    to_models(&mut w, &["m"]);
    press(&mut w, &[KeyCode::Char(' '), KeyCode::Enter]);
    assert_eq!(w.plan().new_key.unwrap().name, "openrouter-2");
    // No saved keys: the option explains itself.
    let env2 = Env::new(false);
    let mut w = env2.wizard();
    press(&mut w, &[KeyCode::Down, KeyCode::Enter]);
    assert_eq!(w.step, Step::ApiKey);
    assert!(w.error.is_some());
}

#[test]
fn the_secret_is_masked_and_never_in_the_preview() {
    let env = Env::new(false);
    let mut w = env.wizard();
    typ(&mut w, "sk-ant-TOPSECRET");
    let out = draw(&w);
    assert!(!out.contains("TOPSECRET") && out.contains("••••"), "{out}");
    assert!(!out.contains("ANTHROPIC_API_KEY") && !out.contains("environment variable"));
}

#[test]
fn ctrl_c_aborts_without_writing() {
    let env = Env::new(true);
    let mut w = env.wizard();
    to_models(&mut w, &["m"]);
    w.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(w.aborted && w.finished.is_none());
    assert!(!env.user.path().join("config.toml").exists());
    assert!(!env.user.path().join("credentials.toml").exists());
}

#[test]
fn every_step_renders() {
    let env = Env::new(true);
    let mut w = env.wizard();
    let out = draw(&w);
    assert!(
        out.contains("step 1 of 7") && out.contains("API key"),
        "{out}"
    );
    typ(&mut w, "sk-ant-x");
    press(&mut w, &[KeyCode::Enter]);
    let out = draw(&w);
    assert!(
        out.contains("step 2 of 7") && !out.contains("Suggestions"),
        "{out}"
    );
    typ(&mut w, "https://api.anthropic.com");
    let Effect::Probe(r) = press(&mut w, &[KeyCode::Enter]) else {
        panic!()
    };
    assert!(draw(&w).contains("step 3 of 7"));
    w.probed(
        r.generation,
        probe_ok("anthropic", &["claude-a", "claude-b"]),
    );
    press(&mut w, &[KeyCode::Enter]);
    let out = draw(&w);
    assert!(
        out.contains("step 4 of 7") && out.contains("claude-b"),
        "{out}"
    );
    press(&mut w, &[KeyCode::Char('a'), KeyCode::Enter]);
    assert!(draw(&w).contains("default model"));
    press(&mut w, &[KeyCode::Enter]);
    assert!(draw(&w).contains("step 5 of 7"));
    press(&mut w, &[KeyCode::Enter]);
    let out = draw(&w);
    assert!(
        out.contains("step 6 of 7") && out.contains("explore"),
        "{out}"
    );
    press(&mut w, &[KeyCode::Enter]);
    assert!(draw(&w).contains("model for `explore`"));
    press(&mut w, &[KeyCode::Esc, KeyCode::Char('c')]);
    let out = draw(&w);
    assert!(
        out.contains("step 7 of 7") && out.contains("config.toml"),
        "{out}"
    );
    let mut t = Terminal::new(TestBackend::new(20, 6)).unwrap();
    t.draw(|f| render::render(&w, f)).unwrap();
}

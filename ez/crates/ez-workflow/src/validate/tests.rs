use super::*;

const WORKFLOWS: &str = include_str!("../../tests/fixtures/workflows.toml");
const MODES: &str = include_str!("../../tests/fixtures/modes.toml");

fn modes() -> ModeCatalog {
    ModeCatalog::from_modes_toml(MODES).unwrap()
}

/// Validates `text` with the fixture modes; returns `(severity, path, message)` triples.
fn findings(text: &str) -> Vec<(Severity, String, String)> {
    let (_, r) = check(&Engine::new(), text, Some(&modes()));
    r.diagnostics
        .into_iter()
        .map(|d| (d.severity, d.path, d.message))
        .collect()
}

fn errors(text: &str) -> Vec<(String, String)> {
    findings(text)
        .into_iter()
        .filter(|f| f.0 == Severity::Error)
        .map(|f| (f.1, f.2))
        .collect()
}

fn has_error(text: &str, path: &str, needle: &str) {
    let errs = errors(text);
    assert!(
        errs.iter().any(|(p, m)| p == path && m.contains(needle)),
        "no error at `{path}` containing `{needle}` in {errs:#?}"
    );
}

/// A one-workflow file with `nodes` appended (`start = "a"`).
fn wf(nodes: &str) -> String {
    format!(
        "version = 1\n[workflows.w]\non = [\"issues.opened\"]\nstart = \"a\"\n{nodes}\n[workflows.w.nodes.end]\ntype = \"end\"\nstatus = \"done\"\n"
    )
}

#[test]
fn the_adr_example_is_valid() {
    let f = findings(WORKFLOWS);
    assert!(f.is_empty(), "{f:#?}");
}

#[test]
fn parse_errors_are_reported_on_the_file() {
    let errs = errors("version = 1\n[workflows.w]\non = [\"issues\"]\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].0, "");
    assert!(errs[0].1.contains("start"), "{}", errs[0].1);

    let errs = errors("version = 1\nbogus = 2\n");
    assert!(errs[0].1.contains("bogus"), "{errs:?}");

    let errs = errors(&wf(
        "[workflows.w.nodes.a]\ntype = \"teleport\"\nnext = \"end\"",
    ));
    assert!(errs[0].1.contains("teleport"), "{errs:?}");

    let errs = errors(&wf(
        "[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"end\"\nsurprise = 1",
    ));
    assert!(errs[0].1.contains("surprise"), "{errs:?}");

    let big = "#".repeat(schema::MAX_FILE_BYTES + 1);
    assert!(errors(&big)[0].1.contains("limit"));
}

#[test]
fn version_ids_and_limits() {
    has_error("version = 2\n", "version", "unsupported");
    let text = wf("[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"end\"")
        .replace("[workflows.w]", "[workflows.\"bad id\"]")
        .replace("workflows.w.", "workflows.\"bad id\".");
    has_error(&text, "workflows.bad id", "workflow id");
    has_error(
        &wf(
            "[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"end\"\nmax_visits = 0",
        ),
        "workflows.w.nodes.a.max_visits",
        "at least 1",
    );
    let t = wf("[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"end\"")
        .replace(
            "start = \"a\"",
            "start = \"a\"\nmax_runs = 0\ntimeout = \"forever\"",
        );
    has_error(&t, "workflows.w.max_runs", "1-50");
    has_error(&t, "workflows.w.timeout", "invalid duration");
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"end\"\ntimeout = \"400d\"",
    );
    has_error(&t, "workflows.w.nodes.a.timeout", "longer than");
}

#[test]
fn triggers_and_event_patterns() {
    let t = wf("[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"Issues\", \"nightly_build\"]\nnext = \"end\"")
        .replace("on = [\"issues.opened\"]", "on = []\nif = \"run.id\"");
    has_error(&t, "workflows.w.on", "at least one");
    has_error(&t, "workflows.w.nodes.a.on[0]", "not `event`");
    has_error(&t, "workflows.w.if", "unknown variable `run`");
    let f = findings(&t);
    assert!(f.iter().any(|(s, p, m)| *s == Severity::Warning
        && p == "workflows.w.nodes.a.on[1]"
        && m.contains("not a known")));
}

#[test]
fn start_edges_and_reachability() {
    let t = wf("[workflows.w.nodes.b]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"nowhere\"");
    has_error(&t, "workflows.w.start", "no node `a`");
    has_error(&t, "workflows.w.nodes.b.next[0].to", "no node `nowhere`");

    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"end\"\n[workflows.w.nodes.b]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"end\"",
    );
    assert!(errors(&t).is_empty());
    assert!(findings(&t).iter().any(|(s, p, m)| *s == Severity::Warning
        && p == "workflows.w.nodes.b"
        && m.contains("not reachable")));

    // An edge without `if` before the last; the last with `if`.
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = [{ to = \"end\" }, { if = \"timed_out\", to = \"end\" }]",
    );
    has_error(&t, "workflows.w.nodes.a.next[0]", "only the last edge");
    has_error(&t, "workflows.w.nodes.a.next[1]", "must not have `if`");

    // A cycle with no way out.
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"b\"\n[workflows.w.nodes.b]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"a\"",
    );
    has_error(&t, "workflows.w.nodes.a", "no `end` node can be reached");
    has_error(&t, "workflows.w.nodes.b", "no `end` node can be reached");

    let t = "version = 1\n[workflows.w]\non = [\"issues\"]\nstart = \"a\"\n[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = \"a\"\n";
    has_error(t, "workflows.w.nodes", "at least one `end`");
}

#[test]
fn edge_conditions_see_the_nodes_outputs() {
    // A wait's `timed_out` and an agent's `result` are bare inside their own `next`, but not
    // another type's outputs.
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = [{ if = \"timed_out or result\", to = \"end\" }, { to = \"end\" }]",
    );
    has_error(
        &t,
        "workflows.w.nodes.a.next[0].if",
        "unknown variable `result`",
    );
    assert_eq!(errors(&t).len(), 1, "{:#?}", errors(&t));
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = [{ if = \"nodes.ghost.x\", to = \"end\" }, { to = \"end\" }]",
    );
    has_error(&t, "workflows.w.nodes.a.next[0].if", "no node `ghost`");
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"wait\"\non = [\"issues\"]\nnext = [{ if = \"timed_out ==\", to = \"end\" }, { to = \"end\" }]",
    );
    has_error(&t, "workflows.w.nodes.a.next[0].if", "syntax error");
}

#[test]
fn agent_nodes() {
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"agent\"\nmode = \"ghost\"\ninstruction = \"{{ event.issue.title }} {{ secrets.x }}\"\nresult = { \"9x\" = \"string\" }\nnext = \"end\"",
    );
    has_error(&t, "workflows.w.nodes.a.mode", "no mode `ghost`");
    has_error(
        &t,
        "workflows.w.nodes.a.instruction",
        "unknown variable `secrets`",
    );
    has_error(&t, "workflows.w.nodes.a.result.9x", "letters, digits");
    has_error(&t, "workflows.w.nodes.a.result.9x", "JSON Schema table");

    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"agent\"\nmode = \"triage\"\ninstruction = \" \"\nresult = {}\nnext = \"end\"",
    );
    has_error(&t, "workflows.w.nodes.a.instruction", "must not be empty");
    has_error(&t, "workflows.w.nodes.a.result", "at least one property");

    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"agent\"\nmode = \"triage\"\ninstruction = \"{{ x\"\nnext = \"end\"",
    );
    has_error(&t, "workflows.w.nodes.a.instruction", "syntax error");

    // No modes file at all.
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"agent\"\nmode = \"triage\"\ninstruction = \"go\"\nnext = \"end\"",
    );
    let (_, r) = check(&Engine::new(), &t, None);
    assert!(
        r.errors()
            .any(|d| d.path == "workflows.w.nodes.a.mode"
                && d.message.contains("no `.ez/modes.toml`"))
    );

    // AskUserQuestion is only a warning; a mode without `tools` (all built-ins) is fine.
    let m = ModeCatalog::from_modes_toml(
        "[modes.ask]\ntools = [\"Read\", \"AskUserQuestion\"]\n[modes.all]\n",
    )
    .unwrap();
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"agent\"\nmode = \"ask\"\ninstruction = \"go\"\nnext = \"b\"\n[workflows.w.nodes.b]\ntype = \"agent\"\nmode = \"all\"\ninstruction = \"go\"\nnext = \"end\"",
    );
    let (_, r) = check(&Engine::new(), &t, Some(&m));
    assert!(r.is_ok(), "{r:#?}");
    let w: Vec<_> = r.warnings().collect();
    assert_eq!(w.len(), 1, "{w:#?}");
    assert!(w[0].message.contains("AskUserQuestion"));
}

#[test]
fn command_nodes() {
    let t = wf("[workflows.w.nodes.a]\ntype = \"command\"\nnext = \"end\"");
    has_error(&t, "workflows.w.nodes.a", "needs `task`");
    let t =
        wf("[workflows.w.nodes.a]\ntype = \"command\"\ntask = \"x\"\nrun = \"y\"\nnext = \"end\"");
    has_error(&t, "workflows.w.nodes.a", "not both");
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"command\"\nrun = \"echo {{ who }}\"\nargs = [\"x\"]\nnext = \"end\"",
    );
    has_error(&t, "workflows.w.nodes.a.args", "go with `task`");
    has_error(&t, "workflows.w.nodes.a.run", "unknown variable `who`");
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"command\"\ntask = \"test\"\nargs = [\"--issue={{ subject.number }}\", \"{{ bad }}\"]\nnext = [{ if = \"exit_code == 0 and json.ok\", to = \"end\" }, { to = \"end\" }]",
    );
    let errs = errors(&t);
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].0, "workflows.w.nodes.a.args[1]");
}

#[test]
fn action_nodes() {
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"action\"\naction = \"comment\"\ntitle = \"t\"\nnext = \"end\"",
    );
    has_error(&t, "workflows.w.nodes.a.body", "needs `body`");
    has_error(&t, "workflows.w.nodes.a.title", "doesn't take `title`");
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"action\"\naction = \"add_labels\"\nbody = \"b\"\nnext = \"end\"",
    );
    has_error(&t, "workflows.w.nodes.a.labels", "needs `labels`");
    has_error(&t, "workflows.w.nodes.a.body", "doesn't take `body`");
    let t = wf(
        "[workflows.w.nodes.a]\ntype = \"action\"\naction = \"open_pull_request\"\nnext = \"end\"",
    );
    has_error(&t, "workflows.w.nodes.a.title", "needs `title`");
    let f = findings(&t);
    assert!(
        f.iter()
            .any(|(s, _, m)| *s == Severity::Warning && m.contains("push = true")),
        "{f:#?}"
    );
    let t = wf("[workflows.w.nodes.a]\ntype = \"action\"\naction = \"explode\"\nnext = \"end\"");
    assert!(errors(&t)[0].1.contains("explode"));
}

#[test]
fn mode_catalog_reads_ids_and_tools() {
    let m = modes();
    assert_eq!(m.modes.keys().collect::<Vec<_>>(), ["triage", "implement"]);
    assert_eq!(
        m.modes["triage"].as_deref().unwrap(),
        ["Read", "Glob", "Grep"]
    );
    assert!(m.contains("implement") && !m.contains("x"));
    assert!(
        ModeCatalog::from_modes_toml("default_mode = \"x\"\n")
            .unwrap_err()
            .contains("no modes")
    );
    assert!(
        ModeCatalog::from_modes_toml("[modes]\nx = 1\n")
            .unwrap_err()
            .contains("table")
    );
    assert!(
        ModeCatalog::from_modes_toml("[modes.x]\ntools = \"Read\"\n")
            .unwrap_err()
            .contains("list")
    );
    assert!(ModeCatalog::from_modes_toml("not toml =").is_err());
}

#[test]
fn files_round_trip_through_toml() {
    let file = schema::parse(WORKFLOWS).unwrap();
    let text = schema::to_toml(&file).unwrap();
    let again = schema::parse(&text).unwrap();
    assert_eq!(file, again, "{text}");
    assert!(validate(&Engine::new(), &again, Some(&modes())).is_ok());
}

#[test]
fn result_needs_submit_result_in_mcp_tools() {
    let modes = ModeCatalog::from_modes_toml(
        "[modes.none]\ndescription = \"x\"\nmcp_tools = []\n\
         [modes.other]\ndescription = \"x\"\nmcp_tools = [\"mcp__github__*\"]\n\
         [modes.all]\ndescription = \"x\"\nmcp_tools = [\"mcp__*\"]\n\
         [modes.exact]\ndescription = \"x\"\nmcp_tools = [\"mcp__enzarb__submit_result\"]\n\
         [modes.dflt]\ndescription = \"x\"\n",
    )
    .unwrap();
    let node = |mode: &str, result: bool| {
        wf(&format!(
            "[workflows.w.nodes.a]\ntype = \"agent\"\nmode = \"{mode}\"\ninstruction = \"go\"\n{}next = \"end\"",
            if result {
                "result = { ok = { type = \"boolean\" } }\n"
            } else {
                ""
            }
        ))
    };
    let errs = |text: &str| {
        let (_, r) = check(&Engine::new(), text, Some(&modes));
        r.errors()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
            .join("\n")
    };
    for bad in ["none", "other", "dflt"] {
        let e = errs(&node(bad, true));
        assert!(e.contains("submit_result"), "{bad}: {e}");
        assert_eq!(errs(&node(bad, false)), "", "no result, no tool needed");
    }
    for good in ["all", "exact"] {
        assert_eq!(errs(&node(good, true)), "", "{good}");
    }
}

#[test]
fn vars_parse_and_are_checked_where_referenced() {
    let ok = "version = 1\n[workflows.w]\non = [\"issues.opened\"]\nstart = \"a\"\n[workflows.w.vars]\nflag = true\nname = \"x\"\n[workflows.w.nodes.a]\ntype = \"end\"\nstatus = \"done\"\nreason = \"{{ vars.name }}\"\n";
    assert!(errors(ok).is_empty(), "{:?}", errors(ok));
    let bad = ok.replace("{{ vars.name }}", "{{ vars.nope }}");
    has_error(&bad, "workflows.w.nodes.a.reason", "no var `nope`");
    let nested = ok.replace("name = \"x\"", "name = [1]");
    has_error(
        &nested,
        "workflows.w.vars.name",
        "string, number or boolean",
    );
}

//! Every TOML example in `docs/modes.md` must load with ez's own parsers.

use ez_core::{Config, ModeSet};

#[test]
fn modes_doc_examples_parse() {
    let doc = include_str!("../../../docs/modes.md");
    let mut checked = 0;
    for (i, block) in doc.split("```toml\n").skip(1).enumerate() {
        let block = block.split("```").next().unwrap();
        let mut table: toml::Table = block
            .parse()
            .unwrap_or_else(|e| panic!("example {i} is not TOML: {e}\n{block}"));
        let mut modes = toml::Table::new();
        for key in ["modes", "default_mode"] {
            if let Some(v) = table.remove(key) {
                modes.insert(key.into(), v);
            }
        }
        if modes.contains_key("modes") {
            // Fragments that set default_mode to a mode they don't show are complete files.
            ModeSet::from_toml_str(&modes.to_string())
                .unwrap_or_else(|e| panic!("example {i}: modes don't load: {e:#}\n{block}"));
        }
        if !table.is_empty() {
            Config::from_toml_str(&table.to_string())
                .unwrap_or_else(|e| panic!("example {i}: config doesn't load: {e:#}\n{block}"));
        }
        checked += 1;
    }
    assert!(checked >= 9, "only {checked} examples found");
}

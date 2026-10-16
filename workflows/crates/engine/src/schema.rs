//! JSON Schema validation of step results and custom event payloads.

use serde_json::Value;

pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    let v = jsonschema::validator_for(schema).map_err(|e| format!("invalid schema: {e}"))?;
    let errs: Vec<String> = v
        .iter_errors(value)
        .take(5)
        .map(|e| format!("{}: {e}", e.instance_path))
        .collect();
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

#!/usr/bin/env python3
"""Validate the rendered pgop resources against the vendored pgop CRD schemas.

kubeconform has no schema for pgop, and a typo'd field would be silently pruned by the API
server, so also reject fields the CRD does not declare. Usage: check-pgop.py [scenario ...]
"""
import pathlib, subprocess, sys
import jsonschema, yaml

root = pathlib.Path(__file__).resolve().parent.parent
crds = {d["spec"]["names"]["kind"]: d
        for d in yaml.safe_load_all((root / "tests/crds/pgop-0.9.1.yaml").read_text()) if d}


def strip(x):
    if isinstance(x, dict):
        return {k: strip(v) for k, v in x.items() if not k.startswith("x-kubernetes")}
    if isinstance(x, list):
        return [strip(i) for i in x]
    return x


def unknown(schema, value, path=""):
    if schema.get("type") == "object" and "properties" in schema and isinstance(value, dict):
        for k, v in value.items():
            if k not in schema["properties"]:
                yield f"{path}.{k}"
            else:
                yield from unknown(schema["properties"][k], v, f"{path}.{k}")
    elif schema.get("type") == "array" and isinstance(value, list):
        for i in value:
            yield from unknown(schema["items"], i, path + "[]")


bad = 0
for scenario in sys.argv[1:] or ["default", "dev", "oldie"]:
    out = subprocess.run([str(root / "scripts/render.sh"), scenario], check=True,
                         capture_output=True, text=True).stdout
    n = 0
    for d in yaml.safe_load_all(out):
        if not d or not d["apiVersion"].startswith("pgop.ruck.io/"):
            continue
        schema = crds[d["kind"]]["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]
        try:
            jsonschema.validate(d["spec"], strip(schema))
        except jsonschema.ValidationError as e:
            print(f"{scenario}: {d['kind']}/{d['metadata']['name']}: {e.message}"); bad += 1
        for u in unknown(schema, d["spec"]):
            print(f"{scenario}: {d['kind']}/{d['metadata']['name']}: unknown field {u}"); bad += 1
        n += 1
    print(f"{scenario}: {n} pgop resources checked")
sys.exit(1 if bad else 0)

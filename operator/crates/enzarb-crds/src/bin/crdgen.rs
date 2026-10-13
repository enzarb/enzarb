//! Prints the Project CRD as YAML:
//! `cargo run -q -p enzarb-crds --bin crdgen > deploy/crds/project.yaml`

fn main() {
    print!("{}", enzarb_crds::project_crd_yaml());
}

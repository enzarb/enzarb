//! ez workflows (`.ez/workflows.toml`, [ADR-0029](../../../../docs/adr/0029-ez-workflows.md)).
//!
//! The one implementation of workflow semantics: the file format ([`schema`]), validation
//! against the modes of the same commit ([`validate`]), MiniJinja templates and conditions
//! ([`expr`]), events, subjects and triggers ([`event`]), a run's state and how it advances
//! ([`run`]), and dry runs over logged events ([`dry_run`]). tracks, the dry run and the UI's
//! validation all go through this crate; nothing re-implements it.
//!
//! Pure: no I/O and no async.

pub mod dry_run;
pub mod event;
pub mod expr;
pub mod run;
pub mod schema;
pub mod validate;

pub use dry_run::{DryRun, DryRunOptions, Trace, TraceEnd, TraceStep, dry_run};
pub use event::{Event, Subject, SubjectKind, TriggerEval, evaluate_triggers};
pub use expr::Engine;
pub use run::{Rendered, RunError, RunInfo, RunState, RunStatus, Transition, wait_outputs};
pub use schema::{WorkflowsFile, parse, to_toml};
pub use validate::{Diagnostic, ModeCatalog, Report, Severity, check, validate};

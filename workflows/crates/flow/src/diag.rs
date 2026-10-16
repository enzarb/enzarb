//! Diagnostics: every error carries the file and a 1-based line/column.

use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One problem found while loading, resolving or linting a definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Display name of the file (`path` for the local repo, `owner/repo@ref:path` for libraries).
    pub file: String,
    /// 1-based line; 0 when unknown.
    pub line: usize,
    /// 1-based column; 0 when unknown.
    pub col: usize,
    pub message: String,
}

impl Diagnostic {
    pub fn error(
        file: impl Into<String>,
        (line, col): (usize, usize),
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Error,
            file: file.into(),
            line,
            col,
            message: message.into(),
        }
    }

    pub fn warning(
        file: impl Into<String>,
        (line, col): (usize, usize),
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            file: file.into(),
            line,
            col,
            message: message.into(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// GitHub Actions workflow-command annotation (`::error file=…,line=…,col=…::msg`).
    pub fn github(&self) -> String {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let msg = self
            .message
            .replace('%', "%25")
            .replace('\r', "%0D")
            .replace('\n', "%0A");
        format!(
            "::{level} file={},line={},col={}::{msg}",
            self.file,
            self.line.max(1),
            self.col.max(1)
        )
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(
            f,
            "{}:{}:{}: {level}: {}",
            self.file, self.line, self.col, self.message
        )
    }
}

/// A list of diagnostics returned as an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics(pub Vec<Diagnostic>);

impl Diagnostics {
    pub fn has_errors(&self) -> bool {
        self.0.iter().any(Diagnostic::is_error)
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for d in &self.0 {
            writeln!(f, "{d}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostics {}

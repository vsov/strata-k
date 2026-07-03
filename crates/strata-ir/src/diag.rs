//! Shared diagnostic type + unified code registry. [IR-9, D9]
//!
//! Lives in the base crate so `strata-front` (E0xxx) and `strata-check` (E1xxx)
//! emit through one `Diagnostic` and one `DiagCode` namespace without depending
//! on each other (resolves the D14 front<->check sibling contradiction).
//! `strata-cli` renders; it never mints codes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A stable diagnostic code, e.g. `E0042`. The registry guarantees global
/// uniqueness across crates (front owns E0xxx, check owns E1xxx). Rendered
/// zero-padded with an `E` prefix; stored as the numeric part.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct DiagCode(pub u16);

impl std::fmt::Display for DiagCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "E{:04}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// Byte-offset span into a source (or IR document). [carried from IR-4 node provenance]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    /// The default/unknown span. Excluded from serialized IR and from equality
    /// wherever a node carries a span (spans are provenance, not identity).
    pub fn is_zero(&self) -> bool {
        self.start == 0 && self.end == 0
    }
}

/// A machine-applicable fix: replace `span` with `replacement`. rustc-style. [D9]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FixPatch {
    pub span: Span,
    pub replacement: String,
}

/// The single diagnostic struct every crate emits (IR-9). Rendered as human text
/// or `--error-format=json` by the CLI (D9); crates never mint their own format.
///
/// TODO(IR-9): secondary labels + the `nearest_allowed` field used by the
/// table-2.4 checker (CHECK-7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Diagnostic {
    pub code: DiagCode,
    pub severity: Severity,
    pub message: String,
    pub primary: Span,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<FixPatch>,
}

/// A shared collector every front-end/checker crate emits through (IR-9). Each
/// crate owns a distinct code range (front E0xxx, check E1xxx); the struct and
/// renderers are common so strata-cli consumes one type.
#[derive(Debug, Default, Clone)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn error(&mut self, code: DiagCode, message: impl Into<String>, span: Span) {
        self.items.push(Diagnostic {
            code,
            severity: Severity::Error,
            message: message.into(),
            primary: span,
            fixes: Vec::new(),
        });
    }

    pub fn error_fix(
        &mut self,
        code: DiagCode,
        message: impl Into<String>,
        span: Span,
        fix: FixPatch,
    ) {
        self.items.push(Diagnostic {
            code,
            severity: Severity::Error,
            message: message.into(),
            primary: span,
            fixes: vec![fix],
        });
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn items(&self) -> &[Diagnostic] {
        &self.items
    }

    pub fn into_items(self) -> Vec<Diagnostic> {
        self.items
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.items.extend(other.items);
    }

    /// Human-readable rendering with line/column and a caret underline.
    pub fn render_text(&self, src: &str) -> String {
        let mut out = String::new();
        for d in &self.items {
            let (line, col, line_text) = locate(src, d.primary.start);
            let sev = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Note => "note",
            };
            out.push_str(&format!("{sev}[{}]: {}\n", d.code, d.message));
            out.push_str(&format!("  --> {}:{}\n", line + 1, col + 1));
            out.push_str(&format!("   | {line_text}\n"));
            let width = (d.primary.end.saturating_sub(d.primary.start)).max(1) as usize;
            out.push_str(&format!("   | {}{}\n", " ".repeat(col), "^".repeat(width)));
            for fix in &d.fixes {
                out.push_str(&format!("   = help: replace with `{}`\n", fix.replacement));
            }
        }
        out
    }

    /// JSON rendering for `--error-format=json` (D9): a JSON array of diagnostics.
    pub fn render_json(&self) -> String {
        serde_json::to_string_pretty(&self.items).unwrap_or_else(|_| "[]".to_string())
    }
}

/// Map a byte offset to (line, column, that line's text). Zero-based line/col.
fn locate(src: &str, offset: u32) -> (usize, usize, &str) {
    let offset = (offset as usize).min(src.len());
    let mut line_start = 0;
    let mut line_no = 0;
    for (i, b) in src.bytes().enumerate() {
        if i >= offset {
            break;
        }
        if b == b'\n' {
            line_no += 1;
            line_start = i + 1;
        }
    }
    let line_end = src[line_start..]
        .find('\n')
        .map(|p| line_start + p)
        .unwrap_or(src.len());
    (line_no, offset - line_start, &src[line_start..line_end])
}

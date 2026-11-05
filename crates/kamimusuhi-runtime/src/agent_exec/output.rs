//! Reading a run's stdout, line by line, into progress events and a final
//! result. Each structured format lives with its harness; this module only
//! dispatches.

use super::config::OutputFormat;
use super::types::{TaskEvent, TaskUsage};

/// Everything a harness reported about one run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedRun {
    /// Answer fragments in order.
    pub texts: Vec<String>,
    pub error: Option<String>,
    pub session_id: Option<String>,
    pub usage: TaskUsage,
    pub cost_usd: Option<f64>,
    pub tools: Vec<String>,
    /// The harness's own verdict, when it gives one.
    pub reported_success: Option<bool>,
}

pub struct OutputParser {
    format: OutputFormat,
    run: ParsedRun,
}

impl OutputParser {
    pub fn new(format: OutputFormat) -> Self {
        Self {
            format,
            run: ParsedRun::default(),
        }
    }

    pub fn feed(&mut self, line: &str) -> Vec<TaskEvent> {
        match self.format {
            OutputFormat::Text => Vec::new(),
            OutputFormat::OpencodeJson => super::opencode::feed(&mut self.run, line),
            OutputFormat::CommandCodeNdjson => super::commandcode::feed(&mut self.run, line),
        }
    }

    /// `stdout` is the whole captured output, used by the text format.
    pub fn finish(mut self, stdout: &str) -> ParsedRun {
        if self.format == OutputFormat::Text {
            let text = stdout.trim();
            if !text.is_empty() {
                self.run.texts.push(text.to_owned());
            }
        }
        self.run
    }
}

//! An indenting text writer shared by the three language generators.
//!
//! [`CodeWriter`] knows nothing about any language. It tracks an indentation
//! level, never writes trailing whitespace, collapses consecutive blank lines
//! and always ends its output with exactly one newline, so golden files are
//! stable byte for byte.

/// The column past which [`CodeWriter::call`] wraps its arguments.
pub const MAX_WIDTH: usize = 100;

/// An indenting line writer.
///
/// ```
/// use keel_bindgen::emit::CodeWriter;
///
/// let mut w = CodeWriter::new("    ");
/// w.block("struct Point", |w| {
///     w.line("var x: Int");
///     w.blank();
///     w.line("var y: Int");
/// });
/// assert_eq!(w.finish(), "struct Point {\n    var x: Int\n\n    var y: Int\n}\n");
/// ```
#[derive(Debug, Clone)]
pub struct CodeWriter {
    buf: String,
    level: usize,
    unit: &'static str,
    last_blank: bool,
}

impl CodeWriter {
    /// A writer that indents nested blocks with `unit` (for example four
    /// spaces).
    #[must_use]
    pub fn new(unit: &'static str) -> CodeWriter {
        CodeWriter {
            buf: String::new(),
            level: 0,
            unit,
            // Nothing is written yet, so a leading blank line is suppressed.
            last_blank: true,
        }
    }

    /// Writes one line at the current indentation. An empty `text` writes a
    /// blank line (see [`CodeWriter::blank`]); text that contains newlines is
    /// split and every part is indented.
    pub fn line(&mut self, text: impl AsRef<str>) {
        let text = text.as_ref();
        if text.is_empty() {
            self.blank();
            return;
        }
        for part in text.split('\n') {
            if part.trim().is_empty() {
                self.blank();
                continue;
            }
            for _ in 0..self.level {
                self.buf.push_str(self.unit);
            }
            self.buf.push_str(part.trim_end());
            self.buf.push('\n');
            self.last_blank = false;
        }
    }

    /// Writes a blank line unless the previous line was already blank or
    /// nothing has been written yet.
    pub fn blank(&mut self) {
        if !self.last_blank {
            self.buf.push('\n');
            self.last_blank = true;
        }
    }

    /// Writes `open {`, runs `body` one level deeper, then writes `}`.
    ///
    /// A block whose body writes nothing collapses to `open {}`.
    pub fn block(&mut self, open: impl AsRef<str>, body: impl FnOnce(&mut CodeWriter)) {
        self.line(format!("{} {{", open.as_ref()));
        let start = self.buf.len();
        self.indented(body);
        if self.buf.len() == start {
            // Nothing was written: turn `open {` into `open {}`.
            self.buf.pop();
            self.buf.push_str("}\n");
            self.last_blank = false;
        } else {
            self.line("}");
        }
    }

    /// Like [`CodeWriter::block`] with an explicit opening line (which already
    /// contains its opening delimiter) and closing line.
    pub fn block_with(
        &mut self,
        open: impl AsRef<str>,
        close: impl AsRef<str>,
        body: impl FnOnce(&mut CodeWriter),
    ) {
        self.line(open);
        self.indented(body);
        self.line(close);
    }

    /// Writes `prefix(arg, arg, ..)suffix` on one line when it fits in
    /// [`MAX_WIDTH`] columns, otherwise with one argument per line.
    /// `trailing_comma` adds a comma after the last argument in the wrapped
    /// form (Swift argument lists cannot have one).
    pub fn call(
        &mut self,
        prefix: impl AsRef<str>,
        args: &[String],
        suffix: impl AsRef<str>,
        trailing_comma: bool,
    ) {
        let (prefix, suffix) = (prefix.as_ref(), suffix.as_ref());
        let single = format!("{prefix}({}){suffix}", args.join(", "));
        let width = self.level * self.unit.len() + single.chars().count();
        if args.is_empty() || (width <= MAX_WIDTH && !single.contains('\n')) {
            self.line(single);
            return;
        }
        self.line(format!("{prefix}("));
        self.indented(|w| {
            for (i, arg) in args.iter().enumerate() {
                let comma = if i + 1 < args.len() || trailing_comma {
                    ","
                } else {
                    ""
                };
                w.line(format!("{arg}{comma}"));
            }
        });
        self.line(format!("){suffix}"));
    }

    /// A declaration header followed by a block: `prefix(args)suffix {`, the
    /// arguments wrapped one per line when the header does not fit in
    /// [`MAX_WIDTH`] columns, then `body` indented and a closing `}`.
    pub fn call_block(
        &mut self,
        prefix: impl AsRef<str>,
        args: &[String],
        suffix: impl AsRef<str>,
        trailing_comma: bool,
        body: impl FnOnce(&mut CodeWriter),
    ) {
        let (prefix, suffix) = (prefix.as_ref(), suffix.as_ref());
        let single = format!("{prefix}({}){suffix} {{", args.join(", "));
        let width = self.level * self.unit.len() + single.chars().count();
        if args.is_empty() || width <= MAX_WIDTH {
            self.line(single);
        } else {
            self.line(format!("{prefix}("));
            self.indented(|w| {
                for (i, arg) in args.iter().enumerate() {
                    let comma = if i + 1 < args.len() || trailing_comma {
                        ","
                    } else {
                        ""
                    };
                    w.line(format!("{arg}{comma}"));
                }
            });
            self.line(format!("){suffix} {{"));
        }
        self.indented(body);
        self.line("}");
    }

    /// Runs `body` one indentation level deeper.
    pub fn indented(&mut self, body: impl FnOnce(&mut CodeWriter)) {
        self.level += 1;
        // A block never starts with a blank line.
        self.last_blank = true;
        body(self);
        self.level -= 1;
        // ... and never ends with one: drop a trailing blank line.
        if self.last_blank && self.buf.ends_with("\n\n") {
            self.buf.pop();
        }
        self.last_blank = false;
    }

    /// Consumes the writer and returns the text, ending in exactly one
    /// newline (or empty if nothing was written).
    #[must_use]
    pub fn finish(mut self) -> String {
        while self.buf.ends_with("\n\n") {
            self.buf.pop();
        }
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_lines_collapse_and_never_lead_or_trail_a_block() {
        let mut w = CodeWriter::new("  ");
        w.blank();
        w.line("a");
        w.blank();
        w.blank();
        w.block("b", |w| {
            w.blank();
            w.line("c");
            w.blank();
        });
        w.blank();
        assert_eq!(w.finish(), "a\n\nb {\n  c\n}\n");
    }

    #[test]
    fn multi_line_text_is_indented_and_trimmed() {
        let mut w = CodeWriter::new("\t");
        w.block("x", |w| w.line("one  \ntwo\n\nthree"));
        assert_eq!(w.finish(), "x {\n\tone\n\ttwo\n\n\tthree\n}\n");
    }

    #[test]
    fn empty_output_is_empty() {
        assert_eq!(CodeWriter::new("    ").finish(), "");
        let mut w = CodeWriter::new("    ");
        w.blank();
        assert_eq!(w.finish(), "");
    }

    #[test]
    fn block_with_uses_the_given_delimiters() {
        let mut w = CodeWriter::new("  ");
        w.block_with("call(", ")", |w| w.line("arg,"));
        assert_eq!(w.finish(), "call(\n  arg,\n)\n");
    }
}

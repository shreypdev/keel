//! What the CLI prints, and how.
//!
//! Results go to stdout (so `undra dev` can be scripted), progress and warnings go to stderr.
//! Colour is used only when the stream is a terminal and `NO_COLOR` is not set.

use std::io::{IsTerminal, Write};

/// Terminal output.
#[derive(Clone, Copy, Debug)]
pub struct Ui {
    color_out: bool,
    color_err: bool,
}

impl Ui {
    /// Output that follows the terminal and `NO_COLOR`.
    #[must_use]
    pub fn detect() -> Ui {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        Ui {
            color_out: !no_color && std::io::stdout().is_terminal(),
            color_err: !no_color && std::io::stderr().is_terminal(),
        }
    }

    /// Output without colour, for tests.
    #[cfg(test)]
    #[must_use]
    pub fn plain() -> Ui {
        Ui {
            color_out: false,
            color_err: false,
        }
    }

    fn paint(enabled: bool, code: &str, text: &str) -> String {
        if enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    /// A line of the command's result, on stdout.
    pub fn line(&self, text: &str) {
        let _ = writeln!(std::io::stdout().lock(), "{text}");
    }

    /// A step of progress (`Building the core for the host`), on stderr.
    pub fn step(&self, text: &str) {
        let _ = writeln!(
            std::io::stderr().lock(),
            "{} {text}",
            Ui::paint(self.color_err, "1;32", "==>")
        );
    }

    /// A detail under a step, on stderr.
    pub fn detail(&self, text: &str) {
        let _ = writeln!(std::io::stderr().lock(), "    {text}");
    }

    /// A warning, on stderr.
    pub fn warn(&self, text: &str) {
        let _ = writeln!(
            std::io::stderr().lock(),
            "{} {text}",
            Ui::paint(self.color_err, "1;33", "warning:")
        );
    }

    /// A piece of advice, on stderr: not a result, and never a failure.
    pub fn hint(&self, text: &str) {
        let _ = writeln!(
            std::io::stderr().lock(),
            "{} {text}",
            Ui::paint(self.color_err, "1;36", "hint:")
        );
    }

    /// Text styled as a heading on stdout.
    #[must_use]
    pub fn bold_out(&self, text: &str) -> String {
        Ui::paint(self.color_out, "1", text)
    }

    /// Green text for stdout.
    #[must_use]
    pub fn green_out(&self, text: &str) -> String {
        Ui::paint(self.color_out, "32", text)
    }

    /// Yellow text for stdout.
    #[must_use]
    pub fn yellow_out(&self, text: &str) -> String {
        Ui::paint(self.color_out, "33", text)
    }

    /// Red text for stdout.
    #[must_use]
    pub fn red_out(&self, text: &str) -> String {
        Ui::paint(self.color_out, "31", text)
    }

    /// Dimmed text for stdout.
    #[must_use]
    pub fn dim_out(&self, text: &str) -> String {
        Ui::paint(self.color_out, "2", text)
    }

    /// Renders an error for stderr, with a coloured `error` prefix.
    #[must_use]
    pub fn error_text(&self, error: &crate::error::CliError) -> String {
        let text = error.to_string();
        match text.strip_prefix("error") {
            Some(rest) => format!("{}{rest}", Ui::paint(self.color_err, "1;31", "error")),
            None => text,
        }
    }
}

/// Renders a table: left-aligned columns separated by two spaces.
#[must_use]
pub fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (c, cell) in row.iter().enumerate() {
            if c + 1 == row.len() {
                line.push_str(cell);
            } else {
                line.push_str(cell);
                line.push_str(&" ".repeat(widths[c] - cell.chars().count() + 2));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_align_columns() {
        let rows = vec![
            vec!["artifact".to_owned(), "size".to_owned()],
            vec!["libundra_core.so".to_owned(), "742.4 KB".to_owned()],
        ];
        assert_eq!(
            table(&rows),
            "artifact         size\nlibundra_core.so  742.4 KB\n"
        );
    }

    #[test]
    fn plain_output_has_no_escape_codes() {
        let ui = Ui::plain();
        assert_eq!(ui.bold_out("x"), "x");
        assert_eq!(ui.red_out("x"), "x");
        let e = crate::error::CliError::bad_argument("w", "y", "f");
        assert!(ui.error_text(&e).starts_with("error[undra::C0009]"));
    }

    #[test]
    fn colour_wraps_when_enabled() {
        assert_eq!(Ui::paint(true, "1", "x"), "\x1b[1mx\x1b[0m");
    }
}

//! Template rendering: `@@NAME@@` placeholders in file contents and in file paths.
//!
//! The templates live in `templates/` and are compiled into the binary, so `undra init` needs no
//! network and no files next to the executable. `@@` is used as the delimiter because it does not
//! occur in Rust, Swift, Kotlin, TypeScript, JSX, Gradle, TOML or property lists.

use std::collections::BTreeMap;

/// A file of a template set: a path (which may contain placeholders) and its contents.
#[derive(Clone, Copy, Debug)]
pub struct TemplateFile {
    /// The path relative to the directory the set is written to.
    pub path: &'static str,
    /// The contents, with placeholders.
    pub contents: &'static str,
    /// Whether the file is made executable (`gradlew`).
    pub executable: bool,
}

/// Declares a [`TemplateFile`] whose contents are embedded from `templates/<path>`.
macro_rules! template {
    ($path:literal) => {
        $crate::render::TemplateFile {
            path: $path,
            contents: include_str!(concat!("../templates/", $path)),
            executable: false,
        }
    };
    ($path:literal => $dest:literal) => {
        $crate::render::TemplateFile {
            path: $dest,
            contents: include_str!(concat!("../templates/", $path)),
            executable: false,
        }
    };
}
pub(crate) use template;

/// The values of the placeholders of one rendering.
#[derive(Clone, Debug, Default)]
pub struct Vars(BTreeMap<String, String>);

impl Vars {
    /// No variables.
    #[must_use]
    pub fn new() -> Vars {
        Vars::default()
    }

    /// Sets `name` (written `@@name@@` in templates).
    #[must_use]
    pub fn with(mut self, name: &str, value: impl Into<String>) -> Vars {
        self.0.insert(name.to_owned(), value.into());
        self
    }

    /// Sets `name` in place.
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.0.insert(name.to_owned(), value.into());
    }

    /// Replaces every `@@NAME@@` in `text`.
    ///
    /// # Errors
    ///
    /// The name of the first placeholder that has no value (a template bug: the tests render
    /// every template, so it cannot reach a user).
    pub fn render(&self, text: &str) -> Result<String, String> {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("@@") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("@@") else {
                out.push_str(&rest[start..]);
                return Ok(out);
            };
            let name = &after[..end];
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                // Not a placeholder (for example `@@` in prose): keep the first `@@` literally.
                out.push_str("@@");
                rest = after;
                continue;
            }
            match self.0.get(name) {
                Some(value) => out.push_str(value),
                None => return Err(name.to_owned()),
            }
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_placeholders_everywhere() {
        let vars = Vars::new()
            .with("NAME", "todo")
            .with("ID", "com.example.todo");
        assert_eq!(
            vars.render("a @@NAME@@ b @@ID@@ c @@NAME@@").unwrap(),
            "a todo b com.example.todo c todo"
        );
    }

    #[test]
    fn unknown_placeholders_are_reported() {
        assert_eq!(
            Vars::new().render("x @@MISSING@@ y").unwrap_err(),
            "MISSING"
        );
    }

    #[test]
    fn stray_at_signs_are_left_alone() {
        let vars = Vars::new().with("A", "1");
        assert_eq!(
            vars.render("@@ not a name @@ @@A@@").unwrap(),
            "@@ not a name @@ 1"
        );
        assert_eq!(vars.render("email@@example").unwrap(), "email@@example");
        assert_eq!(vars.render("tail @@").unwrap(), "tail @@");
        assert_eq!(
            vars.render("@MainActor @Observable").unwrap(),
            "@MainActor @Observable"
        );
    }
}

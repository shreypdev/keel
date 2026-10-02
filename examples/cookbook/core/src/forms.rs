//! Forms and validation: a signal per field, the errors derived from them, and a command that
//! refuses typed.
//!
//! The rules live once, here, instead of three times in SwiftUI, Compose and React. Each platform
//! binds its text fields to the setters and renders two signals:
//!
//! * `errors` is a [`Computed`]: the problems of the fields the user has already left (and of every
//!   field after a failed submit). It is recomputed when a field changes, never by the UI, and it
//!   reaches the platforms only when it changed.
//! * `valid` is a `Computed<bool>` for the submit button.
//!
//! `submit` is the command: it refuses an invalid form with a typed [`SubmitError::Invalid`] before
//! any request is made, and turns the server's "that email is taken" into a typed error too, so the
//! UI matches on cases instead of parsing messages.

use serde::Deserialize;
use undra::prelude::*;

use crate::net::{self, NetError};

/// A field of the sign-up form.
#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The email address.
    Email,
    /// The password.
    Password,
    /// The terms-of-service checkbox.
    Terms,
}

/// One problem with one field.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    /// The field it is about.
    pub field: Field,
    /// What to tell the user.
    pub message: String,
}

/// Why `submit` did not create an account.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitError {
    /// The form has problems; nothing was sent.
    #[error("fix {count} field(s) first")]
    Invalid {
        /// How many fields have a problem.
        count: u32,
    },
    /// The server already has an account with this email.
    #[error("that email is already registered")]
    EmailTaken,
    /// A submit is already running.
    #[error("the form is already being submitted")]
    Busy,
    /// The request failed.
    #[error("{0}")]
    Net(#[from] NetError),
}

#[derive(Deserialize)]
struct Created {
    id: u32,
}

// docs:begin forms-errors
/// Every problem of the form as it stands, in field order. A pure function: the one place the
/// rules are written, easy to test without a runtime.
fn problems(email: &str, password: &str, terms: bool) -> Vec<FieldError> {
    let mut out = Vec::new();
    let mut add = |field, message: &str| {
        out.push(FieldError {
            field,
            message: message.to_owned(),
        })
    };
    let email = email.trim();
    match email.split_once('@') {
        Some((name, host)) if !name.is_empty() && host.contains('.') && !host.ends_with('.') => {}
        _ => add(Field::Email, "Enter an email address like name@example.com"),
    }
    if password.chars().count() < 8 || !password.chars().any(|c| c.is_ascii_digit()) {
        add(Field::Password, "Use at least 8 characters, with a number");
    }
    if !terms {
        add(Field::Terms, "Accept the terms to continue");
    }
    out
}
// docs:end

// docs:begin forms-store
/// The sign-up form's store.
#[undra::store(restore = "Self::assemble")]
pub struct SignUp {
    ctx: WeakCtx,
    email: Signal<String>,
    password: Signal<String>,
    terms: Signal<bool>,
    /// The fields the user has left, whose problems are shown.
    touched: Signal<Vec<Field>>,
    submitting: Signal<bool>,
    /// The problems of the touched fields.
    errors: Computed<Vec<FieldError>>,
    /// Whether the form can be submitted.
    valid: Computed<bool>,
}
// docs:end

#[undra::api(store)]
impl SignUp {
    /// An empty form with no problem shown yet.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(
            ctx,
            Signal::new(String::new()),
            Signal::new(String::new()),
            Signal::new(false),
            Signal::new(vec![]),
            Signal::new(false),
        )
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot.
    fn assemble(
        ctx: Ctx,
        email: Signal<String>,
        password: Signal<String>,
        terms: Signal<bool>,
        touched: Signal<Vec<Field>>,
        submitting: Signal<bool>,
    ) -> Self {
        if submitting.get() {
            submitting.set(false);
        }
        let errors = Computed::new(
            (&email, &password, &terms, &touched),
            |(email, password, terms, touched)| {
                problems(email, password, *terms)
                    .into_iter()
                    .filter(|error| touched.contains(&error.field))
                    .collect::<Vec<_>>()
            },
        );
        let valid = Computed::new((&email, &password, &terms), |(email, password, terms)| {
            problems(email, password, *terms).is_empty()
        });
        Self {
            ctx: ctx.downgrade(),
            email,
            password,
            terms,
            touched,
            submitting,
            errors,
            valid,
        }
    }

    /// The email field changed.
    pub fn set_email(&self, email: String) {
        self.email.set(email);
    }

    /// The password field changed.
    pub fn set_password(&self, password: String) {
        self.password.set(password);
    }

    /// The terms checkbox changed.
    pub fn set_terms(&self, accepted: bool) {
        self.terms.set(accepted);
    }

    /// The user left `field`: from now on its problems are shown.
    pub fn blur(&self, field: Field) {
        if !self.touched.with(|touched| touched.contains(&field)) {
            self.touched.update(|touched| touched.push(field));
        }
    }

    // docs:begin forms-submit
    /// Creates the account and returns its id. An invalid form is refused with
    /// `Invalid` (and every problem is shown from then on); nothing is sent.
    pub async fn submit(&self) -> Result<u32, SubmitError> {
        if self.submitting.get() {
            return Err(SubmitError::Busy);
        }
        let found = problems(&self.email.get(), &self.password.get(), self.terms.get());
        if !found.is_empty() {
            self.touched
                .set(vec![Field::Email, Field::Password, Field::Terms]);
            return Err(SubmitError::Invalid {
                count: u32::try_from(found.len()).unwrap_or(u32::MAX),
            });
        }
        let ctx = self.ctx.upgrade().map_err(|_| NetError::Closed)?;
        self.submitting.set(true);
        let result = self.send(&ctx).await;
        self.submitting.set(false);
        result
    }
    // docs:end

    /// The request of `submit`, so that `submitting` is cleared on every way out of it.
    async fn send(&self, ctx: &Ctx) -> Result<u32, SubmitError> {
        let url = net::url(ctx, "/signup")?;
        let body = serde_json::json!({
            "email": self.email.get().trim(),
            "password": self.password.get(),
        });
        let response = net::send(ctx, net::post_json(url, &body)).await?;
        if response.status == 409 {
            return Err(SubmitError::EmailTaken);
        }
        let created: Created = net::json(&net::ok(response)?)?;
        Ok(created.id)
    }
}

#[cfg(test)]
mod tests {
    use undra::ports::HttpResponse;
    use undra::ports::fakes::Matcher;

    use super::*;
    use crate::net::testing::{App, BASE, json_response};

    fn fields(form: &SignUp) -> Vec<Field> {
        form.errors.get().into_iter().map(|e| e.field).collect()
    }

    #[test]
    fn nothing_is_shown_until_the_user_leaves_a_field() {
        let app = App::new();
        let form = SignUp::new(app.ctx());
        assert!(
            fields(&form).is_empty(),
            "an empty form is not an angry form"
        );
        assert!(!form.valid.get());
        form.set_email("ada".into());
        assert!(fields(&form).is_empty(), "typing is not leaving");
        form.blur(Field::Email);
        assert_eq!(fields(&form), [Field::Email]);
        form.set_email("ada@example.com".into());
        assert!(
            fields(&form).is_empty(),
            "the error goes as soon as the field is right"
        );
    }

    #[test]
    fn the_form_is_valid_when_every_rule_holds() {
        let app = App::new();
        let form = SignUp::new(app.ctx());
        form.set_email("ada@example.com".into());
        form.set_password("hunter22!".into());
        assert!(!form.valid.get(), "the terms are not accepted");
        form.set_terms(true);
        assert!(form.valid.get());
        form.set_password("short1".into());
        assert!(!form.valid.get());
    }

    #[test]
    fn an_invalid_submit_is_refused_before_anything_is_sent_and_shows_every_problem() {
        let app = App::new();
        let form = SignUp::new(app.ctx());
        assert_eq!(
            app.run(form.submit()),
            Err(SubmitError::Invalid { count: 3 })
        );
        assert_eq!(app.fakes.http.call_count(), 0);
        assert_eq!(fields(&form), [Field::Email, Field::Password, Field::Terms]);
        assert!(!form.submitting.get());
    }

    #[test]
    fn a_valid_submit_creates_the_account() {
        let app = App::new();
        app.fakes.http.respond(
            Matcher::post(format!("{BASE}/signup")),
            json_response(201, serde_json::json!({"id": 42})),
        );
        let form = SignUp::new(app.ctx());
        form.set_email(" ada@example.com ".into());
        form.set_password("hunter22!".into());
        form.set_terms(true);
        assert_eq!(app.run(form.submit()), Ok(42));
        let sent = app.fakes.http.last_call().unwrap();
        assert_eq!(
            sent.body.as_ref().map(|b| b.0.clone()),
            Some(br#"{"email":"ada@example.com","password":"hunter22!"}"#.to_vec())
        );
        assert!(!form.submitting.get());
    }

    #[test]
    fn the_servers_refusals_are_typed_too() {
        let app = App::new();
        let form = SignUp::new(app.ctx());
        form.set_email("ada@example.com".into());
        form.set_password("hunter22!".into());
        form.set_terms(true);
        app.fakes.http.respond(
            Matcher::post(format!("{BASE}/signup")),
            HttpResponse::new(409, b"taken".to_vec()),
        );
        assert_eq!(app.run(form.submit()), Err(SubmitError::EmailTaken));
        assert!(!form.submitting.get(), "the form can be submitted again");
        app.fakes.http.reset();
        app.fakes.http.respond(
            Matcher::post(format!("{BASE}/signup")),
            HttpResponse::new(500, b"boom".to_vec()),
        );
        assert_eq!(
            app.run(form.submit()),
            Err(SubmitError::Net(NetError::Status { code: 500 }))
        );
    }

    #[test]
    fn a_second_submit_while_one_runs_is_refused() {
        let app = App::new();
        let form = SignUp::new(app.ctx());
        form.submitting.set(true);
        assert_eq!(app.run(form.submit()), Err(SubmitError::Busy));
    }

    #[test]
    fn the_rules_are_plain_functions() {
        assert!(problems("a@b.co", "abcdefg1", true).is_empty());
        assert_eq!(problems("a@b", "abcdefg1", true).len(), 1);
        assert_eq!(problems("@b.co", "abcdefg1", true).len(), 1);
        assert_eq!(problems("a@b.co", "abcdefgh", true).len(), 1);
        assert_eq!(problems("", "", false).len(), 3);
    }
}

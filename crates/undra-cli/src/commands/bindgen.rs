//! `undra bindgen`.

use std::path::{Path, PathBuf};

use undra_bindgen::Generator;
use undra_meta::Schema;

use crate::bindgen::{self, Plan, canonicalize_lenient};
use crate::builds::host;
use crate::cli::BindgenArgs;
use crate::config::Platform;
use crate::error::{CliError, Code, Result};
use crate::names::Names;
use crate::project::Project;
use crate::runtimes::Runtimes;
use crate::schema::{self, parse_schema_json};
use crate::session::Session;

use super::Env;

/// Runs `undra bindgen`.
///
/// # Errors
///
/// See the modules it drives: [`crate::schema`], [`crate::bindgen`], [`crate::builds`].
pub fn run(env: &Env<'_>, args: &BindgenArgs) -> Result<()> {
    let ui = env.ui;
    let project = match Project::discover(&env.start_dir()?) {
        Ok(project) => Some(project),
        // `--schema` needs no project: generating from a file is the fallback of SPEC 13.
        Err(e) if e.code == Code::NoProject && args.schema.is_some() => None,
        Err(e) => return Err(e),
    };
    let session = project.map(|p| Session::new(p, env.sys, ui));

    let crate_name = crate_name(session.as_ref(), args);
    let schema = match (&args.schema, &session) {
        (Some(file), _) => read_schema_file(file, &crate_name)?,
        (None, Some(session)) => schema_from_core(session, args.release, args.docs)?,
        (None, None) => return Err(CliError::no_project(&env.start_dir()?)),
    };
    if schema_is_empty(&schema) {
        ui.warn("the schema is empty: the core has no `#[undra::api]` items that are `pub`, or its registrations were not linked");
    }

    let plan = plan(session.as_ref(), args, &schema, &env.start_dir()?)?;
    let files = bindgen::plan_files(&schema, &plan)?;

    if args.check {
        let problems = bindgen::check(&plan.out, &files);
        if problems.is_empty() {
            ui.line(&format!(
                "The bindings in {} are up to date (schema hash {:#018x}).",
                plan.out.display(),
                schema.hash()
            ));
            return Ok(());
        }
        return Err(CliError::new(
            Code::Bindgen,
            format!("the generated bindings in {} are out of date", plan.out.display()),
            "they are generated from the core's schema, and the core changed (or they were edited by hand)",
            "run `undra bindgen` and commit the result",
        )
        .with_detail(problems.iter().map(|p| format!("  {p}")).collect::<Vec<_>>().join("\n")));
    }

    let applied = bindgen::apply(&plan.out, &files)?;
    let shown = |p: &Path| {
        session
            .as_ref()
            .and_then(|s| p.strip_prefix(&s.project.root).ok())
            .unwrap_or(p)
            .display()
            .to_string()
    };
    ui.line(&format!(
        "Generated bindings for {} (schema hash {:#018x}) in {}:",
        schema.crate_name,
        schema.hash(),
        shown(&plan.out)
    ));
    for platform in &plan.platforms {
        let (dir, what) = match platform {
            Platform::Ios => (
                "swift",
                format!("Swift package `{}`", plan.generator.swift_module),
            ),
            Platform::Android => (
                "kotlin",
                format!("Kotlin module, package `{}`", plan.generator.kotlin_package),
            ),
            Platform::Web => (
                "ts",
                format!("TypeScript package `{}`", plan.generator.ts_package_name()),
            ),
        };
        let count = files
            .iter()
            .filter(|f| f.path.starts_with(&format!("{dir}/")))
            .count();
        ui.line(&format!("  {dir:<7} {what} ({count} files)"));
    }
    ui.line(&format!(
        "  {} written, {} unchanged, {} removed",
        applied.written.len(),
        applied.unchanged,
        applied.removed.len()
    ));
    Ok(())
}

fn schema_is_empty(schema: &Schema) -> bool {
    schema.records.is_empty()
        && schema.enums.is_empty()
        && schema.objects.is_empty()
        && schema.functions.is_empty()
        && schema.ports.is_empty()
        && schema.queries.is_empty()
}

/// The crate name that labels a schema with none: `--crate-name`, else the project's core.
fn crate_name(session: Option<&Session<'_>>, args: &BindgenArgs) -> String {
    if let Some(name) = &args.crate_name {
        return name.clone();
    }
    match session {
        Some(s) => s
            .project
            .config
            .core_package
            .clone()
            .unwrap_or_else(|| Names::derive(&s.project.config.name).core_package),
        None => "core".to_owned(),
    }
}

fn read_schema_file(file: &Path, crate_name: &str) -> Result<Schema> {
    let text = std::fs::read_to_string(file).map_err(|e| CliError::io("read", file, &e))?;
    parse_schema_json(&text, crate_name)
        .map_err(|e| CliError::bad_config(file, e.what.clone(), e.fix.clone()))
}

/// Builds the core as a host library and asks it for its schema.
///
/// The library's schema carries the doc comments (the same JSON the dev runner prints, ADR-050),
/// so `--docs` only decides whether the bindings keep them: without it they are dropped, which is
/// what generated bindings have always been by default.
fn schema_from_core(session: &Session<'_>, release: bool, docs: bool) -> Result<Schema> {
    session
        .ui
        .step("Building the core for this machine to read its schema");
    let library = host::cdylib(session, release)?;
    let core = session.core()?;
    session.ui.step("Reading the schema from the built library");
    schema::load_from_library(&library, &core.package, docs)
}

/// The generator configuration and output locations.
fn plan(
    session: Option<&Session<'_>>,
    args: &BindgenArgs,
    schema: &Schema,
    cwd: &Path,
) -> Result<Plan> {
    let mut generator = Generator::for_crate(&schema.crate_name);
    let mut platforms = Platform::ALL.to_vec();
    let mut runtimes = Runtimes::from_registries(crate::config::UNDRA_VERSION);
    let mut default_out = cwd.join("generated");
    if let Some(session) = session {
        let project = &session.project;
        let cfg = &project.config.bindings;
        generator.swift_module = project.swift_module();
        generator.kotlin_package = project.kotlin_package();
        if let Some(scope) = &cfg.ts_scope {
            generator.ts_scope.clone_from(scope);
        }
        if let Some(package) = &cfg.ts_package {
            generator.ts_package.clone_from(package);
        }
        generator.ts_js_number = cfg.ts_js_number;
        if let Some(typed) = cfg.swift_typed_throws {
            generator.swift_typed_throws = typed;
        }
        platforms.clone_from(&project.config.platforms);
        runtimes = Runtimes::for_project(project);
        default_out = project.generated_dir();
    }
    if let Some(list) = &args.platforms {
        platforms = Platform::parse_list(list)?;
    }
    let out: PathBuf = match &args.out {
        Some(out) if out.is_absolute() => out.clone(),
        Some(out) => cwd.join(out),
        None => default_out,
    };
    Ok(Plan {
        generator,
        platforms,
        runtimes,
        out: canonicalize_lenient(&out),
    })
}

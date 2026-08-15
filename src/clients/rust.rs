//! The Rust client generator, in both flavours.
//!
//! Rust is the one language here with two calling conventions, so it
//! is the one language that emits two clients. Everything a client
//! says about the contract -- the types, the renames, the nullability,
//! the auth scheme, the URL and query shaping -- is identical between
//! them. What differs is only how a call suspends.
//!
//! So there is one generator taking a [`Flavor`], rather than two
//! generators to keep in step. The alternative was a copy that drifts:
//! a scheme change or a new action lands in the async client, and the
//! blocking one keeps compiling while quietly describing an older
//! contract. Byte-identical async output before and after this became
//! parameterised is what the golden asserts.

use std::fmt::Write as _;

use surql::schema::{FieldType, TableDefinition};

use crate::ir::{ActionOutput, Contract};
use crate::naming::{singular, snake, type_name};
use crate::openapi::GenerateError;

use super::{
    checked, column, query_kind, query_params, query_takes_id, sub_method_stem, sub_table,
    sub_type_name,
};

/// Rust keywords that a raw identifier cannot rescue. `r#crate` and
/// friends are rejected by the compiler, so a field or binding landing
/// on one of these is suffixed instead.
const UNRAWABLE: &[&str] = &["crate", "self", "Self", "super"];

/// Every Rust keyword, strict and reserved, plus the ones only
/// meaningful in an edition this generator targets. A column named
/// `type` is ordinary in a schema and ordinary on the wire; it is only
/// Rust that cannot spell it plainly, which is why this list lives in
/// the Rust generator and not in the contract's own reserved gate.
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

/// A Rust identifier for `name`, and the serde rename it needs to keep
/// the wire name it had.
///
/// Raw identifiers carry their own name through serde, which strips
/// the `r#`, so `r#type` still reads and writes `"type"` and needs no
/// attribute. The four keywords that cannot be raw take a suffix and
/// therefore do need one.
fn rust_ident(name: &str) -> (String, Option<String>) {
    if UNRAWABLE.contains(&name) {
        (format!("{name}_"), Some(name.to_owned()))
    } else if KEYWORDS.contains(&name) {
        (format!("r#{name}"), None)
    } else {
        (name.to_owned(), None)
    }
}

/// A local binding for a query parameter that neither collides with a
/// keyword nor shadows a local the generated method already holds.
/// The wire name is passed separately, so renaming the binding is
/// invisible to the caller.
fn rust_binding(name: &str, taken: &[String]) -> String {
    let (mut ident, _) = rust_ident(name);
    while taken.iter().any(|t| t == &ident) {
        ident.push('_');
    }
    ident
}

/// Whether anything in the generated file names `Value`: an open
/// column type, an action's input or JSON output, or any query, all of
/// which return open JSON.
fn needs_value(
    contract: &Contract,
    resources: &[(&crate::ir::Resource, &TableDefinition)],
    schema: &[TableDefinition],
) -> bool {
    if !contract.queries.is_empty() {
        return true;
    }
    resources.iter().any(|(resource, table)| {
        resource
            .actions
            .iter()
            .any(|action| !action.input.is_empty() || matches!(action.output, ActionOutput::Json))
            || open_column(&resource.fields, table)
            || resource
                .sub_resources
                .iter()
                .any(|sub| open_column(&sub.fields, sub_table(schema, sub)))
    })
}

/// Whether any exposed column maps to `Value` rather than a scalar.
fn open_column(fields: &[crate::ir::FieldExposure], table: &TableDefinition) -> bool {
    fields.iter().any(|exposure| {
        matches!(
            column(table, &exposure.column).field_type,
            FieldType::Object | FieldType::Array | FieldType::Geometry | FieldType::Any
        )
    })
}

/// How a generated client suspends: the whole of the difference
/// between the two Rust clients, in four fragments.
#[derive(Debug, Clone, Copy)]
struct Flavor {
    /// Spliced before every `fn`: `"async "`, or nothing.
    asyncness: &'static str,
    /// Spliced after every call that suspends: `".await"`, or nothing.
    awaited: &'static str,
    /// The reqwest client path, which differs by module.
    http: &'static str,
    /// The reqwest features the header tells callers to enable.
    features: &'static str,
}

const ASYNC: Flavor = Flavor {
    asyncness: "async ",
    awaited: ".await",
    http: "reqwest::Client",
    features: "\"json\"",
};

const BLOCKING: Flavor = Flavor {
    asyncness: "",
    awaited: "",
    http: "reqwest::blocking::Client",
    features: "\"json\", \"blocking\"",
};

/// Generate the async Rust client (reqwest + serde).
pub fn generate_client_rs(
    contract: &Contract,
    schema: &[TableDefinition],
) -> Result<String, GenerateError> {
    generate(contract, schema, ASYNC)
}

/// Generate the blocking Rust client (reqwest::blocking + serde).
///
/// The same contract, for callers with no runtime to await on: build
/// scripts, CLIs, test harnesses, and synchronous services that would
/// otherwise stand up an executor to make one call.
pub fn generate_client_rs_blocking(
    contract: &Contract,
    schema: &[TableDefinition],
) -> Result<String, GenerateError> {
    generate(contract, schema, BLOCKING)
}

#[allow(clippy::too_many_lines)]
fn generate(
    contract: &Contract,
    schema: &[TableDefinition],
    flavor: Flavor,
) -> Result<String, GenerateError> {
    let resources = checked(contract, schema)?;
    let prefix = contract.prefix();
    let Flavor {
        asyncness,
        awaited,
        http,
        features,
    } = flavor;
    // Imported, and declared as a dependency, only when something
    // names it: an unused import is a warning in the consumer's build,
    // and they cannot edit a generated file to silence it.
    let value = needs_value(contract, &resources, schema);
    let json_dep = if value { ", serde_json" } else { "" };
    let mut out = String::new();
    writeln!(
        out,
        "//! Generated by janus for the `{}` contract v{}. Do not edit.\n\
         //!\n\
         //! Dependencies: reqwest = {{ features = [{features}] }}, serde =\n\
         //! {{ features = [\"derive\"] }}{json_dep}.\n",
        contract.name, contract.version,
    )
    .unwrap();
    out.push_str("use serde::Deserialize;\n");
    if value {
        out.push_str("use serde_json::Value;\n");
    }
    out.push('\n');

    for (resource, table) in &resources {
        rust_struct(
            &mut out,
            &type_name(&resource.name),
            &resource.fields,
            table,
        );
        for sub in &resource.sub_resources {
            rust_struct(
                &mut out,
                &sub_type_name(resource, sub),
                &sub.fields,
                sub_table(schema, sub),
            );
        }
    }

    // The credential is whatever the contract declared, down to what it
    // is called: a service authenticating with a bearer token gets
    // `Client::new(url, token)`, one with a tenant header gets
    // `Client::new(url, tenant)`, and one with neither gets
    // `Client::new(url)` and no field to carry.
    let credential = contract.auth.credential_name();
    // Built once and spliced into every request below, so a scheme
    // change cannot reach some call sites and miss others.
    let auth_header = match contract.auth.header() {
        Some((name, scheme_prefix)) => {
            let value = credential.expect("a header scheme names its credential");
            if scheme_prefix.is_empty() {
                format!(".header(\"{name}\", &self.{value})")
            } else {
                format!(".header(\"{name}\", format!(\"{scheme_prefix}{{}}\", self.{value}))")
            }
        }
        None => String::new(),
    };
    // The tail every request shares. Spliced rather than repeated so
    // the two flavours cannot disagree about error handling either.
    let sent = format!("{awaited}?.error_for_status()?");
    let received = format!(".json(){awaited}?");

    let field = credential.map_or_else(String::new, |name| format!("    {name}: String,\n"));
    let param = credential.map_or_else(String::new, |name| format!(", {name}: impl Into<String>"));
    let init = credential.map_or_else(String::new, |name| {
        format!("            {name}: {name}.into(),\n")
    });
    writeln!(
        out,
        "#[derive(Debug, Clone)]\n\
         pub struct Client {{\n\
         \x20   base_url: String,\n\
         {field}\
         \x20   http: {http},\n\
         }}\n\n\
         pub type Error = Box<dyn std::error::Error + Send + Sync>;\n\n\
         impl Client {{\n\
         \x20   pub fn new(base_url: impl Into<String>{param}) -> Self {{\n\
         \x20       Self {{\n\
         \x20           base_url: base_url.into(),\n\
         {init}\
         \x20           http: {http}::new(),\n\
         \x20       }}\n\
         \x20   }}\n",
    )
    .unwrap();

    for (resource, _) in &resources {
        let name = type_name(&resource.name);
        let list_fn = format!("list_{}", snake(&resource.name));
        let get_fn = format!("get_{}", snake(&singular(&resource.name)));
        writeln!(
            out,
            "    pub {asyncness}fn {list_fn}(&self, limit: Option<u32>, cursor: Option<&str>) \
             -> Result<{name}Page, Error> {{"
        )
        .unwrap();
        writeln!(
            out,
            "        let mut url = format!(\"{{}}{prefix}/{}\", self.base_url);",
            resource.name,
        )
        .unwrap();
        out.push_str(
            "        let mut query: Vec<(String, String)> = Vec::new();\n\
             \x20       if let Some(limit) = limit { query.push((\"limit\".into(), limit.to_string())); }\n\
             \x20       if let Some(cursor) = cursor { query.push((\"cursor\".into(), cursor.to_string())); }\n\
             \x20       if !query.is_empty() {\n\
             \x20           let joined: Vec<String> = query.iter().map(|(k, v)| format!(\"{k}={v}\")).collect();\n\
             \x20           url = format!(\"{url}?{}\", joined.join(\"&\"));\n\
             \x20       }\n",
        );
        writeln!(
            out,
            "        Ok(self.http.get(url){auth_header}\
             .send(){sent}{received})"
        )
        .unwrap();
        out.push_str("    }\n\n");
        writeln!(
            out,
            "    pub {asyncness}fn {get_fn}(&self, id: &str) -> Result<{name}, Error> {{"
        )
        .unwrap();
        writeln!(
            out,
            "        let url = format!(\"{{}}{prefix}/{}/{{id}}\", self.base_url);",
            resource.name,
        )
        .unwrap();
        writeln!(
            out,
            "        Ok(self.http.get(url){auth_header}\
             .send(){sent}{received})"
        )
        .unwrap();
        out.push_str("    }\n\n");

        // One list method per sub-collection, reached through the
        // parent id.
        for sub in &resource.sub_resources {
            let sub_name = sub_type_name(resource, sub);
            writeln!(
                out,
                "    pub {asyncness}fn {stem}(&self, id: &str, limit: Option<u32>, cursor: Option<&str>)                  -> Result<{sub_name}Page, Error> {{",
                stem = sub_method_stem(resource, sub),
            )
            .unwrap();
            writeln!(
                out,
                "        let mut url = format!(\"{{}}{prefix}/{}/{{id}}/{}\", self.base_url);",
                resource.name, sub.name,
            )
            .unwrap();
            out.push_str(
                "        let mut query: Vec<(String, String)> = Vec::new();
                         if let Some(limit) = limit { query.push((\"limit\".into(), limit.to_string())); }
                         if let Some(cursor) = cursor { query.push((\"cursor\".into(), cursor.to_string())); }
                         if !query.is_empty() {
                             let joined: Vec<String> = query.iter().map(|(k, v)| format!(\"{k}={v}\")).collect();
                             url = format!(\"{url}?{}\", joined.join(\"&\"));
                         }
",
            );
            // A separate write, because the line above is a plain
            // push_str and would emit `{auth_header}` verbatim.
            writeln!(
                out,
                "                         Ok(self.http.get(url){auth_header}                 .send(){sent}{received})\n    }}\n",
            )
            .unwrap();
        }
        for action in &resource.actions {
            let method_fn = format!(
                "{}_{}",
                snake(&action.name),
                snake(&singular(&resource.name))
            );
            let mut parameters = vec!["&self".to_owned()];
            if action.takes_id() {
                parameters.push("id: &str".to_owned());
            }
            if !action.input.is_empty() {
                parameters.push("input: Value".to_owned());
            }
            let output = match action.output {
                ActionOutput::Resource => name.clone(),
                ActionOutput::Json => "Value".to_owned(),
                ActionOutput::None => "()".to_owned(),
            };
            writeln!(
                out,
                "    pub {asyncness}fn {method_fn}({}) -> Result<{output}, Error> {{",
                parameters.join(", "),
            )
            .unwrap();
            // The literal `{id}` survives into the generated format!
            // string, where the method's `id` parameter interpolates.
            writeln!(
                out,
                "        let url = format!(\"{{}}{prefix}/{}{}\", self.base_url);",
                resource.name, action.path,
            )
            .unwrap();
            let verb = action.method.to_ascii_lowercase();
            let mut call = format!("self.http.{verb}(url){auth_header}");
            if !action.input.is_empty() {
                call.push_str(".json(&input)");
            }
            match action.output {
                ActionOutput::None => {
                    writeln!(out, "        {call}.send(){sent};\n        Ok(())").unwrap();
                }
                _ => {
                    writeln!(out, "        Ok({call}.send(){sent}{received})").unwrap();
                }
            }
            out.push_str("    }\n\n");
        }
    }
    // Queries: declared reads that are not listings. They reach REST
    // as GETs with their inputs in the query string, which is why they
    // carry no body and return open JSON.
    for query in &contract.queries {
        let mut parameters = vec!["&self".to_owned()];
        // The locals the method below holds, which a parameter must
        // not shadow: `url` and `request` are bound by the generated
        // body, `value` by the optional-parameter branch, and `id` is
        // already a parameter when the path takes one.
        let mut taken: Vec<String> = ["url", "request", "value"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        if query_takes_id(query) {
            parameters.push("id: &str".to_owned());
            taken.push("id".to_owned());
        }
        // Resolved once, so the signature and the calls below cannot
        // disagree about what a parameter is called. Each resolved name
        // joins `taken`, so two parameters cannot land on it either.
        let mut bindings: Vec<(String, &crate::ir::ActionField)> = Vec::new();
        for field in query_params(query) {
            let binding = rust_binding(&snake(&field.name), &taken);
            taken.push(binding.clone());
            bindings.push((binding, field));
        }
        for (binding, field) in &bindings {
            let kind = query_kind(field.kind, "rs");
            parameters.push(if field.required {
                format!("{binding}: {kind}")
            } else {
                format!("{binding}: Option<{kind}>")
            });
        }
        writeln!(
            out,
            "    pub {asyncness}fn {}({}) -> Result<Value, Error> {{",
            snake(&query.name),
            parameters.join(", "),
        )
        .unwrap();
        writeln!(
            out,
            "        let url = format!(\"{{}}{}\", self.base_url);",
            query.path,
        )
        .unwrap();
        // `mut` only when something reassigns it, so a query with no
        // parameters does not generate a warning in the client.
        let binding = if query_params(query).is_empty() {
            "let request"
        } else {
            "let mut request"
        };
        // The header goes on its own line here, where the builder is bound
        // rather than chained into a return, so the generated line stays
        // inside a sane width. With no scheme there is nothing to wrap.
        let wrapped = if auth_header.is_empty() {
            String::new()
        } else {
            format!("\n            {auth_header}")
        };
        writeln!(out, "        {binding} = self.http.get(url){wrapped};",).unwrap();
        for (binding, field) in &bindings {
            let name = &field.name;
            if field.required {
                writeln!(
                    out,
                    "        request = request.query(&[(\"{name}\", {binding})]);",
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "        if let Some(value) = {binding} {{\n\
                     \x20           request = request.query(&[(\"{name}\", value)]);\n\
                     \x20       }}",
                )
                .unwrap();
            }
        }
        writeln!(out, "        Ok(request.send(){sent}{received})\n    }}\n").unwrap();
    }
    out.push_str("}\n");
    Ok(out)
}

fn rust_struct(
    out: &mut String,
    name: &str,
    fields: &[crate::ir::FieldExposure],
    table: &TableDefinition,
) {
    writeln!(out, "#[derive(Debug, Clone, Deserialize)]").unwrap();
    writeln!(out, "pub struct {name} {{").unwrap();
    writeln!(out, "    pub id: String,").unwrap();
    for exposure in fields {
        let field = column(table, &exposure.column);
        let base = match field.field_type {
            FieldType::Int => "i64",
            FieldType::Float | FieldType::Decimal | FieldType::Number => "f64",
            FieldType::Bool => "bool",
            FieldType::Object | FieldType::Array | FieldType::Geometry | FieldType::Any => "Value",
            _ => "String",
        };
        let optional = field.nullable || exposure.guard.is_some();
        let ty = if optional {
            format!("Option<{base}>")
        } else {
            base.to_owned()
        };
        let api = exposure.api_name();
        if optional {
            writeln!(out, "    #[serde(default)]").unwrap();
        }
        // A column named `type` is fine in a schema, fine on the wire,
        // and fine in the other three clients. Only Rust needs it
        // spelled differently, and it must still deserialize from the
        // name the contract exposes.
        let (ident, rename) = rust_ident(&snake(api));
        if let Some(wire) = rename {
            writeln!(out, "    #[serde(rename = \"{wire}\")]").unwrap();
        }
        writeln!(out, "    pub {ident}: {ty},").unwrap();
    }
    writeln!(out, "}}\n").unwrap();
    writeln!(out, "#[derive(Debug, Clone, Deserialize)]").unwrap();
    writeln!(out, "pub struct {name}Page {{").unwrap();
    writeln!(out, "    pub items: Vec<{name}>,").unwrap();
    writeln!(out, "    #[serde(default)]").unwrap();
    writeln!(out, "    pub next_cursor: Option<String>,").unwrap();
    writeln!(out, "}}\n").unwrap();
}

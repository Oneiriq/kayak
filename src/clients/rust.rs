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

use surql::schema::{FieldType, TableDefinition};

use crate::emit::wln;
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
) -> Result<bool, GenerateError> {
    if !contract.queries.is_empty() {
        return Ok(true);
    }
    // Written as loops rather than chained `any`, because the column
    // lookup can fail and a fallible closure inside `any` either
    // swallows that or turns the expression inside out.
    for (resource, table) in resources {
        let open_action = resource
            .actions
            .iter()
            .any(|action| !action.input.is_empty() || matches!(action.output, ActionOutput::Json));
        if open_action || open_column(&resource.fields, table)? {
            return Ok(true);
        }
        for sub in &resource.sub_resources {
            if open_column(&sub.fields, sub_table(schema, sub)?)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether any exposed column maps to `Value` rather than a scalar.
fn open_column(
    fields: &[crate::ir::FieldExposure],
    table: &TableDefinition,
) -> Result<bool, GenerateError> {
    for exposure in fields {
        if matches!(
            column(table, &exposure.column)?.field_type,
            FieldType::Object | FieldType::Array | FieldType::Geometry | FieldType::Any
        ) {
            return Ok(true);
        }
    }
    Ok(false)
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
    let value = needs_value(contract, &resources, schema)?;
    let json_dep = if value { ", serde_json" } else { "" };
    let mut out = String::new();
    wln!(
        out,
        "//! Generated by janus for the `{}` contract v{}. Do not edit.\n\
         //!\n\
         //! Dependencies: reqwest = {{ features = [{features}] }}, serde =\n\
         //! {{ features = [\"derive\"] }}{json_dep}.\n",
        contract.name,
        contract.version,
    );
    out.push_str("use serde::Deserialize;\n");
    if value {
        out.push_str("use serde_json::Value;\n");
    }
    out.push('\n');

    for (resource, table) in &resources {
        rust_struct(
            &mut out,
            &type_name(&resource.name),
            resource.wire_identity(),
            &resource.fields,
            table,
        )?;
        for sub in &resource.sub_resources {
            rust_struct(
                &mut out,
                &sub_type_name(resource, sub),
                sub.identity.wire_column(),
                &sub.fields,
                sub_table(schema, sub)?,
            )?;
        }
    }

    // The credential is whatever the contract declared, down to what it
    // is called: a service authenticating with a bearer token gets
    // `Client::new(url, token)`, one with a tenant header gets
    // `Client::new(url, tenant)`, and one with neither gets
    // `Client::new(url)` and no field to carry.
    let wire = contract.auth.wire();
    // Built once and spliced into every request below, so a scheme
    // change cannot reach some call sites and miss others.
    let auth_header = wire.map_or_else(String::new, |auth| {
        let (header, credential) = (auth.header, auth.credential);
        if auth.prefix.is_empty() {
            format!(".header(\"{header}\", &self.{credential})")
        } else {
            let prefix = auth.prefix;
            format!(".header(\"{header}\", format!(\"{prefix}{{}}\", self.{credential}))")
        }
    });
    // The tail every request shares. Spliced rather than repeated so
    // the two flavours cannot disagree about error handling either.
    let sent = format!("{awaited}?.error_for_status()?");
    let received = format!(".json(){awaited}?");

    let field = wire.map_or_else(String::new, |a| format!("    {}: String,\n", a.credential));
    let param = wire.map_or_else(String::new, |a| {
        format!(", {}: impl Into<String>", a.credential)
    });
    let init = wire.map_or_else(String::new, |a| {
        format!("            {0}: {0}.into(),\n", a.credential)
    });
    wln!(
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
    );

    for (resource, _) in &resources {
        let name = type_name(&resource.name);
        let list_fn = format!("list_{}", snake(&resource.name));
        let get_fn = format!("get_{}", snake(&singular(&resource.name)));
        if resource.faces.list {
            wln!(
                out,
                "    pub {asyncness}fn {list_fn}(&self, limit: Option<u32>, cursor: Option<&str>) \
                 -> Result<{name}Page, Error> {{"
            );
            wln!(
                out,
                "        let url = format!(\"{{}}{prefix}/{}\", self.base_url);",
                resource.name,
            );
            // Handed to reqwest rather than joined by hand. A cursor is
            // opaque to the caller and routinely base64, so it carries
            // `+`, `/` and `=`; pasted straight into a query string, a
            // `+` reaches the server as a space and the page after it is
            // not the page that was asked for.
            out.push_str(
                "        let mut query: Vec<(&str, String)> = Vec::new();\n\
                 \x20       if let Some(limit) = limit { query.push((\"limit\", limit.to_string())); }\n\
                 \x20       if let Some(cursor) = cursor { query.push((\"cursor\", cursor.to_string())); }\n",
            );
            wln!(
                out,
                "        Ok(self.http.get(url){auth_header}.query(&query)\
                 .send(){sent}{received})"
            );
            out.push_str("    }\n\n");
        }
        if resource.faces.get {
            wln!(
                out,
                "    pub {asyncness}fn {get_fn}(&self, id: &str) -> Result<{name}, Error> {{"
            );
            wln!(
                out,
                "        let url = format!(\"{{}}{prefix}/{}/{{id}}\", self.base_url);",
                resource.name,
            );
            wln!(
                out,
                "        Ok(self.http.get(url){auth_header}\
                 .send(){sent}{received})"
            );
            out.push_str("    }\n\n");
        }

        // One list method per sub-collection, reached through the
        // parent id.
        for sub in &resource.sub_resources {
            let sub_name = sub_type_name(resource, sub);
            wln!(
                out,
                "    pub {asyncness}fn {stem}(&self, id: &str, limit: Option<u32>, cursor: Option<&str>)                  -> Result<{sub_name}Page, Error> {{",
                stem = sub_method_stem(resource, sub),
            );
            wln!(
                out,
                "        let url = format!(\"{{}}{prefix}/{}/{{id}}/{}\", self.base_url);",
                resource.name,
                sub.name,
            );
            out.push_str(
                "        let mut query: Vec<(&str, String)> = Vec::new();
                         if let Some(limit) = limit { query.push((\"limit\", limit.to_string())); }
                         if let Some(cursor) = cursor { query.push((\"cursor\", cursor.to_string())); }
",
            );
            // A separate write, because the line above is a plain
            // push_str and would emit `{auth_header}` verbatim.
            wln!(
                out,
                "                         Ok(self.http.get(url){auth_header}.query(&query)                 .send(){sent}{received})\n    }}\n",
            );
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
            wln!(
                out,
                "    pub {asyncness}fn {method_fn}({}) -> Result<{output}, Error> {{",
                parameters.join(", "),
            );
            // The literal `{id}` survives into the generated format!
            // string, where the method's `id` parameter interpolates.
            wln!(
                out,
                "        let url = format!(\"{{}}{prefix}/{}{}\", self.base_url);",
                resource.name,
                action.path,
            );
            let verb = action.method.to_ascii_lowercase();
            let mut call = format!("self.http.{verb}(url){auth_header}");
            if !action.input.is_empty() {
                call.push_str(".json(&input)");
            }
            match action.output {
                ActionOutput::None => {
                    wln!(out, "        {call}.send(){sent};\n        Ok(())");
                }
                _ => {
                    wln!(out, "        Ok({call}.send(){sent}{received})");
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
        wln!(
            out,
            "    pub {asyncness}fn {}({}) -> Result<Value, Error> {{",
            snake(&query.name),
            parameters.join(", "),
        );
        wln!(
            out,
            "        let url = format!(\"{{}}{}\", self.base_url);",
            query.path,
        );
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
        wln!(out, "        {binding} = self.http.get(url){wrapped};",);
        for (binding, field) in &bindings {
            let name = &field.name;
            if field.required {
                wln!(
                    out,
                    "        request = request.query(&[(\"{name}\", {binding})]);",
                );
            } else {
                wln!(
                    out,
                    "        if let Some(value) = {binding} {{\n\
                     \x20           request = request.query(&[(\"{name}\", value)]);\n\
                     \x20       }}",
                );
            }
        }
        wln!(out, "        Ok(request.send(){sent}{received})\n    }}\n");
    }
    out.push_str("}\n");
    Ok(out)
}

fn rust_struct(
    out: &mut String,
    name: &str,
    identity: Option<&str>,
    fields: &[crate::ir::FieldExposure],
    table: &TableDefinition,
) -> Result<(), GenerateError> {
    wln!(out, "#[derive(Debug, Clone, Deserialize)]");
    wln!(out, "pub struct {name} {{");
    // The identity column -- when the rows name themselves at all, and
    // unless the resource already exposes it as a field, in which case
    // the loop below emits it once, with its real type. Emitting `id`
    // unconditionally is how this generator came to describe a
    // `Presence` the service never sends.
    if let Some(identity) = identity.filter(|id| !fields.iter().any(|f| f.api_name() == *id)) {
        let (ident, rename) = rust_ident(&snake(identity));
        if let Some(wire) = rename {
            wln!(out, "    #[serde(rename = \"{wire}\")]");
        }
        wln!(out, "    pub {ident}: String,");
    }
    for exposure in fields {
        let field = column(table, &exposure.column)?;
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
            wln!(out, "    #[serde(default)]");
        }
        // A column named `type` is fine in a schema, fine on the wire,
        // and fine in the other three clients. Only Rust needs it
        // spelled differently, and it must still deserialize from the
        // name the contract exposes.
        let (ident, rename) = rust_ident(&snake(api));
        if let Some(wire) = rename {
            wln!(out, "    #[serde(rename = \"{wire}\")]");
        }
        wln!(out, "    pub {ident}: {ty},");
    }
    wln!(out, "}}\n");
    wln!(out, "#[derive(Debug, Clone, Deserialize)]");
    wln!(out, "pub struct {name}Page {{");
    wln!(out, "    pub items: Vec<{name}>,");
    wln!(out, "    #[serde(default)]");
    wln!(out, "    pub next_cursor: Option<String>,");
    wln!(out, "}}\n");
    Ok(())
}

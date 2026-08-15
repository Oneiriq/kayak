//! Shared test helpers.
//!
//! Not a test binary itself: Cargo compiles `tests/common/mod.rs` as a
//! module for whoever declares `mod common;`, so it is included twice
//! rather than linked. That is fine for a predicate this small, and it
//! beats the alternative of two copies of the parsing that can disagree.
//!
//! `dead_code` is allowed for the same reason -- each including binary
//! compiles the whole module, so a helper only one of them needs looks
//! unused to the other.
#![allow(dead_code)]

/// Golden artifacts that may be re-blessed, by the name the CLI already
/// uses for them. `copal-files` is the odd one out: it is not a
/// generator target but a second OpenAPI golden over a different
/// fixture, and it needs a name to be nameable.
pub const BLESSABLE: &[&str] = &[
    "openapi",
    "sdl",
    "mcp",
    "client-rs",
    "client-ts",
    "client-py",
    "client-go",
    "copal-files",
];

/// Whether this run may rewrite the golden for `artifact`.
///
/// `JANUS_BLESS` takes the artifacts to re-bless, so a bless is an
/// assertion about what you meant to change rather than a blanket
/// permission:
///
/// ```text
/// JANUS_BLESS=client-go              one
/// JANUS_BLESS=client-go,openapi      several
/// JANUS_BLESS=1                      all of them (or `all`)
/// ```
///
/// Blessing everything is still the right move after a change that
/// genuinely touches every face. What it should not be is the way you
/// look at a diff -- a failing run already prints both sides, and
/// `git diff tests/golden/` shows what any bless did.
///
/// # Panics
/// On a name that is not an artifact, via [`wants`].
#[must_use]
pub fn blessed(artifact: &str) -> bool {
    wants(std::env::var("JANUS_BLESS").ok().as_deref(), artifact)
}

/// The decision itself, taken apart from the environment so it can be
/// tested without a process-global mutation racing the suite that
/// reads it for real.
///
/// # Panics
/// On a name that is not an artifact. This is the point of the
/// function: `JANUS_BLESS=golang` is a plausible thing to type, Go is
/// `client-go` here, and silently blessing nothing would read as
/// success and send you off believing a golden was updated.
#[must_use]
pub fn wants(requested: Option<&str>, artifact: &str) -> bool {
    let Some(requested) = requested.map(str::trim) else {
        return false;
    };
    if requested == "1" || requested.eq_ignore_ascii_case("all") {
        return true;
    }

    let names: Vec<&str> = requested
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    // Validate the WHOLE list, not just the name being asked about, or
    // `JANUS_BLESS=client-go,typo` would pass unremarked on the lookup
    // that happens to be for client-go.
    let unknown: Vec<&str> = names
        .iter()
        .filter(|name| !BLESSABLE.contains(name))
        .copied()
        .collect();
    assert!(
        unknown.is_empty(),
        "JANUS_BLESS names {} -- no such artifact. Valid: {}, or 1 / all",
        unknown.join(", "),
        BLESSABLE.join(", "),
    );
    names.contains(&artifact)
}

/// The artifact name for a generated filename, since the generator
/// keys its output by filename and the blessing vocabulary is the
/// target names the CLI takes.
///
/// # Panics
/// On a filename with no artifact name, which means a target was added
/// without teaching the bless gate about it.
#[must_use]
pub fn artifact_of(filename: &str) -> &'static str {
    match filename {
        "openapi.json" => "openapi",
        "schema.graphql" => "sdl",
        "mcp-tools.json" => "mcp",
        "client.rs" => "client-rs",
        "client.ts" => "client-ts",
        "client.py" => "client-py",
        "client.go" => "client-go",
        other => panic!("no artifact name for {other}; add it to tests/common"),
    }
}

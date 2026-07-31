//! SurrealDB v3.0+ reserved-name gate.
//!
//! Contract-chosen names — field renames, GraphQL overrides, action and
//! input names — must never collide with a SurrealQL keyword, operator
//! word, special parameter, or special field. Collisions are legal in
//! SurrealDB itself (idents can be escaped), but a contract that leans
//! on escaping is a contract that breaks the moment a query is written
//! by hand. The gate is deliberately conservative.
//!
//! The list is curated from the SurrealDB v3 grammar: statement and
//! clause keywords, word operators, literals, special `$` parameters
//! (checked without the sigil), and the special field names `id`,
//! `in`, and `out`.

/// Keywords, operator words, and literals reserved by SurrealQL v3.
/// Sorted, lowercase; membership checks are case-insensitive.
const RESERVED: &[&str] = &[
    "access",
    "after",
    "algorithm",
    "all",
    "allinside",
    "alter",
    "analyze",
    "analyzer",
    "and",
    "anyinside",
    "as",
    "asc",
    "ascending",
    "assert",
    "at",
    "auth",
    "bearer",
    "before",
    "begin",
    "break",
    "by",
    "cancel",
    "capacity",
    "changefeed",
    "changes",
    "comment",
    "commit",
    "contains",
    "containsall",
    "containsany",
    "containsnone",
    "containsnot",
    "content",
    "continue",
    "create",
    "database",
    "default",
    "define",
    "delete",
    "desc",
    "descending",
    "diff",
    "dimension",
    "drop",
    "duplicate",
    "else",
    "end",
    "enforced",
    "event",
    "exists",
    "explain",
    "false",
    "fetch",
    "field",
    "fields",
    "flexible",
    "for",
    "from",
    "full",
    "function",
    "group",
    "id",
    "if",
    "ignore",
    "in",
    "index",
    "info",
    "input",
    "insert",
    "inside",
    "intersects",
    "into",
    "is",
    "key",
    "kill",
    "let",
    "limit",
    "live",
    "lowercase",
    "merge",
    "model",
    "namespace",
    "noindex",
    "none",
    "not",
    "notinside",
    "null",
    "omit",
    "on",
    "only",
    "option",
    "or",
    "order",
    "out",
    "outside",
    "overwrite",
    "parallel",
    "param",
    "parent",
    "passhash",
    "password",
    "patch",
    "permissions",
    "readonly",
    "rebuild",
    "relate",
    "relation",
    "remove",
    "return",
    "roles",
    "schemafull",
    "schemaless",
    "scope",
    "select",
    "session",
    "set",
    "show",
    "signin",
    "signup",
    "sleep",
    "split",
    "start",
    "structure",
    "table",
    "tables",
    "then",
    "this",
    "throw",
    "timeout",
    "to",
    "token",
    "tokenizers",
    "transaction",
    "true",
    "type",
    "unique",
    "unset",
    "update",
    "uppercase",
    "upsert",
    "use",
    "user",
    "value",
    "values",
    "version",
    "when",
    "where",
    "with",
];

/// Whether `name` collides with a SurrealDB v3 reserved name.
/// Case-insensitive; `$`-prefixed input is checked without the sigil.
pub fn is_reserved(name: &str) -> bool {
    let bare = name.strip_prefix('$').unwrap_or(name);
    let lowered = bare.to_ascii_lowercase();
    RESERVED.binary_search(&lowered.as_str()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_sorted_for_binary_search() {
        let mut sorted = RESERVED.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, RESERVED, "RESERVED must stay sorted and unique");
    }

    #[test]
    fn keywords_and_specials_are_caught() {
        for name in [
            "select", "SELECT", "Value", "id", "in", "out", "$auth", "order",
        ] {
            assert!(is_reserved(name), "{name} should be reserved");
        }
    }

    #[test]
    fn ordinary_names_pass() {
        for name in ["path", "state", "digest", "created_at", "tenant_id", "size"] {
            assert!(!is_reserved(name), "{name} should not be reserved");
        }
    }
}

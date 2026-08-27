//! Naming conventions shared by every generator, so `files` becomes
//! `File`/`FilesPage`/`filesList`/`list_files` identically everywhere.

/// `files` -> `File`; `file-versions` -> `FileVersion`;
/// `deliveries` -> `Delivery`.
pub(crate) fn type_name(resource: &str) -> String {
    pascal(&singular(resource))
}

/// `file_version` -> `FileVersion`.
pub(crate) fn pascal(name: &str) -> String {
    name.split(['-', '_'])
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// `created_at` -> `createdAt`.
pub(crate) fn camel(name: &str) -> String {
    let mut parts = name.split(['-', '_']);
    let mut out = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        out.push_str(&pascal(part));
    }
    out
}

/// `file-versions` -> `file_versions`.
pub(crate) fn snake(name: &str) -> String {
    name.replace('-', "_")
}

/// The singular form generators hang instance operations on.
/// A small English heuristic, covering the plural forms API resources
/// actually take. It is not meant to be a full inflector: anything it
/// gets wrong is fixed by naming the type explicitly through the
/// `graphql` overrides, which is a better answer than a word list that
/// drifts.
pub(crate) fn singular(resource: &str) -> String {
    for suffix in ["sses", "shes", "ches", "xes", "zes"] {
        if let Some(stem) = resource.strip_suffix("es") {
            if resource.ends_with(suffix) {
                return stem.to_owned();
            }
        }
    }
    // `deliveries` -> `delivery`, but `ties` -> `tie`: the `y` only
    // comes back when something precedes it, which is what separates a
    // real `-ies` plural from a short word that merely ends that way.
    if let Some(stem) = resource.strip_suffix("ies") {
        if stem.len() >= 2 {
            return format!("{stem}y");
        }
    }
    resource.strip_suffix('s').unwrap_or(resource).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_derive_consistently() {
        assert_eq!(type_name("files"), "File");
        assert_eq!(type_name("file-versions"), "FileVersion");
        assert_eq!(camel("created_at"), "createdAt");
        assert_eq!(camel("issue_url"), "issueUrl");
        assert_eq!(snake("file-versions"), "file_versions");
        assert_eq!(singular("files"), "file");
        // Plurals whose singular is more than a dropped `s`.
        assert_eq!(singular("deliveries"), "delivery");
        assert_eq!(type_name("deliveries"), "Delivery");
        assert_eq!(singular("addresses"), "address");
        assert_eq!(singular("batches"), "batch");
        // Short words ending in `ies` keep their own shape.
        assert_eq!(singular("ties"), "tie");
        // Already singular, and words a naive rule would mangle.
        assert_eq!(singular("usage"), "usage");
    }
}

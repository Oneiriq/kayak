//! Naming conventions shared by every generator, so `files` becomes
//! `File`/`FilesPage`/`filesList`/`list_files` identically everywhere.

/// `files` -> `File`; `file-versions` -> `FileVersion`.
pub(crate) fn type_name(resource: &str) -> String {
    pascal(resource.strip_suffix('s').unwrap_or(resource))
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
pub(crate) fn singular(resource: &str) -> &str {
    resource.strip_suffix('s').unwrap_or(resource)
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
    }
}

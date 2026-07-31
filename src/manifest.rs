//! Loads and validates an `elamite.toml` package manifest, per `ROADMAP.md`
//! Milestone 1 and `SPEC.md` §2.3.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml::Spanned;

use crate::diagnostics::{Category, Diagnostic};
use crate::ident::is_valid_identifier;
use crate::source::{FileId, SourceManager, Span};

/// `SPEC.md` §2.3: "Target kind is either `lib` or `exe`."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Library,
    Executable,
}

impl TargetKind {
    fn default_root_file_name(self) -> &'static str {
        match self {
            TargetKind::Library => "lib.elx",
            TargetKind::Executable => "main.elx",
        }
    }
}

/// One `[dependencies]` entry.
///
/// The initial resolver supports only local path dependencies. `SPEC.md` §2.3
/// makes that model normative: a richer registry, Git, version-selecting, or
/// lockfile-aware resolver may be introduced later without changing the
/// resolved graph consumed by semantic analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyDecl {
    pub alias: String,
    /// Resolved relative to the manifest's directory.
    pub path: PathBuf,
}

/// A validated `elamite.toml` manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub target_kind: TargetKind,
    /// Path to the root `.elx` file, relative to the manifest's directory.
    pub root: PathBuf,
    pub dependencies: Vec<DependencyDecl>,
    pub include_paths: Vec<PathBuf>,
    pub library_paths: Vec<PathBuf>,
    pub native_libraries: Vec<String>,
    pub link_options: Vec<String>,
    /// Preferred source formatting width from `[format].line_length`.
    pub format_line_length: usize,
}

#[derive(Debug, Deserialize)]
struct RawManifest {
    package: RawPackage,
    #[serde(default)]
    dependencies: BTreeMap<Spanned<String>, RawDependency>,
    #[serde(default)]
    native: RawNative,
    #[serde(default)]
    format: RawFormat,
}

#[derive(Debug, Deserialize)]
struct RawPackage {
    name: Spanned<String>,
    version: Spanned<String>,
    target_kind: Spanned<String>,
    root: Option<Spanned<String>>,
}

#[derive(Debug, Deserialize)]
struct RawDependency {
    path: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawNative {
    #[serde(default)]
    include_paths: Vec<PathBuf>,
    #[serde(default)]
    library_paths: Vec<PathBuf>,
    #[serde(default)]
    libraries: Vec<String>,
    #[serde(default)]
    link_options: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawFormat {
    line_length: Option<Spanned<usize>>,
}

/// Builds a diagnostic whose primary span is `spanned`'s location within
/// `file`, per the `codespan-reporting` retrofit in `LEDGER.md` §18.
fn spanned_diagnostic<T>(
    file: FileId,
    spanned: &Spanned<T>,
    message: impl Into<String>,
) -> Diagnostic {
    let range = spanned.span();
    Diagnostic::new(Category::ManifestInvalid, message).with_primary(Span::new(
        file,
        u32::try_from(range.start).unwrap_or(u32::MAX),
        u32::try_from(range.end).unwrap_or(u32::MAX),
    ))
}

impl Manifest {
    /// Reads, registers, and validates the manifest at `path`.
    pub fn load(path: &Path, sources: &mut SourceManager) -> Result<Manifest, Vec<Diagnostic>> {
        let file = sources.load_file(path).map_err(|error| {
            vec![Diagnostic::new(
                Category::ManifestInvalid,
                error.to_string(),
            )]
        })?;
        Self::parse(file, sources)
    }

    /// Parses and validates a manifest already registered with `sources`.
    /// Split from [`Manifest::load`] so tests can exercise validation without
    /// touching the filesystem (`sources.add_text` registers text directly).
    ///
    /// Every validation failure is collected rather than stopping at the
    /// first one, per `ROADMAP.md` §2.3's "continue after locally recoverable
    /// errors" rule.
    pub fn parse(file: FileId, sources: &SourceManager) -> Result<Manifest, Vec<Diagnostic>> {
        let text = sources.text(file);
        let raw: RawManifest = toml::from_str(text).map_err(|error| {
            // `error.message()` is the short description alone; the full
            // `Display` impl also embeds its own source excerpt, which would
            // duplicate the span-based rendering `codespan-reporting`
            // already does for `diagnostic.primary` (`LEDGER.md` §18).
            let mut diagnostic = Diagnostic::new(
                Category::ManifestInvalid,
                format!("malformed manifest: {}", error.message()),
            );
            if let Some(range) = error.span() {
                diagnostic = diagnostic.with_primary(Span::new(
                    file,
                    u32::try_from(range.start).unwrap_or(u32::MAX),
                    u32::try_from(range.end).unwrap_or(u32::MAX),
                ));
            }
            vec![diagnostic]
        })?;

        let mut diagnostics = Vec::new();
        let manifest_dir = sources
            .path(file)
            .parent()
            .unwrap_or_else(|| Path::new("."));

        if raw.package.name.get_ref().trim().is_empty() {
            diagnostics.push(spanned_diagnostic(
                file,
                &raw.package.name,
                "package name must not be empty",
            ));
        }
        if raw.package.version.get_ref().trim().is_empty() {
            diagnostics.push(spanned_diagnostic(
                file,
                &raw.package.version,
                "package version must not be empty",
            ));
        }

        let target_kind = match raw.package.target_kind.get_ref().as_str() {
            "lib" => Some(TargetKind::Library),
            "exe" => Some(TargetKind::Executable),
            other => {
                diagnostics.push(spanned_diagnostic(
                    file,
                    &raw.package.target_kind,
                    format!("target kind must be \"lib\" or \"exe\", found \"{other}\""),
                ));
                None
            }
        };

        let mut dependencies = Vec::new();
        for (alias, dependency) in &raw.dependencies {
            if !is_valid_identifier(alias.get_ref()) {
                diagnostics.push(spanned_diagnostic(
                    file,
                    alias,
                    format!(
                        "dependency alias \"{}\" is not a valid identifier",
                        alias.get_ref()
                    ),
                ));
                continue;
            }
            dependencies.push(DependencyDecl {
                alias: alias.get_ref().clone(),
                path: manifest_dir.join(&dependency.path),
            });
        }

        // Root resolution needs `target_kind` for its default file name, so
        // it waits until the loop above has finished collecting every other
        // diagnostic first.
        let root = match (&raw.package.root, target_kind) {
            (Some(root), _) => PathBuf::from(root.get_ref()),
            (None, Some(kind)) => Path::new("src").join(kind.default_root_file_name()),
            (None, None) => PathBuf::new(),
        };
        if !root.as_os_str().is_empty()
            && root.extension().and_then(|ext| ext.to_str()) != Some("elx")
        {
            let message = format!(
                "root source file {} must have an `.elx` extension",
                root.display()
            );
            diagnostics.push(match &raw.package.root {
                Some(root) => spanned_diagnostic(file, root, message),
                None => Diagnostic::new(Category::ManifestInvalid, message),
            });
        }
        let format_line_length = raw
            .format
            .line_length
            .as_ref()
            .map_or(crate::formatter::DEFAULT_LINE_LENGTH, |length| {
                *length.get_ref()
            });
        if let Some(line_length) = &raw.format.line_length
            && *line_length.get_ref() == 0
        {
            diagnostics.push(spanned_diagnostic(
                file,
                line_length,
                "format line length must be greater than zero",
            ));
        }

        let Some(target_kind) = target_kind else {
            return Err(diagnostics);
        };
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }

        Ok(Manifest {
            name: raw.package.name.into_inner(),
            version: raw.package.version.into_inner(),
            target_kind,
            root,
            dependencies,
            include_paths: raw.native.include_paths,
            library_paths: raw.native.library_paths,
            native_libraries: raw.native.libraries,
            link_options: raw.native.link_options,
            format_line_length,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Manifest, Vec<Diagnostic>> {
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("elamite.toml"), text.to_string());
        Manifest::parse(file, &sources)
    }

    #[test]
    fn parses_a_minimal_executable_manifest() {
        let manifest = parse(
            r#"
            [package]
            name = "demo"
            version = "0.1.0"
            target_kind = "exe"
            "#,
        )
        .expect("manifest should parse");
        assert_eq!(manifest.name, "demo");
        assert_eq!(manifest.target_kind, TargetKind::Executable);
        assert_eq!(manifest.root, PathBuf::from("src/main.elx"));
        assert_eq!(
            manifest.format_line_length,
            crate::formatter::DEFAULT_LINE_LENGTH
        );
    }

    #[test]
    fn parses_and_validates_the_format_line_length() {
        let manifest = parse(
            r#"
            [package]
            name = "demo"
            version = "0.1.0"
            target_kind = "exe"

            [format]
            line_length = 88
            "#,
        )
        .expect("format settings should parse");
        assert_eq!(manifest.format_line_length, 88);

        let diagnostics = parse(
            r#"
            [package]
            name = "demo"
            version = "0.1.0"
            target_kind = "exe"

            [format]
            line_length = 0
            "#,
        )
        .expect_err("zero line length should be rejected");
        assert!(diagnostics.iter().any(|diagnostic| diagnostic.category
            == Category::ManifestInvalid
            && diagnostic.message.contains("greater than zero")
            && diagnostic.primary.is_some()));
    }

    #[test]
    fn defaults_library_root() {
        let manifest = parse(
            r#"
            [package]
            name = "demo"
            version = "0.1.0"
            target_kind = "lib"
            "#,
        )
        .expect("manifest should parse");
        assert_eq!(manifest.root, PathBuf::from("src/lib.elx"));
    }

    #[test]
    fn honors_a_custom_root() {
        let manifest = parse(
            r#"
            [package]
            name = "demo"
            version = "0.1.0"
            target_kind = "exe"
            root = "start.elx"
            "#,
        )
        .expect("manifest should parse");
        assert_eq!(manifest.root, PathBuf::from("start.elx"));
    }

    #[test]
    fn rejects_empty_name() {
        let diagnostics = parse(
            r#"
            [package]
            name = ""
            version = "0.1.0"
            target_kind = "exe"
            "#,
        )
        .expect_err("empty name should be rejected");
        assert!(diagnostics.iter().any(|d| d.message.contains("name")));
        assert!(diagnostics.iter().any(|d| d.primary.is_some()));
    }

    #[test]
    fn rejects_unknown_target_kind() {
        let diagnostics = parse(
            r#"
            [package]
            name = "demo"
            version = "0.1.0"
            target_kind = "daemon"
            "#,
        )
        .expect_err("unknown target kind should be rejected");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("target kind"))
        );
        assert!(diagnostics.iter().any(|d| d.primary.is_some()));
    }

    #[test]
    fn rejects_legacy_target_kind_spellings() {
        for target_kind in ["executable", "library"] {
            let diagnostics = parse(&format!(
                "[package]\n\
                 name = \"demo\"\n\
                 version = \"0.1.0\"\n\
                 target_kind = \"{target_kind}\"\n"
            ))
            .expect_err("legacy target kind should be rejected");
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.message.contains("\"lib\" or \"exe\""))
            );
        }
    }

    #[test]
    fn rejects_root_without_elx_extension() {
        let diagnostics = parse(
            r#"
            [package]
            name = "demo"
            version = "0.1.0"
            target_kind = "exe"
            root = "start.txt"
            "#,
        )
        .expect_err("non-.elx root should be rejected");
        assert!(diagnostics.iter().any(|d| d.message.contains(".elx")));
        assert!(diagnostics.iter().any(|d| d.primary.is_some()));
    }

    #[test]
    fn rejects_invalid_dependency_alias() {
        let diagnostics = parse(
            r#"
            [package]
            name = "demo"
            version = "0.1.0"
            target_kind = "exe"

            [dependencies."not-valid"]
            path = "../other"
            "#,
        )
        .expect_err("invalid dependency alias should be rejected");
        assert!(diagnostics.iter().any(|d| d.message.contains("alias")));
        assert!(diagnostics.iter().any(|d| d.primary.is_some()));
    }

    #[test]
    fn parses_a_path_dependency() {
        let manifest = parse(
            r#"
            [package]
            name = "demo"
            version = "0.1.0"
            target_kind = "exe"

            [dependencies.other]
            path = "../other"
            "#,
        )
        .expect("manifest should parse");
        assert_eq!(manifest.dependencies.len(), 1);
        assert_eq!(manifest.dependencies[0].alias, "other");
    }

    #[test]
    fn parses_native_libraries_and_link_options() {
        let manifest = parse(
            r#"
            [package]
            name = "demo"
            version = "0.1.0"
            target_kind = "exe"

            [native]
            include_paths = ["native/include"]
            library_paths = ["native/lib"]
            libraries = ["gc", "m"]
            link_options = ["-pthread"]
            "#,
        )
        .expect("manifest should parse");
        assert_eq!(manifest.include_paths, [PathBuf::from("native/include")]);
        assert_eq!(manifest.library_paths, [PathBuf::from("native/lib")]);
        assert_eq!(manifest.native_libraries, ["gc", "m"]);
        assert_eq!(manifest.link_options, ["-pthread"]);
    }

    #[test]
    fn rejects_malformed_toml() {
        let diagnostics = parse("this is not [ valid toml").expect_err("should reject");
        assert!(
            diagnostics
                .iter()
                .all(|d| d.category == Category::ManifestInvalid)
        );
    }
}

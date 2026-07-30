//! Package scaffolding used by the `elamc init` command.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::manifest::TargetKind;

const EXECUTABLE_SOURCE: &str = "fn main() -> ():\n    println(\"Hello, Elamite!\")\n";
const LIBRARY_SOURCE: &str = "pub fn hello() -> ():\n    println(\"Hello, Elamite!\")\n";

/// Files created for a new executable package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializedPackage {
    pub directory: PathBuf,
    pub manifest: PathBuf,
    pub root_source: PathBuf,
    pub target_kind: TargetKind,
}

/// A recoverable failure while creating a package skeleton.
#[derive(Debug)]
pub enum InitError {
    MissingPackageName(PathBuf),
    InvalidPackageName,
    AlreadyExists(PathBuf),
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for InitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPackageName(path) => write!(
                formatter,
                "cannot infer a package name from {}; pass `--name NAME`",
                path.display()
            ),
            Self::InvalidPackageName => formatter.write_str("package name must not be empty"),
            Self::AlreadyExists(path) => write!(
                formatter,
                "refusing to overwrite existing package file {}",
                path.display()
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "cannot {action} {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Creates a minimal executable or library package without replacing existing
/// package files. Existing destination directories are allowed so an empty
/// directory can be initialized in place.
pub fn init_package(
    destination: &Path,
    requested_name: Option<&str>,
    target_kind: TargetKind,
) -> Result<InitializedPackage, InitError> {
    let name = match requested_name {
        Some(name) => name.to_string(),
        None => {
            let name_path = if destination.file_name().is_some() {
                destination.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|source| InitError::Io {
                        action: "resolve",
                        path: destination.to_path_buf(),
                        source,
                    })?
                    .join(destination)
            };
            name_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
                .ok_or_else(|| InitError::MissingPackageName(destination.to_path_buf()))?
        }
    };
    if name.trim().is_empty() {
        return Err(InitError::InvalidPackageName);
    }

    let (target_kind_name, root_file_name, root_source_text) = match target_kind {
        TargetKind::Executable => ("exe", "main.elx", EXECUTABLE_SOURCE),
        TargetKind::Library => ("lib", "lib.elx", LIBRARY_SOURCE),
    };
    let manifest = destination.join("elamite.toml");
    let root_source = destination.join("src").join(root_file_name);
    for path in [&manifest, &root_source] {
        if path.exists() {
            return Err(InitError::AlreadyExists(path.clone()));
        }
    }

    let source_directory = destination.join("src");
    fs::create_dir_all(&source_directory).map_err(|source| InitError::Io {
        action: "create directory",
        path: source_directory,
        source,
    })?;

    let quoted_name = toml::Value::String(name).to_string();
    let manifest_source = format!(
        "[package]\n\
         name = {quoted_name}\n\
         version = \"0.1.0\"\n\
         target_kind = \"{target_kind_name}\"\n"
    );
    write_new(&root_source, root_source_text)?;
    if let Err(error) = write_new(&manifest, &manifest_source) {
        let _ = fs::remove_file(&root_source);
        return Err(error);
    }

    Ok(InitializedPackage {
        directory: destination.to_path_buf(),
        manifest,
        root_source,
        target_kind,
    })
}

fn write_new(path: &Path, contents: &str) -> Result<(), InitError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                InitError::AlreadyExists(path.to_path_buf())
            } else {
                InitError::Io {
                    action: "write",
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
    if let Err(source) = file.write_all(contents.as_bytes()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(InitError::Io {
            action: "write",
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, TargetKind};
    use crate::source::SourceManager;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temporary_directory(name: &str) -> PathBuf {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "elamite-scaffold-{}-{name}-{serial}",
            std::process::id()
        ))
    }

    #[test]
    fn creates_a_valid_hello_world_package() {
        let directory = temporary_directory("hello");
        let initialized = init_package(&directory, Some("hello"), TargetKind::Executable)
            .expect("package should initialize");
        let mut sources = SourceManager::new();
        let manifest = Manifest::load(&initialized.manifest, &mut sources)
            .expect("generated manifest should load");

        assert_eq!(manifest.name, "hello");
        assert_eq!(manifest.target_kind, TargetKind::Executable);
        assert_eq!(
            fs::read_to_string(&initialized.root_source).expect("read generated root"),
            EXECUTABLE_SOURCE
        );
        fs::remove_dir_all(directory).expect("remove test package");
    }

    #[test]
    fn creates_a_valid_library_package() {
        let directory = temporary_directory("library");
        let initialized = init_package(&directory, Some("hello_lib"), TargetKind::Library)
            .expect("library should initialize");
        let mut sources = SourceManager::new();
        let manifest = Manifest::load(&initialized.manifest, &mut sources)
            .expect("generated manifest should load");

        assert_eq!(manifest.name, "hello_lib");
        assert_eq!(manifest.target_kind, TargetKind::Library);
        assert_eq!(initialized.root_source, directory.join("src/lib.elx"));
        assert_eq!(
            fs::read_to_string(&initialized.root_source).expect("read generated root"),
            LIBRARY_SOURCE
        );
        fs::remove_dir_all(directory).expect("remove test package");
    }

    #[test]
    fn refuses_to_replace_an_existing_manifest() {
        let directory = temporary_directory("existing");
        fs::create_dir_all(&directory).expect("create test directory");
        let manifest = directory.join("elamite.toml");
        fs::write(&manifest, "keep me").expect("write existing manifest");

        let error = init_package(&directory, Some("demo"), TargetKind::Executable)
            .expect_err("must not overwrite");
        assert!(matches!(error, InitError::AlreadyExists(path) if path == manifest));
        assert_eq!(
            fs::read_to_string(&manifest).expect("read existing manifest"),
            "keep me"
        );
        fs::remove_dir_all(directory).expect("remove test package");
    }
}

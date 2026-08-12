use std::fs;
use std::path::{Path, PathBuf};

fn rust_sources(root: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            rust_sources(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}

#[test]
fn compiler_sources_contain_no_collector_or_root_hook() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    sources.sort();

    let forbidden = [
        "<gc.h>",
        "GC_INIT",
        "GC_MALLOC",
        "GC_add_roots",
        "GC_remove_roots",
        "GC_reachable_here",
        "ManagedMemoryStrategy",
        "requires_managed_memory",
    ];
    for path in sources {
        let source = fs::read_to_string(&path).expect("read Rust source");
        for hook in forbidden {
            assert!(
                !source.contains(hook),
                "{} reintroduced forbidden collector hook `{hook}`",
                path.display()
            );
        }
    }
}

#[test]
fn process_lifetime_runtime_view_allocations_stay_in_one_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    sources.sort();

    let mut sites = Vec::new();
    for path in sources {
        let source = fs::read_to_string(&path).expect("read Rust source");
        if source.contains("el_process_alloc") {
            sites.push(
                path.strip_prefix(&root)
                    .expect("source is under root")
                    .to_owned(),
            );
        }
    }
    assert_eq!(
        sites,
        [PathBuf::from("backend/runtime.rs")],
        "new process-lifetime allocation sites require an explicit inventory decision"
    );
}

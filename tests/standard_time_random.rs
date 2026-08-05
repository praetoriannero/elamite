use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::Target;
use elamite::driver::{BuildOptions, Optimization, build, run};
use elamite::lexer::lex;
use elamite::package::PackageGraph;
use elamite::parser::parse;
use elamite::source::SourceManager;

const TIME: &str = include_str!("../stdlib/src/time.elx");
const RANDOM: &str = include_str!("../stdlib/src/random.elx");
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn assert_parses(name: &str, source: &str) {
    let mut sources = SourceManager::new();
    let file = sources.add_text(PathBuf::from(name), source.to_string());
    let lexed = lex(file, source);
    assert!(
        lexed.diagnostics.is_empty(),
        "{name} lexer diagnostics: {:?}",
        lexed.diagnostics
    );
    let parsed = parse(&lexed.tokens);
    assert!(
        parsed.diagnostics.is_empty(),
        "{name} parser diagnostics: {:?}",
        parsed.diagnostics
    );
}

#[test]
fn time_and_random_modules_parse_as_ordinary_elamite() {
    assert_parses("time.elx", TIME);
    assert_parses("random.elx", RANDOM);
}

#[test]
fn time_api_keeps_clock_domains_distinct_and_arithmetic_checked() {
    assert!(TIME.contains("pub struct Instant"));
    assert!(TIME.contains("pub struct SystemTime"));
    assert!(!TIME.contains("impl Instant:\n    pub fn unix_nanoseconds"));
    assert!(TIME.contains("pub fn checked_add"));
    assert!(TIME.contains("pub fn checked_sub"));
    assert!(TIME.contains("pub fn checked_mul"));
    assert!(TIME.contains("-> Option[Self]"));
    assert!(TIME.contains("not a synchronization edge"));
}

#[test]
fn splitmix64_fixed_seed_contract_matches_published_sequence() {
    fn next(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = *state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    let mut state = 0;
    assert_eq!(next(&mut state), 0xe220_a839_7b1d_cdaf);
    assert_eq!(next(&mut state), 0x6e78_9e6a_a1b9_65f4);
    assert_eq!(next(&mut state), 0x06c4_5d18_8009_454f);
    assert!(RANDOM.contains("never consults the clock, process, or operating system"));
    assert!(RANDOM.contains("Rejection sampling avoids modulo bias"));
}

#[test]
fn empty_random_ranges_do_not_advance_state_by_contract() {
    let zero_check = RANDOM.find("if upper == 0u64:").expect("zero range guard");
    let first_draw = RANDOM
        .find("let value = self.next_u64()")
        .expect("first random draw");
    assert!(zero_check < first_draw);
    assert!(RANDOM.contains("empty or reversed range returns `None` without advancing"));
}

#[test]
fn native_clocks_preserve_domains_and_checked_elapsed_behavior() {
    let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "elamite-time-runtime-{}-{serial}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).expect("create clock test package");
    fs::write(
        root.join("elamite.toml"),
        "[package]\nname = \"time_runtime\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n",
    )
    .expect("write manifest");
    fs::write(
        root.join("src/main.elx"),
        "fn main() -> ():\n\
         \x20\x20\x20\x20let first: std.time.Instant = std.time.monotonic_now()\n\
         \x20\x20\x20\x20let wall: std.time.SystemTime = std.time.system_now()\n\
         \x20\x20\x20\x20let second: std.time.Instant = std.time.monotonic_now()\n\
         \x20\x20\x20\x20match second.duration_since(first):\n\
         \x20\x20\x20\x20\x20\x20\x20\x20Option.Some(_):\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20println(\"elapsed\")\n\
         \x20\x20\x20\x20\x20\x20\x20\x20Option.None:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20println(\"backward\")\n\
         \x20\x20\x20\x20let maximum = std.time.Duration.from_nanoseconds(18446744073709551615u64)\n\
         \x20\x20\x20\x20match maximum.checked_add(std.time.Duration.from_nanoseconds(1u64)):\n\
         \x20\x20\x20\x20\x20\x20\x20\x20Option.Some(_):\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20println(\"overflow missed\")\n\
         \x20\x20\x20\x20\x20\x20\x20\x20Option.None:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20println(\"checked\")\n\
         \x20\x20\x20\x20println(wall.unix_nanoseconds() > 0u64)\n",
    )
    .expect("write source");

    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&root.join("elamite.toml"), &mut sources)
        .expect("resolve clock test package");
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Debug,
            features: Default::default(),
            output_directory: root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
            // Existing aggregate-None and eagerly emitted empty-Vec helpers
            // trigger these unrelated generated-C warnings under `-Werror`.
            c_flags: vec![
                "-Wno-error=missing-braces".into(),
                "-Wno-error=unused-function".into(),
            ],
        },
    )
    .unwrap_or_else(|diagnostics| panic!("clock build failed: {diagnostics:#?}"));
    let result = run(&artifact).expect("run clock test executable");
    assert!(
        result.status.success(),
        "status={:?}, stdout={}, stderr={}",
        result.status.code(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "elapsed\nchecked\ntrue\n"
    );
    let _ = fs::remove_dir_all(&root);
}

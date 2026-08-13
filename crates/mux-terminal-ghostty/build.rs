use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRED_ZIG_VERSION: &str = "0.16.0";

fn main() {
    println!("cargo:rerun-if-changed=native/ghostty_shim.c");
    println!("cargo:rerun-if-changed=../../native/ghostty/build.zig");
    println!("cargo:rerun-if-changed=../../native/ghostty/build.zig.zon");
    println!("cargo:rerun-if-env-changed=MUX_GHOSTTY_PREFIX");
    println!("cargo:rerun-if-env-changed=MUX_ZIG");

    if env::var_os("CARGO_FEATURE_LINK").is_none() {
        return;
    }

    let prefix = if env::var_os("CARGO_FEATURE_VENDORED").is_some() {
        build_vendored()
    } else {
        PathBuf::from(env::var_os("MUX_GHOSTTY_PREFIX").unwrap_or_else(|| {
            panic!("the `link` feature requires MUX_GHOSTTY_PREFIX or the `vendored` feature")
        }))
    };

    let include_dir = prefix.join("include");
    let library_dir = prefix.join("lib");
    assert!(
        include_dir.join("ghostty/vt.h").is_file(),
        "missing libghostty headers under {}",
        include_dir.display()
    );

    let version = format!(
        "\"{}\"",
        env::var("CARGO_PKG_VERSION").expect("Cargo package version")
    );
    cc::Build::new()
        .file("native/ghostty_shim.c")
        .include(&include_dir)
        .define("GHOSTTY_STATIC", None)
        .define("MUX_VERSION", version.as_str())
        .warnings(true)
        .extra_warnings(true)
        .compile("mux-ghostty-shim");

    println!("cargo:rustc-link-search=native={}", library_dir.display());
    println!("cargo:rustc-link-lib=dylib=ghostty-vt");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", library_dir.display());
}

fn build_vendored() -> PathBuf {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let native_dir = manifest_dir.join("../../native/ghostty");
    let prefix = PathBuf::from(env::var_os("OUT_DIR").expect("out dir")).join("ghostty");
    let zig = env::var_os("MUX_ZIG").unwrap_or_else(|| "zig".into());
    let cargo_target = env::var("TARGET").expect("Cargo target triple");

    require_zig_version(Path::new(&zig));
    let mut command = Command::new(&zig);
    command
        .current_dir(&native_dir)
        .args(["build", "install", "-Doptimize=ReleaseFast"]);
    if let Some(target) = zig_macos_target(&cargo_target) {
        command.arg(format!("-Dtarget={target}"));
    }
    let status = command
        .arg("--prefix")
        .arg(&prefix)
        .status()
        .unwrap_or_else(|error| panic!("failed to launch Zig: {error}"));
    assert!(status.success(), "vendored libghostty-vt build failed");
    prefix
}

fn zig_macos_target(cargo_target: &str) -> Option<&'static str> {
    match cargo_target {
        "aarch64-apple-darwin" => Some("aarch64-macos"),
        "x86_64-apple-darwin" => Some("x86_64-macos"),
        _ => None,
    }
}

fn require_zig_version(zig: &Path) {
    let output = Command::new(zig)
        .arg("version")
        .output()
        .unwrap_or_else(|error| panic!("failed to query Zig version: {error}"));
    assert!(output.status.success(), "`zig version` failed");
    let version = String::from_utf8_lossy(&output.stdout);
    assert!(
        version.trim() == REQUIRED_ZIG_VERSION,
        "libghostty-vt requires Zig {REQUIRED_ZIG_VERSION}; found {}. Set MUX_ZIG to the correct executable",
        version.trim()
    );
}

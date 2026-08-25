//! Compiles the libavcodec C shim when the `ffmpeg` feature is on.
//!
//! Uses std::process::Command rather than the `cc` and `pkg-config` crates:
//! this crate deliberately has no Rust dependency for ffmpeg support, and
//! pulling two build-dependencies in to invoke two programs would undo that.
//!
//! With the feature off this does nothing, so the default build neither needs
//! a C compiler nor the ffmpeg headers.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=csrc/miru_ffmpeg.c");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_FFMPEG");

    if std::env::var_os("CARGO_FEATURE_FFMPEG").is_none() {
        return;
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());

    let cflags = pkg_config("--cflags");
    let libs = pkg_config("--libs");

    let obj = out.join("miru_ffmpeg.o");
    let mut cmd = Command::new(&cc);
    cmd.args(["-c", "-O2", "-fPIC", "csrc/miru_ffmpeg.c", "-o"])
        .arg(&obj)
        .args(&cflags);
    run(&mut cmd, &cc);

    let lib = out.join("libmiru_ffmpeg.a");
    let _ = std::fs::remove_file(&lib); // ar appends; stale members would linger
    run(Command::new("ar").arg("rcs").arg(&lib).arg(&obj), "ar");

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=miru_ffmpeg");
    // -L/-l from pkg-config, translated into cargo directives.
    for l in libs {
        if let Some(p) = l.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={p}");
        } else if let Some(n) = l.strip_prefix("-l") {
            println!("cargo:rustc-link-lib=dylib={n}");
        }
    }
}

fn pkg_config(what: &str) -> Vec<String> {
    let out = Command::new("pkg-config")
        .args([what, "libavcodec", "libavutil"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        // Fall back to bare linkage: works wherever the libraries are on the
        // default search path, which covers most Linux and Homebrew setups.
        _ => {
            if what == "--libs" {
                vec!["-lavcodec".into(), "-lavutil".into()]
            } else {
                Vec::new()
            }
        }
    }
}

fn run(cmd: &mut Command, what: &str) {
    match cmd.status() {
        Ok(s) if s.success() => {}
        Ok(s) => panic!(
            "{what} failed ({s}) building the ffmpeg shim. \
             Install the ffmpeg dev libraries (apt: libavcodec-dev libavutil-dev), \
             or build without --features ffmpeg."
        ),
        Err(e) => panic!(
            "could not run {what}: {e}. A C compiler and the ffmpeg dev libraries \
             are needed for --features ffmpeg."
        ),
    }
}

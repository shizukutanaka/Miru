//! Compiles the xdg-desktop-portal / PipeWire C shim when the `pipewire`
//! feature is on. Mirrors crates/miru-codec/build.rs — see that file for why
//! this shells out rather than taking `cc` and `pkg-config` as crates.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=csrc/miru_portal.c");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_PIPEWIRE");

    if std::env::var_os("CARGO_FEATURE_PIPEWIRE").is_none() {
        return;
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let pkgs = ["gio-2.0", "glib-2.0", "gobject-2.0", "libpipewire-0.3"];

    let obj = out.join("miru_portal.o");
    let mut cmd = Command::new(&cc);
    cmd.args(["-c", "-O2", "-fPIC", "csrc/miru_portal.c", "-o"])
        .arg(&obj)
        .args(pkg_config("--cflags", &pkgs));
    run(&mut cmd, &cc);

    let lib = out.join("libmiru_portal.a");
    let _ = std::fs::remove_file(&lib);
    run(Command::new("ar").arg("rcs").arg(&lib).arg(&obj), "ar");

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=miru_portal");
    for l in pkg_config("--libs", &pkgs) {
        if let Some(p) = l.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={p}");
        } else if let Some(n) = l.strip_prefix("-l") {
            println!("cargo:rustc-link-lib=dylib={n}");
        }
    }
}

fn pkg_config(what: &str, pkgs: &[&str]) -> Vec<String> {
    match Command::new("pkg-config").arg(what).args(pkgs).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        _ => panic!(
            "pkg-config could not resolve {pkgs:?}. Install the dev packages \
             (apt: libglib2.0-dev libpipewire-0.3-dev), or build without \
             --features pipewire."
        ),
    }
}

fn run(cmd: &mut Command, what: &str) {
    match cmd.status() {
        Ok(s) if s.success() => {}
        Ok(s) => panic!("{what} failed ({s}) building the portal shim"),
        Err(e) => panic!("could not run {what}: {e}"),
    }
}

use std::env;
use std::path::PathBuf;
use std::process::{Command, Output};

fn run(command: &mut Command) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|e| panic!("failed to start {command:?}: {e}"));
    assert!(
        output.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn flags(option: &str) -> Vec<String> {
    let pkg_config = env::var_os("PKG_CONFIG").unwrap_or_else(|| "pkg-config".into());
    let output =
        run(Command::new(pkg_config).args([option, "gstreamer-app-1.0", "gstreamer-video-1.0"]));
    String::from_utf8(output.stdout)
        .expect("pkg-config output must be UTF-8")
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

pub fn build() {
    println!("cargo:rerun-if-changed=native/build_media.rs");
    println!("cargo:rerun-if-changed=native/media.c");
    for var in [
        "CC",
        "AR",
        "PKG_CONFIG",
        "PKG_CONFIG_PATH",
        "PKG_CONFIG_LIBDIR",
        "PKG_CONFIG_SYSROOT_DIR",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let object = out.join("media.o");
    run(
        Command::new(env::var_os("CC").unwrap_or_else(|| "cc".into()))
            .args(["-std=c11", "-Wall", "-Wextra", "-O2", "-fPIC"])
            .args(flags("--cflags"))
            .args(["-c", "native/media.c", "-o"])
            .arg(&object),
    );
    run(
        Command::new(env::var_os("AR").unwrap_or_else(|| "ar".into()))
            .arg("crs")
            .arg(out.join("libcinema_media.a"))
            .arg(object),
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=cinema_media");
    for flag in flags("--libs") {
        if let Some(directory) = flag.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={directory}");
        } else if let Some(library) = flag.strip_prefix("-l") {
            println!("cargo:rustc-link-lib={library}");
        } else {
            println!("cargo:rustc-link-arg={flag}");
        }
    }
}

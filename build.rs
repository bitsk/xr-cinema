use std::{env, io::ErrorKind, path::PathBuf, process::Command};

#[path = "native/build_media.rs"]
mod build_media;

fn main() {
    build_media::build();
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for shader in ["cinema.vert", "cinema.frag", "hands.vert", "hands.frag"] {
        let source = PathBuf::from("shaders").join(shader);
        let binary = out.join(format!("{shader}.spv"));
        println!("cargo:rerun-if-changed={}", source.display());
        let result = Command::new("glslc")
            .args(["--target-env=vulkan1.0", "-O"])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .status();
        let status = match result {
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::PermissionDenied) => {
                Command::new("glslangValidator")
                    .args(["-V", "--target-env", "vulkan1.0"])
                    .arg(source)
                    .arg("-o")
                    .arg(binary)
                    .status()
                    .expect("install glslc or glslang-tools to compile the cinema shaders")
            }
            result => result.expect("could not launch glslc"),
        };
        assert!(status.success(), "shader compilation failed for {shader}");
    }
}

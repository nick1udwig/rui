use std::{env, path::PathBuf, process::Command};

fn output(command: &mut Command) -> String {
    let result = command.output().expect("native build tool unavailable");
    assert!(
        result.status.success(),
        "native build tool failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).expect("native build metadata must be UTF-8")
}
fn main() {
    println!("cargo:rerun-if-changed=native/bridge.c");
    if env::var_os("CARGO_FEATURE_NATIVE_EIS").is_none() {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_OS").unwrap(),
        "linux",
        "native-eis requires the isolated Linux backend"
    );
    for (package, version) in [
        ("libei-1.0", "1.6.0"),
        ("libeis-1.0", "1.6.0"),
        ("xkbcommon", "1.13.2"),
    ] {
        let actual = output(Command::new("pkg-config").args(["--modversion", package]));
        assert_eq!(
            actual.trim(),
            version,
            "native-eis pinned environment requires {package}={version}"
        );
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let flags = output(Command::new("pkg-config").args([
        "--cflags",
        "libei-1.0",
        "libeis-1.0",
        "xkbcommon",
    ]));
    let mut cc = Command::new("cc");
    cc.args([
        "-std=c11",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-fPIC",
        "-c",
        "native/bridge.c",
        "-o",
    ])
    .arg(out.join("bridge.o"))
    .args(flags.split_whitespace());
    output(&mut cc);
    output(
        Command::new("ar")
            .arg("crs")
            .arg(out.join("librui_native.a"))
            .arg(out.join("bridge.o")),
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=rui_native");
    for library in ["ei", "eis", "xkbcommon"] {
        println!("cargo:rustc-link-lib={library}");
    }
}

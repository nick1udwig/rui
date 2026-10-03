fn main() {
    println!("cargo:rerun-if-changed=native/physical_x11.c");
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_none()
        || std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux")
    {
        return;
    }
    let gtk = pkg_config::Config::new()
        .probe("gtk4-x11")
        .expect("the Linux desktop physical-key adapter requires GTK X11 development headers");
    let x11 = pkg_config::Config::new()
        .probe("x11")
        .expect("the Linux desktop physical-key adapter requires X11 development headers");
    let mut build = cc::Build::new();
    build.file("native/physical_x11.c");
    for include in gtk.include_paths.into_iter().chain(x11.include_paths) {
        build.include(include);
    }
    build.warnings(true).extra_warnings(true);
    build.compile("rui_physical_x11");
}

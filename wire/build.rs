fn main() {
    println!("cargo:rerun-if-changed=../spec/session.proto");
    prost_build::Config::new()
        .skip_debug([".rui.session.v1.ClientHello", ".rui.session.v1.Packet"])
        .compile_protos(&["../spec/session.proto"], &["../spec"])
        .expect("compile transport schema");
}

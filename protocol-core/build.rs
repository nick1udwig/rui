fn main() {
    println!("cargo:rerun-if-changed=../spec/rui.proto");
    prost_build::Config::new()
        .compile_protos(&["../spec/rui.proto"], &["../spec"])
        .expect("compile RUI schema (install protoc)");
}

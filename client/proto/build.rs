fn main() {
    let proto = "../../proto/frontline.proto";
    println!("cargo:rerun-if-changed={proto}");
    prost_build::compile_protos(&[proto], &["../../proto"]).expect("compile frontline.proto (is protoc installed?)");
}

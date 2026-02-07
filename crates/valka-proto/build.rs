fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files = &[
        "../../proto/valka/v1/common.proto",
        "../../proto/valka/v1/events.proto",
        "../../proto/valka/v1/api.proto",
        "../../proto/valka/v1/worker.proto",
        "../../proto/valka/v1/internal.proto",
    ];

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let descriptor_path = out_dir.join("valka_descriptor.bin");

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .file_descriptor_set_path(&descriptor_path)
        .compile_protos(proto_files, &["../../proto"])?;

    for proto in proto_files {
        println!("cargo:rerun-if-changed={proto}");
    }

    Ok(())
}

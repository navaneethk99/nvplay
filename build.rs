#[path = "src/source_version.rs"]
mod source_version;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in source_version::INPUTS {
        println!("cargo:rerun-if-changed={path}");
    }
    println!(
        "cargo:rustc-env=NVPLAY_SOURCE_VERSION={}",
        source_version::fingerprint(root).expect("read source files")
    );
}

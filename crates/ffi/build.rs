fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cbindgen::Config::from_file(format!("{crate_dir}/cbindgen.toml")).expect("cbindgen.toml"))
        .generate()
        .expect("generate C header")
        .write_to_file(format!("{crate_dir}/include/usd_toolbox.h"));
}

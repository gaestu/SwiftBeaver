fn main() {
    println!("cargo:rerun-if-env-changed=LIBEWF_LIB_DIR");

    if cfg!(all(feature = "ewf", target_os = "windows"))
        && let Ok(lib_dir) = std::env::var("LIBEWF_LIB_DIR")
    {
        println!("cargo:rustc-link-search=native={lib_dir}");
    }
}

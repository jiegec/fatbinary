use std::env;
use std::path::PathBuf;

fn main() {
    // Tell cargo to link the static library
    println!("cargo:rustc-link-search=native=/usr/local/cuda/targets/x86_64-linux/lib/");
    println!("cargo:rustc-link-lib=static=nvfatbin_static");
    // The static library is built with C++ and depends on libstdc++, libm, libdl
    println!("cargo:rustc-link-lib=stdc++");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-lib=dl");

    // Generate bindings
    let bindings = bindgen::Builder::default()
        .header("/usr/local/cuda/targets/x86_64-linux/include/nvFatbin.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .allowlist_function("nvFatbin.*")
        .allowlist_type("nvFatbin.*")
        .allowlist_var("NVFATBIN_.*")
        .size_t_is_usize(true)
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
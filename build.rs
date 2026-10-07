use std::env;

fn main() {
    println!("cargo:rerun-if-changed=shim/shim.cpp");
    println!("cargo:rerun-if-changed=shim/shim.h");
    println!("cargo:rerun-if-changed=shim/hooks.cpp");
    println!("cargo:rerun-if-changed=shim/hooks.h");
    println!("cargo:rerun-if-env-changed=LEAPSDK_LIB_PATH");

    // `cargo test --features host-stubs` on the build host: no shim, no LeapC.
    if env::var("CARGO_FEATURE_HOST_STUBS").is_ok() {
        return;
    }

    cc::Build::new()
        .cpp(true)
        .file("shim/shim.cpp")
        .file("shim/hooks.cpp")
        .include("external/openvr/headers")
        .include("shim")
        .flag("/std:c++17")
        .flag("/EHs-c-") // the OpenVR header doesn't throw; no unwinding through the shim
        .define("_HAS_EXCEPTIONS", "0")
        .warnings(false)
        .compile("leapfrog_shim");

    // LeapC.lib from the Gemini install (leap-sys only declares the functions).
    let leap_lib = env::var("LEAPSDK_LIB_PATH")
        .unwrap_or_else(|_| "/mnt/e/Games/Steam/steamapps/common/Ultraleap Gemini/LeapSDK/lib/x64".to_string());
    println!("cargo:rustc-link-search=native={leap_lib}");
    println!("cargo:rustc-link-lib=dylib=LeapC");
}

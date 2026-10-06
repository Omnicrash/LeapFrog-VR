fn main() {
    println!("cargo:rerun-if-changed=shim/shim.cpp");
    println!("cargo:rerun-if-changed=shim/shim.h");
    cc::Build::new()
        .cpp(true)
        .file("shim/shim.cpp")
        .include("external/openvr/headers")
        .include("shim")
        .flag("/std:c++17")
        .flag("/EHs-c-") // the OpenVR header doesn't throw; no unwinding through the shim
        .define("_HAS_EXCEPTIONS", "0")
        .warnings(false)
        .compile("leapfrog_shim");
}

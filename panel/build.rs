fn main() {
    println!("cargo:rerun-if-changed=panel.cpp");
    println!("cargo:rerun-if-changed=panel.h");
    if std::env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }
    cc::Build::new()
        .cpp(true)
        .file("panel.cpp")
        .include("../external/openvr/headers")
        .flag("/std:c++17")
        .flag("/EHs-c-")
        .define("_HAS_EXCEPTIONS", "0")
        .warnings(false)
        .compile("leapfrog_panel");
}

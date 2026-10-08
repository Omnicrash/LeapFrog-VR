fn main() {
    for f in ["panel.cpp", "panel.h", "overlay.cpp"] {
        println!("cargo:rerun-if-changed={f}");
    }
    if std::env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }
    let imgui = "../external/imgui";
    cc::Build::new()
        .cpp(true)
        .file("panel.cpp")
        .file("overlay.cpp")
        .file(format!("{imgui}/imgui.cpp"))
        .file(format!("{imgui}/imgui_draw.cpp"))
        .file(format!("{imgui}/imgui_tables.cpp"))
        .file(format!("{imgui}/imgui_widgets.cpp"))
        .file(format!("{imgui}/backends/imgui_impl_dx11.cpp"))
        .include("../external/openvr/headers")
        .include(imgui)
        .define("LFP_VERSION", format!("\"{}\"", env!("CARGO_PKG_VERSION")).as_str())
        .flag("/std:c++17")
        .flag("/EHs-c-")
        .define("_HAS_EXCEPTIONS", "0")
        .warnings(false)
        .compile("leapfrog_panel");
    println!("cargo:rustc-link-lib=d3d11");
    println!("cargo:rustc-link-lib=d3dcompiler");
}

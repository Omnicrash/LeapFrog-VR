// The exe's icon (setup.rc -> leapfrog.ico, the hand from the status icons): llvm-rc compiles
// the resource script, lld-link takes the .res as a link input.
fn main() {
    println!("cargo:rerun-if-changed=setup.rc");
    println!("cargo:rerun-if-changed=leapfrog.ico");
    if std::env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("setup.res");
    let status = std::process::Command::new("llvm-rc")
        .args(["/FO"])
        .arg(&out)
        .arg("setup.rc")
        .status()
        .expect("llvm-rc not found (apt llvm)");
    assert!(status.success(), "llvm-rc failed");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

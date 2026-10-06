//! Regenerates the hand render models from OBJs of the earlier bar-per-bone generator (the
//! joint capture; `git show fde1adf:resources/rendermodels/...` has them):
//! `cargo run --example regen --features host-stubs --target x86_64-unknown-linux-gnu -- <bars dir> <rendermodels dir>`
//! Reads `<bars dir>/leapfrog_hand_<side>/leapfrog_hand_<side>.obj`, writes into the second dir.
use std::path::Path;

#[path = "../src/hand_model.rs"]
#[allow(dead_code)]
mod hand_model;
#[path = "../src/hand_outline.rs"]
#[allow(dead_code)]
mod hand_outline;
#[path = "../src/log.rs"]
#[allow(dead_code)]
mod log;
mod host {
    pub mod ffi {
        pub unsafe fn lfh_log(msg: *const std::ffi::c_char) {
            eprintln!("{}", std::ffi::CStr::from_ptr(msg).to_string_lossy());
        }
    }
}

fn main() {
    let bars = std::env::args().nth(1).expect("bars directory (the joint capture)");
    let out = std::env::args().nth(2).expect("rendermodels directory to write");
    for side in ["left", "right"] {
        let obj = Path::new(&bars).join(format!("leapfrog_hand_{side}")).join(format!("leapfrog_hand_{side}.obj"));
        let text = std::fs::read_to_string(&obj).expect("bar obj");
        let bones = hand_model::bones_from_bar_obj(&text);
        assert_eq!(bones.len(), 19, "{}: not a bar-per-bone capture", obj.display());
        hand_model::write(Path::new(&out), side, &bones);
    }
}

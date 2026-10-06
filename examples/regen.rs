//! Regenerates the hand render models from OBJs of the earlier bar-per-bone generator:
//! `cargo run --example regen --features host-stubs --target x86_64-unknown-linux-gnu -- <rendermodels dir>`
use std::path::Path;

#[path = "../src/hand_model.rs"]
#[allow(dead_code)]
mod hand_model;
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
    let dir = std::env::args().nth(1).expect("rendermodels directory");
    for side in ["left", "right"] {
        let obj = Path::new(&dir).join(format!("leapfrog_hand_{side}")).join(format!("leapfrog_hand_{side}.obj"));
        let text = std::fs::read_to_string(&obj).expect("old obj");
        let bones = hand_model::bones_from_bar_obj(&text);
        hand_model::write(Path::new(&dir), side, &bones);
    }
}

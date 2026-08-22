//! Keep the machine awake while the backend runs. An idle-looking Windows box
//! sleeps right through a show, and even *display* sleep can take out audio:
//! HDMI/DisplayPort display-audio devices (a common loopback beat source)
//! disappear when the monitor powers down — observed live on the dev machine.
//!
//! System sleep is always blocked while we run. Display sleep is only blocked
//! while sACN output is enabled (i.e. actually performing), so a dev machine
//! with output off keeps its normal monitor timeout.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::state::SharedState;

pub fn spawn(state: Arc<SharedState>) {
    #[cfg(windows)]
    std::thread::Builder::new()
        .name("keep-awake".into())
        .spawn(move || run(state))
        .expect("spawn keep-awake thread");
    #[cfg(not(windows))]
    let _ = state;
}

#[cfg(windows)]
fn run(state: Arc<SharedState>) {
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(es_flags: u32) -> u32;
    }

    // ES_CONTINUOUS is per-thread state, so the flags must be (re)asserted from
    // this thread; the loop also picks up output-enabled changes.
    let mut last_flags = 0u32;
    while !state.shutdown.load(Ordering::Relaxed) {
        let output_on = state.config.read().output.enabled;
        let flags = ES_CONTINUOUS
            | ES_SYSTEM_REQUIRED
            | if output_on { ES_DISPLAY_REQUIRED } else { 0 };
        if flags != last_flags {
            log::info!(
                "keep-awake: blocking system sleep{}",
                if output_on { " + display sleep (output enabled)" } else { "" }
            );
            last_flags = flags;
        }
        unsafe { SetThreadExecutionState(flags) };
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
    unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
}

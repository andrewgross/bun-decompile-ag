#![no_main]

//! The no-panic contract for [`get_executable_version`] and
//! [`inspect_executable`]: any byte input must return without panicking.

use bun_decompile_ag::{get_executable_version, inspect_executable};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = get_executable_version(data);
    // inspect never returns Err as a whole; it must still never panic.
    let _ = inspect_executable(data);
});

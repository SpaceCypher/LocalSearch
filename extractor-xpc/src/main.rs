//! LocalSearch content extractor helper process
//!
//! Receives one file path per line on stdin, extracts its text, and writes
//! the result to stdout (see `localsearch::extract::client` for the wire
//! format). It exists so that parser crashes or hangs (PDFKit, the Word and
//! RTF importers, on malformed files) are contained: the main process sees
//! the pipe close or time out, skips that file, and starts a fresh helper.
//!
//! Before reading any file the process sandboxes itself: it can no longer
//! open network connections or write to the filesystem, so a parser that is
//! exploited by a hostile document has nowhere to send or put anything.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};

extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

/// Everything the parsers need to read stays allowed; leaving the process
/// (network) and changing the disk (writes) do not.
const SANDBOX_PROFILE: &str = "(version 1)\n(allow default)\n(deny network*)\n(deny file-write*)\n";

fn enter_sandbox() -> Result<(), String> {
    let profile = CString::new(SANDBOX_PROFILE).expect("profile has no NUL");
    let mut error: *mut c_char = std::ptr::null_mut();
    // flags = 0: `profile` is the profile text itself, not the name of one
    if unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) } == 0 {
        return Ok(());
    }
    let message = if error.is_null() {
        "unknown error".to_string()
    } else {
        let message = unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned();
        unsafe { sandbox_free_error(error) };
        message
    };
    Err(message)
}

/// `--self-test <dir>`: report what the sandbox permits, for the test suite.
fn self_test(dir: &str) {
    let write = std::fs::write(std::path::Path::new(dir).join("escaped.txt"), b"x");
    println!("write: {}", if write.is_ok() { "allowed" } else { "denied" });
    let connect = std::net::TcpStream::connect_timeout(
        &"1.1.1.1:443".parse().expect("valid address"),
        std::time::Duration::from_secs(2),
    );
    let denied = matches!(&connect, Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied);
    println!("network: {}", if denied { "denied" } else { "not denied" });
    let read = std::fs::read_dir(dir);
    println!("read: {}", if read.is_ok() { "allowed" } else { "denied" });
}

fn main() {
    env_logger::init();

    // Refuse to parse untrusted files unsandboxed: better no document
    // contents in the index than a parser running with the user's full reach.
    if let Err(e) = enter_sandbox() {
        eprintln!("localsearch-extractor: could not enter sandbox: {e}");
        std::process::exit(70);
    }

    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--self-test" {
        self_test(&args[2]);
        return;
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    if let Err(e) = localsearch::extract::client::serve_helper(stdin.lock(), stdout.lock()) {
        log::error!("extractor helper stopped: {e}");
    }
}

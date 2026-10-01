/// LocalSearch content extractor helper process
///
/// Receives one file path per line on stdin, extracts its text, and writes
/// the result to stdout (see `localsearch::extract::client` for the wire
/// format). It exists so that parser crashes or hangs (PDFKit on malformed
/// files) are contained: the main process sees the pipe close or time out,
/// skips that file, and starts a fresh helper.
fn main() {
    env_logger::init();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    if let Err(e) = localsearch::extract::client::serve_helper(stdin.lock(), stdout.lock()) {
        log::error!("extractor helper stopped: {e}");
    }
}

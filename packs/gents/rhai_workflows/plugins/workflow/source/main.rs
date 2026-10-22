//! workflow plugin entry point: one JSON value on stdin, one on stdout.
use std::io::{Read, Write};

fn main() {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .expect("read stdin");
    let out = match serde_json::from_str(&raw) {
        Ok(input) => workflow::invoke(input),
        Err(e) => workflow::fail(
            "workflow_invalid_input",
            &format!("input is not JSON ({e}). Next: workflow help"),
        ),
    };
    let mut stdout = std::io::stdout();
    serde_json::to_writer(&mut stdout, &out).expect("write stdout");
    stdout.flush().expect("flush stdout");
}

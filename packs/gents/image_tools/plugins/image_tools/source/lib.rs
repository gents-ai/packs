//! image_tools plugin: deterministic image operations for agents. One JSON
//! value on stdin, one on stdout; a failure is one sentence on stderr and a
//! non-zero exit (see TOOL.md). The program is this library plus a one-line
//! `main`, so the pack's read-write entry `image_tools_write` can reuse it.
use std::io::{Read, Write};

mod analyze;
mod annotate;
mod chain;
mod codes;
#[cfg(test)]
mod codes_tests;
mod cursor;
mod cx;
mod decode;
#[cfg(test)]
mod decode_tests;
mod diff;
mod draw;
mod encode;
mod exif;
mod fit;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod fuzz_tests;
mod geom;
mod graph;
#[cfg(test)]
mod graph_tests;
mod hash;
mod input;
#[cfg(test)]
mod io_tests;
mod jpeg;
mod load;
mod model;
mod montage;
mod palette;
#[cfg(test)]
mod prop_tests;
mod resize;
mod run;
#[cfg(test)]
mod run_tests;
mod src;
#[cfg(test)]
mod step_tests;
#[cfg(test)]
mod testkit;
mod tile;
mod typed;

/// Reads one request from standard input, runs it and prints the result; on failure prints one
/// sentence on standard error and exits non-zero.
pub fn run_main() {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        fail("the request could not be read from standard input");
    }
    match run_text(&raw) {
        Ok(out) => {
            let mut stdout = std::io::stdout().lock();
            if stdout
                .write_all(out.as_bytes())
                .and_then(|()| stdout.write_all(b"\n"))
                .is_err()
            {
                fail("the result could not be written");
            }
        }
        Err(e) => fail(&e),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("image_tools: {message}");
    std::process::exit(1);
}

/// Runs the request in `raw` and returns the JSON text of the result.
fn run_text(raw: &str) -> Result<String, String> {
    if let Some(out) = graph::run_node(raw) {
        return out;
    }
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|_| "the request is not valid JSON; send one JSON object".to_string())?;
    let input: input::Input = typed::from_value(value, "the request", &[])?;
    run::execute(&input)
}

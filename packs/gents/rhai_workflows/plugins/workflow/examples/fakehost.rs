//! `cargo run --example fakehost -- --script x.rhai --args a.json --oracle labels.json [--trace]`
//! runs a script through the engine on the fake host and prints the run.
use workflow::fakehost::cli;

fn main() {
    let done =
        cli::parse(std::env::args().skip(1)).and_then(|args| cli::run(&args, workflow::invoke));
    match done {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("fakehost: {e}");
            std::process::exit(2);
        }
    }
}

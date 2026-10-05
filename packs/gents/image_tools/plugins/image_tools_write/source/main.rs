//! The `image_tools_write` program: the `image_tools` program under its own name.
//! The pack declares this entry with read-write access, so a call that asks for
//! `output.file` or `output.suffix` can write into the folder it is bound to.
fn main() {
    image_tools::run_main();
}

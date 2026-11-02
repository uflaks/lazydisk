//! Print the default keymap as Markdown: `cargo run -p lazydisk-core --example keymap > docs/KEYMAP.md`
fn main() {
    print!("{}", lazydisk_core::keymap::docs_markdown());
}

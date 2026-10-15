//! A Python extension module leaves Python's symbols for the interpreter to provide: on macOS the
//! linker has to be told so (maturin does this itself; plain `cargo build` needs it here).
fn main() {
    pyo3_build_config::add_extension_module_link_args();
}

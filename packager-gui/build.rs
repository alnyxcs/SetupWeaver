// packager-gui/build.rs
fn main() {
    slint_build::compile("ui.slint").expect("failed to compile Slint UI");
}

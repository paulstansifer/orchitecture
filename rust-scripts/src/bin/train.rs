#![recursion_limit = "256"]

fn main() {
    rust_scripts::qnn::train::<burn::backend::Wgpu>();
}
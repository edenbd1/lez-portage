//! Print the size and ImageID of a packed program binary.
//!
//! `cargo run --release --bin image_id -- <program .bin>`

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: image_id <program .bin>");
        std::process::exit(2);
    });
    let bin = std::fs::read(&path).expect("read the program binary");
    let id = risc0_binfmt::compute_image_id(&bin).expect("a valid program binary");
    let hex: String = id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    println!("{} bytes, ImageID {hex}", bin.len());
}

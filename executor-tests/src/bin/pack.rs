//! Wrap a guest ELF into the R0BF program binary LEZ loads, with the
//! protocol's default kernel, as `cargo risczero build` does.
//!
//! `cargo run --release --bin pack -- <guest elf> <out .bin>`

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, elf, out] = args.as_slice() else {
        eprintln!("usage: pack <guest elf> <out .bin>");
        std::process::exit(2);
    };
    let elf = std::fs::read(elf).expect("read the guest ELF");
    let bin = risc0_binfmt::ProgramBinary::new(&elf, risc0_zkos_v1compat::V1COMPAT_ELF).encode();
    std::fs::write(out, &bin).expect("write the program binary");
    let image_id = risc0_binfmt::compute_image_id(&bin).expect("a valid program binary");
    println!("{} bytes, ImageID {}", bin.len(), hex(image_id.as_bytes()));
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// wrap_bridge (RFP-022 prototype) for LEZ v0.3: the whole program is
// `wrap_bridge::{plan, apply}`; this binary hands them to the v0.3 entry point.

fn main() {
    lee_core::program::run_program::<_, _, [u8], Vec<u8>>(wrap_bridge::plan, wrap_bridge::apply)
}

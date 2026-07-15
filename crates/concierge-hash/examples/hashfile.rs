#![allow(
    clippy::all,
    clippy::pedantic,
    clippy::print_stdout,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&args[0]).unwrap();
    let b64 = concierge_hash::xxhash64_base64(&bytes);
    println!("xxhash64_base64 = {b64}  ({} bytes)", bytes.len());
    if let Some(expected) = args.get(1) {
        println!("expected        = {expected}");
        println!(
            "MATCH: {}",
            concierge_hash::matches_wabbajack_hash(&bytes, expected)
        );
    }
}

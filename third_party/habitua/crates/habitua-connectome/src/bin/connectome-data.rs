use std::env;
use std::error::Error;

#[path = "connectome-data/malecns.rs"]
mod malecns;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("convert-malecns-rate") => malecns::run_rate(args),
        _ => Err("usage: connectome-data convert-malecns-rate [options]".into()),
    }
}

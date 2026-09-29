fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = ara_lite::cli::run(&args) {
        eprintln!("ara-lite: {e}");
        std::process::exit(1);
    }
}

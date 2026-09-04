fn main() {
    if let Err(error) = coosenpai_desktop::run() {
        eprintln!("CooSenpAI desktop の起動に失敗しました: {error}");
        std::process::exit(1);
    }
}

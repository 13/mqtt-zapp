fn main() {
    if std::env::args().any(|a| a == "--version") {
        println!("mqtt-zapp {}", env!("CARGO_PKG_VERSION"));
    }
}

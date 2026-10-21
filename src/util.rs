pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAME: &str = env!("CARGO_PKG_NAME");

pub fn init_logging() {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "soundshare=info,cpal=warn,opus=warn");
    }
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .try_init();
}

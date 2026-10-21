use eframe::egui;
use soundshare::{app, engine, util};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    util::init_logging();
    log::info!("{} v{} starting", util::NAME, util::VERSION);

    let engine_h = engine::engine_handle();
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size((780.0, 520.0))
            .with_resizable(false),
        vsync: true,
        ..Default::default()
    };
    eframe::run_native(
        "SoundShare",
        native_options,
        Box::new(|cc| Ok(Box::new(app::SoundShareApp::new(cc, engine_h)))),
    )?;
    Ok(())
}

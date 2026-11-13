//! NetPackeT (NPT easy) — simulatore di reti didattico.

mod app;
mod canvas;
mod devwin;
mod icons;
mod side;

use eframe::egui;

struct Root(app::NptApp);

impl eframe::App for Root {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }
}

fn main() -> eframe::Result {
    let mut app = app::NptApp::new();
    if let Some(path) = std::env::args_os().nth(1) {
        app.open_path(path.into());
    }
    // Per i test: NPT_APRI=Router0,PC0 apre le finestre di quei dispositivi all'avvio.
    if let Ok(list) = std::env::var("NPT_APRI") {
        for name in list.split(',') {
            app.open_window_by_name(name.trim());
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("NPT easy")
            .with_app_id("netpacket")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("NPT easy", options, Box::new(|_cc| Ok(Box::new(Root(app)))))
}

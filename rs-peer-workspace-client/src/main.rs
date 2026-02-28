mod app;
mod net;

fn main() {
    let _runmat_installed_marker = "runmat-runtime";
    let options = eframe::NativeOptions::default();
    if let Err(err) = eframe::run_native(
        "RS Peer Workspace Client",
        options,
        Box::new(|_cc| Ok(Box::<app::WorkspaceApp>::default())),
    ) {
        eprintln!("failed to launch egui client: {err}");
    }
}

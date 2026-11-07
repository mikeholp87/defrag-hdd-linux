use eframe::egui;
use extentpilot_devices::{Mount, discover};
use std::io::Write;

use extentpilot_analysis::{Scan, scan};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

struct App {
    mounts: Vec<Mount>,
    error: String,
    selected: usize,
    path: String,
    result: String,
    receiver: Option<mpsc::Receiver<Result<Scan, String>>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    scan: Option<Scan>,
    filter: String,
}
impl Default for App {
    fn default() -> Self {
        let (mounts, error) = match discover() {
            Ok(m) => (m, String::new()),
            Err(e) => (Vec::new(), e.to_string()),
        };
        Self {
            mounts,
            error,
            selected: 0,
            path: String::new(),
            result: String::new(),
            receiver: None,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicU64::new(0)),
            scan: None,
            filter: String::new(),
        }
    }
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.receiver {
            match rx.try_recv() {
                Ok(Ok(result)) => {
                    self.result = result.summary();
                    self.scan = Some(result);
                    self.receiver = None;
                }
                Ok(Err(error)) => {
                    self.result = format!("Analysis unavailable: {error}");
                    self.receiver = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.result = "Analysis worker stopped unexpectedly".into();
                    self.receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(200));
                }
            }
        }
        egui::Panel::left("mounts").resizable(true).show(ui, |ui| {
            ui.heading("ExtentPilot");
            ui.label("MOUNTED FILESYSTEMS");
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, mount) in self.mounts.iter().enumerate() {
                    if ui
                        .selectable_label(
                            self.selected == i,
                            format!("{}  ·  {}", mount.target_label(), mount.fstype),
                        )
                        .clicked()
                    {
                        self.selected = i;
                    }
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("workspace").show(ui, |ui| {
            ui.heading("Storage overview");
            ui.label("Read-only inspection · No optimization operations are enabled");
            ui.separator();
            if !self.error.is_empty() {
                ui.colored_label(egui::Color32::LIGHT_RED, &self.error);
            }
            if let Some(m) = self.mounts.get(self.selected) {
                ui.heading(m.target_label());
                ui.label(format!(
                    "Filesystem: {}  |  Source: {}",
                    m.fstype,
                    m.source.as_deref().unwrap_or("Unknown")
                ));
                match (m.used, m.size) {
                    (Some(used), Some(size)) if size > 0 => {
                        ui.add(
                            egui::ProgressBar::new((used as f64 / size as f64).min(1.0) as f32)
                                .text(format!("{used} / {size} bytes")),
                        );
                    }
                    _ => {
                        ui.label("Capacity unavailable");
                    }
                }
                ui.label(match &m.device {
                    Some(d) => format!("Storage (as reported by the kernel): {}", d.summary()),
                    None => "Storage technology and topology: unknown (no block device)".into(),
                });
            }
            ui.add_space(24.0);
            ui.heading("Analyze files");
            ui.label("Enter a file or directory. Symlinks and nested mounts are excluded.");
            ui.text_edit_singleline(&mut self.path);
            if ui
                .add_enabled(
                    self.receiver.is_none(),
                    egui::Button::new("Start read-only scan"),
                )
                .clicked()
            {
                let path = self.path.clone();
                let (tx, rx) = mpsc::sync_channel(1);
                let ctx = ui.ctx().clone();
                self.cancel = Arc::new(AtomicBool::new(false));
                self.progress = Arc::new(AtomicU64::new(0));
                let cancel = self.cancel.clone(); let progress = self.progress.clone();
                match std::thread::Builder::new().name("extent-scan".into()).spawn(move || {
                    let result = scan(Path::new(&path), &cancel, &progress).map_err(|e| e.to_string());
                    let _ = tx.send(result); ctx.request_repaint();
                }) { Ok(_) => { self.receiver = Some(rx); self.scan = None; self.result = "Scanning…".into(); }, Err(e) => self.result = format!("Cannot start scan: {e}") }
            }
            if self.receiver.is_some() {
                ui.label(format!("{} entries examined · total unknown", self.progress.load(Ordering::Relaxed)));
                if ui.button("Cancel scan").clicked() { self.cancel.store(true, Ordering::Relaxed); }
            }
            ui.label(&self.result);
            if let Some(scan) = &self.scan {
                ui.label("Reported extents are metadata observations, not a performance score.");
                ui.label("Logical schematic · one cell per retained file · not physical disk blocks");
                let width = ui.available_width();
                let (response, painter) = ui.allocate_painter(egui::vec2(width, 90.0), egui::Sense::hover());
                let columns = ((width / 9.0) as usize).max(1);
                for (i, row) in scan.rows.iter().take(columns * 9).enumerate() {
                    let m = &row.measurement;
                    let color = if !m.complete || m.uncertain > 0 { egui::Color32::GRAY } else if m.shared > 0 { egui::Color32::from_rgb(160, 110, 210) } else if m.extents > 1 { egui::Color32::from_rgb(220, 170, 70) } else { egui::Color32::from_rgb(40, 175, 165) };
                    let pos = response.rect.min + egui::vec2((i % columns) as f32 * 9.0, (i / columns) as f32 * 9.0);
                    painter.rect_filled(egui::Rect::from_min_size(pos, egui::vec2(7.0,7.0)), 1.0, color);
                }
                if let Some(pos) = response.hover_pos() {
                    let local = pos - response.rect.min;
                    let index = (local.y / 9.0) as usize * columns + (local.x / 9.0) as usize;
                    if let Some(row) = scan.rows.get(index) { response.on_hover_text(&row.display_path); }
                }
                ui.label(format!("Showing up to {} retained files; retained detail rows are in the table and JSON report.", columns * 9));
                ui.label("Teal: zero/one extent · Amber: multiple extents · Purple: shared indicator · Gray: uncertain/incomplete");
                ui.text_edit_singleline(&mut self.filter);
                let rows: Vec<_> = scan.rows.iter().filter(|r| r.display_path.contains(&self.filter)).collect();
                ui.label("Path | bytes | extents | average extent bytes | measurement");
                egui::ScrollArea::vertical().id_salt("measurements").max_height(240.0).show_rows(ui, 22.0, rows.len(), |ui, range| {
                    for i in range { let row = rows[i]; let m = &row.measurement;
                        ui.add(egui::Label::new(format!("{} | {} | {} | {} | {}", row.display_path, m.size, m.extents, m.average(), if m.complete { "complete" } else { "incomplete" })).truncate()).on_hover_text(format!("Allocated: {} · Largest extent: {} · Shared indicators: {} · Uncertain extents: {}", m.allocated, m.largest, m.shared, m.uncertain));
                    }
                });
                egui::CollapsingHeader::new("Exclusions and errors (first 100)").show(ui, |ui| { for error in &scan.errors { ui.label(error); } });
            }
            ui.add_space(20.0);
            ui.label("Defragmentation, TRIM, history and scheduling are not yet available.");
            });
        });
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 2 && args[1] == "drives" {
        match discover() {
            Ok(mounts) => {
                for m in mounts {
                    // Raw target bytes: lossy conversion would hide non-UTF-8 mount paths.
                    let mut out = std::io::stdout().lock();
                    let _ = out.write_all(&m.target);
                    let media = m.device.as_ref().map_or("unknown", |d| d.media().label());
                    let _ = writeln!(
                        out,
                        "\t{}\t{}\t{media}",
                        m.fstype,
                        m.source.unwrap_or_default()
                    );
                }
            }
            Err(e) => {
                eprintln!("Discovery failed: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if args.get(1).is_some_and(|x| x == "analyze") {
        let Some(path) = args.get(2) else {
            eprintln!("Usage: extentpilot analyze PATH [--json]");
            std::process::exit(2);
        };
        if args.len() > 4 || (args.len() == 4 && args[3] != "--json") {
            eprintln!("Usage: extentpilot analyze PATH [--json]");
            std::process::exit(2);
        }
        match scan(Path::new(path), &AtomicBool::new(false), &AtomicU64::new(0)) {
            Ok(result) => {
                let output = if args.len() == 4 {
                    serde_json::to_string_pretty(&result).unwrap_or_else(|e| {
                        eprintln!("Export failed: {e}");
                        std::process::exit(1);
                    })
                } else {
                    result.summary()
                };
                if let Err(e) = writeln!(std::io::stdout().lock(), "{output}")
                    && e.kind() != std::io::ErrorKind::BrokenPipe
                {
                    eprintln!("Output failed: {e}");
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("Measurement unavailable: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if args.get(1).is_some_and(|x| x == "--help") {
        println!("extentpilot [drives | analyze PATH [--json] | --version]");
        return Ok(());
    }
    if args.get(1).is_some_and(|x| x == "--version") {
        println!("extentpilot {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.len() != 1 {
        eprintln!("Unknown command or arguments. Use extentpilot --help.");
        std::process::exit(2);
    }
    // SAFETY: geteuid has no arguments or memory safety requirements.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("The graphical application must not run as root.");
        std::process::exit(1);
    }
    eframe::run_native(
        "ExtentPilot",
        eframe::NativeOptions::default(),
        Box::new(|_| Ok(Box::<App>::default())),
    )
}

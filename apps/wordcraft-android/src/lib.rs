use android_activity::AndroidApp;
use jni::objects::JValue;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use wordcraft_engine::Session;
use wordcraft_ui_egui::{FileDialogAnswer, FileDialogRequest, Services, WordApp};

const WORKSPACE_DIR: &str = "/sdcard/Download/ArtCraft";
const FONTS_DIR: &str = "/sdcard/Download/ArtCraft/Fonts";

fn ensure_workspace_dir() -> PathBuf {
    let p = PathBuf::from(WORKSPACE_DIR);
    let _ = std::fs::create_dir_all(&p);
    let fonts = PathBuf::from(FONTS_DIR);
    let _ = std::fs::create_dir_all(&fonts);
    p
}

fn scan_and_load_custom_fonts() {
    let fonts_path = PathBuf::from(FONTS_DIR);
    if let Ok(entries) = std::fs::read_dir(&fonts_path) {
        let db = wordcraft_fonts::FontDb::global();
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                if ["ttf", "otf", "ttc", "woff", "woff2"].contains(&ext.as_str()) {
                    if let Ok(bytes) = std::fs::read(&p) {
                        let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("font");
                        let _ = db.install_font(fname, &bytes);
                        log::info!("Installed custom font: {}", fname);
                    }
                }
            }
        }
    }
}

fn apply_immersive_sticky(app: &AndroidApp) {
    let vm_ptr = app.vm_as_ptr();
    let activity_ptr = app.activity_as_ptr();
    if vm_ptr.is_null() || activity_ptr.is_null() {
        return;
    }

    unsafe {
        let jvm = match jni::JavaVM::from_raw(vm_ptr as *mut jni::sys::JavaVM) {
            Ok(jvm) => jvm,
            Err(_) => return,
        };
        let mut env = match jvm.attach_current_thread() {
            Ok(env) => env,
            Err(_) => return,
        };
        let activity = match jni::objects::JObject::from_raw(activity_ptr as jni::sys::jobject) {
            activity if !activity.as_raw().is_null() => activity,
            _ => return,
        };

        if let Ok(window) = env.call_method(&activity, "getWindow", "()Landroid/view/Window;", &[]) {
            if let Ok(window_obj) = window.l() {
                if let Ok(decor_view) = env.call_method(&window_obj, "getDecorView", "()Landroid/view/View;", &[]) {
                    if let Ok(decor_obj) = decor_view.l() {
                        let flags: i32 = 4096 | 4 | 2 | 1024 | 512 | 256;
                        let _ = env.call_method(&decor_obj, "setSystemUiVisibility", "(I)V", &[JValue::Int(flags)]);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum DialogMode {
    Open,
    Save,
}

struct PendingDialog {
    mode: DialogMode,
    current_dir: PathBuf,
    selected_file: Option<PathBuf>,
    save_filename: String,
    purpose: String,
    sender: Sender<FileDialogAnswer>,
}

static PENDING_DIALOG: Mutex<Option<PendingDialog>> = Mutex::new(None);

fn is_supported_extension(purpose: &str, ext: &str) -> bool {
    let ext_lower = ext.to_ascii_lowercase();
    match purpose {
        "picture" => ["png", "jpg", "jpeg", "webp", "gif", "bmp", "svg"].contains(&ext_lower.as_str()),
        "font" => ["ttf", "otf", "ttc", "woff", "woff2"].contains(&ext_lower.as_str()),
        _ => ["docx", "doc", "rtf", "txt", "wordcraft", "pdf", "odt"].contains(&ext_lower.as_str()),
    }
}

fn render_pending_file_dialog(ctx: &egui::Context) {
    let mut dlg_guard = PENDING_DIALOG.lock().unwrap();
    let Some(ref mut dlg) = *dlg_guard else { return };

    let is_open = dlg.mode == DialogMode::Open;
    let title = if is_open {
        if dlg.purpose == "font" {
            "Tambah Font (Add Font)"
        } else if dlg.purpose == "picture" {
            "Sisipkan Gambar (Insert Picture)"
        } else {
            "Buka Berkas (Open Document)"
        }
    } else {
        "Simpan Dokumen (Save As)"
    };

    let mut send_answer: Option<Option<String>> = None;
    let mut navigate_to: Option<PathBuf> = None;

    egui::Window::new(title)
        .id(egui::Id::new("wordcraft_android_file_dialog"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .fixed_size(egui::vec2(540.0, 420.0))
        .show(ctx, |ui| {
            // Navigation Bar
            ui.horizontal(|ui| {
                if ui.button("⬆ Atas").clicked() {
                    if let Some(parent) = dlg.current_dir.parent() {
                        navigate_to = Some(parent.to_path_buf());
                    }
                }
                if ui.button("📁 ArtCraft").clicked() {
                    navigate_to = Some(PathBuf::from(WORKSPACE_DIR));
                }
                if ui.button("🔤 Fonts").clicked() {
                    navigate_to = Some(PathBuf::from(FONTS_DIR));
                }
                ui.label(
                    egui::RichText::new(dlg.current_dir.to_string_lossy().to_string())
                        .strong()
                        .monospace(),
                );
            });

            ui.separator();

            // File & Directory list
            let mut entries_list = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dlg.current_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let is_dir = path.is_dir();
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                    if !name.starts_with('.') {
                        if is_dir {
                            entries_list.push((name, path, true));
                        } else {
                            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                            if !is_open || is_supported_extension(&dlg.purpose, ext) {
                                entries_list.push((name, path, false));
                            }
                        }
                    }
                }
            }
            entries_list.sort_by(|a, b| {
                if a.2 == b.2 {
                    a.0.to_lowercase().cmp(&b.0.to_lowercase())
                } else if a.2 {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            });

            egui::ScrollArea::vertical()
                .max_height(260.0)
                .min_scrolled_height(260.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if entries_list.is_empty() {
                        ui.label(egui::RichText::new("Direktori ini kosong.").italics());
                    }
                    for (name, path, is_dir) in entries_list {
                        let is_selected = dlg.selected_file.as_ref() == Some(&path);
                        ui.horizontal(|ui| {
                            let icon = if is_dir { "📁" } else { "📄" };
                            let label = format!("{} {}", icon, name);
                            let resp = ui.selectable_label(is_selected, label);
                            if resp.clicked() {
                                if is_dir {
                                    navigate_to = Some(path.clone());
                                } else {
                                    dlg.selected_file = Some(path.clone());
                                    if !is_open {
                                        dlg.save_filename = name.clone();
                                    }
                                }
                            }
                            if resp.double_clicked() {
                                if is_dir {
                                    navigate_to = Some(path);
                                } else if is_open {
                                    send_answer = Some(Some(path.to_string_lossy().into_owned()));
                                }
                            }
                        });
                    }
                });

            ui.separator();
            ui.add_space(4.0);

            // Save filename input if in Save mode
            if !is_open {
                ui.horizontal(|ui| {
                    ui.label("Nama Dokumen:");
                    ui.text_edit_singleline(&mut dlg.save_filename);
                });
                ui.add_space(4.0);
            }

            // Action buttons
            ui.horizontal(|ui| {
                if ui.button(egui::RichText::new("Batal").color(egui::Color32::from_rgb(220, 80, 80))).clicked() {
                    send_answer = Some(None);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if is_open {
                        let can_open = dlg.selected_file.is_some();
                        let btn = ui.add_enabled(can_open, egui::Button::new(egui::RichText::new("Pilih / Buka").strong()));
                        if btn.clicked() {
                            if let Some(ref sel) = dlg.selected_file {
                                send_answer = Some(Some(sel.to_string_lossy().into_owned()));
                            }
                        }
                    } else {
                        let can_save = !dlg.save_filename.trim().is_empty();
                        let btn = ui.add_enabled(can_save, egui::Button::new(egui::RichText::new("Simpan (Save)").strong()));
                        if btn.clicked() {
                            let mut target = dlg.current_dir.join(dlg.save_filename.trim());
                            if target.extension().is_none() {
                                target.set_extension("docx");
                            }
                            send_answer = Some(Some(target.to_string_lossy().into_owned()));
                        }
                    }
                });
            });
        });

    if let Some(new_dir) = navigate_to {
        dlg.current_dir = new_dir;
        dlg.selected_file = None;
    }

    if let Some(answer) = send_answer {
        let _ = dlg.sender.send(Ok(answer));
        *dlg_guard = None;
    }
}

struct WordAndroidApp(WordApp);

impl eframe::App for WordAndroidApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(raw);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.logic(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
        render_pending_file_dialog(ui.ctx());
    }
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("WordCraft"),
    );

    apply_immersive_sticky(&app);
    let _ = ensure_workspace_dir();
    scan_and_load_custom_fonts();

    let mut options = eframe::NativeOptions::default();
    options.android_app = Some(app);
    options.renderer = eframe::Renderer::Wgpu;

    let _ = eframe::run_native(
        "WordCraft",
        options,
        Box::new(|_cc| {
            let doc = wordcraft_doc::Document::new();
            let session = Session::new(doc);
            let mut services = Services::default();

            services.file_dialog = Some(Box::new(|req| {
                let (tx, rx): (Sender<FileDialogAnswer>, Receiver<FileDialogAnswer>) = channel();
                let base_dir = ensure_workspace_dir();
                match req {
                    FileDialogRequest::Open { purpose } => {
                        let start_dir = if purpose == "font" {
                            PathBuf::from(FONTS_DIR)
                        } else {
                            base_dir
                        };
                        let mut dlg = PENDING_DIALOG.lock().unwrap();
                        *dlg = Some(PendingDialog {
                            mode: DialogMode::Open,
                            current_dir: start_dir,
                            selected_file: None,
                            save_filename: String::new(),
                            purpose,
                            sender: tx,
                        });
                    }
                    FileDialogRequest::Save { name } => {
                        let path = Path::new(&name);
                        let filename = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| name.clone());
                        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).map(PathBuf::from).unwrap_or(base_dir);
                        let mut dlg = PENDING_DIALOG.lock().unwrap();
                        *dlg = Some(PendingDialog {
                            mode: DialogMode::Save,
                            current_dir: dir,
                            selected_file: None,
                            save_filename: filename,
                            purpose: "save".into(),
                            sender: tx,
                        });
                    }
                }
                rx
            }));

            let app = WordApp::new(session, services);
            Ok(Box::new(WordAndroidApp(app)))
        }),
    );
}

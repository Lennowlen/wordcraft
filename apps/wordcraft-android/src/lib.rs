use android_activity::AndroidApp;
use jni::objects::JValue;
use std::path::{Path, PathBuf};
use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, WordApp};

const WORKSPACE_DIR: &str = "/sdcard/Download/ArtCraft";

fn ensure_workspace_dir() -> PathBuf {
    let p = PathBuf::from(WORKSPACE_DIR);
    let _ = std::fs::create_dir_all(&p);
    p
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

            services.pick_open = Some(Box::new(|_purpose| {
                let base_dir = ensure_workspace_dir();
                let mut candidates = Vec::new();
                if let Ok(entries) = std::fs::read_dir(&base_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                        if ["docx", "doc", "rtf", "txt", "wordcraft"].contains(&ext.as_str()) {
                            candidates.push(path.to_string_lossy().into_owned());
                        }
                    }
                }
                candidates.first().cloned()
            }));

            services.pick_save = Some(Box::new(|name| {
                let base_dir = ensure_workspace_dir();
                let path = Path::new(name);
                let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_string());
                let mut target = base_dir.join(file_name);
                if target.extension().is_none() {
                    target.set_extension("docx");
                }
                Some(target.to_string_lossy().into_owned())
            }));

            let app = WordApp::new(session, services);
            Ok(Box::new(WordAndroidApp(app)))
        }),
    );
}

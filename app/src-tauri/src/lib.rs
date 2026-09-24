pub mod archive;
mod capture;
mod credentials;
pub mod deepseek;
pub mod domain;
mod models;
mod native_process;
mod pdf;
pub mod runtime;
mod soniox;
pub mod store;
use std::sync::Arc;
use tauri::Manager;
/// Runs an optional command, then returns only what changed since `since`.
#[tauri::command]
async fn sync(
    command: Option<serde_json::Value>,
    epoch: String,
    since: u64,
    state: tauri::State<'_, Arc<runtime::Runtime>>,
) -> Result<Box<serde_json::value::RawValue>, String> {
    let runtime = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.sync(command, &epoch, since))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn dispatch(
    command: serde_json::Value,
    state: tauri::State<'_, Arc<runtime::Runtime>>,
) -> Result<domain::State, String> {
    let runtime = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.dispatch(command))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn audio_data(
    session_id: String,
    run_id: String,
    start_sample: u64,
    end_sample: u64,
    state: tauri::State<'_, Arc<runtime::Runtime>>,
) -> Result<String, String> {
    let runtime = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        runtime.audio_data(&session_id, &run_id, start_sample, end_sample)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn runtime_info(
    state: tauri::State<'_, Arc<runtime::Runtime>>,
) -> Result<serde_json::Value, String> {
    let runtime = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.info())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn translate_text(
    request: deepseek::TranslateRequest,
    state: tauri::State<'_, Arc<runtime::Runtime>>,
) -> Result<deepseek::TranslateResult, String> {
    let runtime = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.translate_text(request))
        .await
        .map_err(|_| "DeepSeek 翻译任务意外中断".to_string())?
}
#[tauri::command]
async fn recognize_formula(
    request: deepseek::FormulaRequest,
    state: tauri::State<'_, Arc<runtime::Runtime>>,
) -> Result<deepseek::FormulaResult, String> {
    let runtime = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.recognize_formula(request))
        .await
        .map_err(|_| "DeepSeek 公式识别任务意外中断".to_string())?
}
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data = std::env::var_os("LECTUREEDIT_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?);
            let resource = app.path().resource_dir()?;
            let runtime = runtime::Runtime::new(data, resource).map_err(std::io::Error::other)?;
            app.manage(runtime);
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                window
                    .app_handle()
                    .state::<Arc<runtime::Runtime>>()
                    .shutdown();
                window.app_handle().exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            sync,
            dispatch,
            audio_data,
            runtime_info,
            translate_text,
            recognize_formula,
            pdf::export_pdf
        ])
        .build(tauri::generate_context!())
        .expect("LectureEdit could not start")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                app.state::<Arc<runtime::Runtime>>().shutdown();
            }
        });
}

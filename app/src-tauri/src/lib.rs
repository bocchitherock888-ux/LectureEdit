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
mod cloud;
mod doubao;
mod bailian;
mod elevenlabs;
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
/// Pages people need while setting up a cloud service. Only these addresses can be opened, so
/// the web view cannot be used to launch arbitrary programs or URLs.
fn help_link(id: &str) -> Option<&'static str> {
    Some(match id {
        "doubao-console" => "https://console.volcengine.com/speech/new/setting/apikeys",
        "doubao-docs" => "https://www.volcengine.com/docs/6561/1354869",
        "bailian-console-cn" => "https://bailian.console.aliyun.com/?tab=model#/api-key",
        "bailian-console-intl" => "https://modelstudio.console.alibabacloud.com/?tab=model#/api-key",
        "elevenlabs-console" => "https://elevenlabs.io/app/settings/api-keys",
        "soniox-console" => "https://console.soniox.com",
        _ => return None,
    })
}

#[tauri::command]
fn open_help_link(id: String) -> Result<(), String> {
    let url = help_link(&id).ok_or("未知链接")?;
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    result.map(|_| ()).map_err(|_| format!("无法打开浏览器，请手动访问 {url}"))
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
            open_help_link,
            pdf::export_pdf
        ])
        .build(tauri::generate_context!())
        .expect("随堂 could not start")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                app.state::<Arc<runtime::Runtime>>().shutdown();
            }
        });
}

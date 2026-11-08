mod launcher;

use std::path::PathBuf;

pub use launcher::{DesktopProfile, LauncherError, ensure_service, ensure_service_with_timeout, resolve_data_dir};
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};

const TRAY_ID: &str = "vatra";
const QUIT_EVENT: &str = "vatra://quit-requested";

fn managed_file_path(root:&std::path::Path,path:&std::path::Path,reveal:bool)->Result<PathBuf,String>{
    let root=std::fs::canonicalize(root.join("files")).map_err(|e|e.to_string())?;
    let path=std::fs::canonicalize(path).map_err(|e|e.to_string())?;
    if !path.starts_with(root)||!path.is_file(){return Err("Only managed conversation files can be opened here.".into());}
    let ext=path.extension().and_then(|e|e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !reveal&&["exe","com","cmd","bat","ps1","sh","msi","scr","lnk","url","vbs","js","hta","reg"].contains(&ext.as_str()){
        return Err("Save executable files explicitly before opening them.".into());
    }
    Ok(path)
}
#[tauri::command]
async fn open_managed_file(app:tauri::AppHandle,profile:tauri::State<'_,DesktopProfile>,path:PathBuf,reveal:bool)->Result<(),String>{
    use tauri_plugin_opener::OpenerExt;
    let path=managed_file_path(profile.directory(),&path,reveal)?;
    if reveal{app.opener().reveal_item_in_dir(path).map_err(|e|e.to_string())}
    else{app.opener().open_path(path.to_string_lossy().into_owned(),None::<&str>).map_err(|e|e.to_string())}
}

fn tray_menu(app: &tauri::AppHandle, open: &str, quit: &str) -> tauri::Result<Menu<tauri::Wry>> {
    let open_item = MenuItem::with_id(app, "show", open, true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", quit, true, None::<&str>)?;
    Menu::with_items(app, &[&open_item, &quit_item])
}

#[tauri::command]
async fn open_skill_source(app:tauri::AppHandle,profile:tauri::State<'_,DesktopProfile>,path:PathBuf,project_id:Option<i64>,folder:bool)->Result<(),String>{
    use tauri_plugin_opener::OpenerExt;
    let directory=profile.directory().to_path_buf();
    let target=tauri::async_runtime::spawn_blocking(move||vatra_service::skills::source_target(&directory,&path,project_id,folder)).await.map_err(|e|e.to_string())??;
    app.opener().open_path(target.to_string_lossy().into_owned(),None::<&str>).map_err(|e|e.to_string())
}

/// The window is created once, so reopening it means showing the same one.
fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[derive(Serialize)]
struct ConnectionInfo {
    port: u16,
    token: String,
}

/// Called on start and on every reconnect, so a service that died is started again.
#[tauri::command]
async fn connection_info(profile: tauri::State<'_, DesktopProfile>) -> Result<ConnectionInfo, String> {
    connect(&profile).await
}

fn sidecar_path() -> Result<PathBuf, String> {
    let app = std::env::current_exe().map_err(|error| error.to_string())?;
    let folder = app.parent().ok_or("Couldn't find the app folder.")?;
    Ok(folder.join("vatra.exe"))
}

async fn connect(profile: &DesktopProfile) -> Result<ConnectionInfo, String> {
    let info = profile.ensure_service(&sidecar_path()?).await.map_err(|error| error.to_string())?;
    let token = std::fs::read_to_string(profile.directory().join("token")).map_err(|error| error.to_string())?;
    Ok(ConnectionInfo { port: info.port, token: token.trim().to_string() })
}

/// Whether updates are set up, and whether one is waiting. Without an address, nothing is checked.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateStatus {
    configured: bool,
    available: Option<String>,
    error: Option<String>,
}

#[tauri::command]
async fn update_status(app: tauri::AppHandle) -> UpdateStatus {
    use tauri_plugin_updater::UpdaterExt;
    let checked = match app.updater() {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    match checked {
        Ok(update) => UpdateStatus { configured: true, available: update.map(|update| update.version), error: None },
        Err(tauri_plugin_updater::Error::EmptyEndpoints) => UpdateStatus { configured: false, available: None, error: None },
        Err(error) => UpdateStatus { configured: true, available: None, error: Some(error.to_string()) },
    }
}

/// The tray follows the interface language, so the menu never mixes languages.
#[tauri::command]
fn set_tray_labels(app: tauri::AppHandle, open: String, quit: String, tooltip: String) -> Result<(), String> {
    let menu = tray_menu(&app, &open, &quit).map_err(|error| error.to_string())?;
    let tray = app.tray_by_id(TRAY_ID).ok_or("The Vatra tray is unavailable.")?;
    tray.set_menu(Some(menu)).map_err(|error| error.to_string())?;
    tray.set_tooltip(Some(tooltip)).map_err(|error| error.to_string())?;
    Ok(())
}

/// Quitting must not leave the service behind. Only the service started from this
/// installation is stopped, and only through its authenticated RPC.
#[tauri::command]
async fn quit_app(app: tauri::AppHandle, profile: tauri::State<'_, DesktopProfile>) -> Result<(), String> {
    let binary = sidecar_path()?;
    let mut command = tokio::process::Command::new(&binary);
    command
        .arg("service-shutdown-owned")
        .arg("--data-dir")
        .arg(profile.directory())
        .arg("--expected-exe")
        .arg(&binary)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = tokio::time::timeout(std::time::Duration::from_secs(20), command.status())
        .await
        .map_err(|_| "The Vatra service did not stop in time.")?
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("The Vatra service did not stop. Close it before quitting Vatra.".into());
    }
    app.exit(0);
    Ok(())
}

pub fn run() {
    let profile = DesktopProfile::from_args(std::env::args_os()).expect("Invalid Vatra desktop profile");
    tauri::Builder::default()
        .manage(profile)
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            // Closing keeps reminders alive, so the window hides and the service keeps running.
            // This lives in Rust as well so a missing frontend listener can never destroy it.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let menu = tray_menu(app.handle(), "Open Vatra", "Quit")?;
            let Some(icon) = app.default_window_icon().cloned() else {
                return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Vatra has no icon for the tray.").into());
            };
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Vatra — running for reminders")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => {
                        show_main(app);
                        let _ = app.emit(QUIT_EVENT, ());
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![connection_info, update_status, install_update, set_tray_labels, quit_app,open_managed_file,open_skill_source])
        .run(tauri::generate_context!())
        .expect("Vatra couldn't start");
}

/// The updater verifies the downloaded artifact signature before running its installer.
/// Called only after the person confirms an available update in Settings.
#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app.updater().map_err(|e|e.to_string())?.check().await.map_err(|e|e.to_string())?
        .ok_or("No update is available")?;
    update.download_and_install(|_,_|{},||{}).await.map_err(|e|e.to_string())
}

#[cfg(test)]
mod managed_file_tests {
    use super::managed_file_path;
    #[test]
    fn opening_managed_files_cannot_escape_the_profile_or_execute_scripts(){
        let profile=tempfile::tempdir().unwrap();let folder=profile.path().join("files/session/1");std::fs::create_dir_all(&folder).unwrap();
        let document=folder.join("result.md");std::fs::write(&document,"local").unwrap();
        let script=folder.join("result.ps1");std::fs::write(&script,"exit 0").unwrap();
        let outside=profile.path().join("outside.md");std::fs::write(&outside,"outside").unwrap();
        assert!(managed_file_path(profile.path(),&document,false).is_ok());
        assert!(managed_file_path(profile.path(),&outside,true).is_err());
        assert!(managed_file_path(profile.path(),&script,false).is_err());
        assert!(managed_file_path(profile.path(),&script,true).is_ok());
    }
}

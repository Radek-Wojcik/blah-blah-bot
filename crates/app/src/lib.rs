//! Desktop shell: tray icon, windows, global hotkeys and IPC commands.

mod commands;
mod cycle_commands;
mod cycles;
mod draft_commands;
mod hotkeys;
mod ipc;
mod llm;
mod logging;
mod overlay;
mod processing;
mod processing_commands;
mod recording;
mod recording_dto;
mod secrets;
mod settings;
mod speaker_commands;
mod state;

use std::time::Duration;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};

use state::AppState;

/// How often the widget gets a fresh recording state (levels, elapsed time).
const TICK: Duration = Duration::from_millis(250);

/// Show the main window and bring it to the front.
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Toggle the small recorder widget next to the tray icon.
fn toggle_widget(app: &AppHandle, tray_rect: Option<tauri::Rect>) {
    let Some(w) = app.get_webview_window("widget") else {
        return;
    };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    if let (Some(rect), Ok(size)) = (tray_rect, w.outer_size()) {
        let scale = w.scale_factor().unwrap_or(1.0);
        let pos = rect.position.to_physical::<f64>(scale);
        let tray = rect.size.to_physical::<f64>(scale);
        // Taskbar at the bottom: open above the icon, clamped to the left screen edge.
        let x = (pos.x + tray.width / 2.0 - f64::from(size.width) / 2.0).max(0.0);
        let y = (pos.y - f64::from(size.height) - 8.0).max(0.0);
        let _ = w.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
    }
    let _ = w.show();
    let _ = w.set_focus();
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Blah Blah Bot", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("window icon".into()))?;

    TrayIconBuilder::with_id("tray")
        .icon(icon)
        .tooltip("Blah Blah Bot")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                toggle_widget(tray.app_handle(), Some(rect));
            }
        })
        .build(app)?;
    Ok(())
}

/// Background thread that pushes the recording state to the UI while recording.
fn start_ticker(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("recording-ticker".into())
        .spawn(move || loop {
            std::thread::sleep(TICK);
            let state = app.state::<AppState>();
            if let Some(s) = state.recorder.tick() {
                commands::emit_state(&app, &s);
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "could not start the state ticker");
    }
}

/// Entry point called from `main`.
pub fn run() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            let log_dir = handle.path().app_log_dir()?;
            logging::init(&log_dir);
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");

            secrets::init();
            let state = AppState::open(&handle)?;
            let recovered = state.mark_interrupted()?;
            if !recovered.is_empty() {
                tracing::warn!(count = recovered.len(), "found interrupted recordings");
            }
            let settings = state.settings()?;
            app.manage(state);
            // Jobs interrupted by a crash continue; nothing else starts on its own.
            handle.state::<AppState>().processing().resume_interrupted();
            if let Ok(root) = handle.state::<AppState>().recordings_root() {
                handle
                    .state::<AppState>()
                    .processing()
                    .sweep_expired_audio(&root);
                handle.state::<AppState>().processing().sweep_clips(&root);
            }

            // A taken hotkey must not stop the application from starting.
            if let Err(e) = hotkeys::register(&handle, &settings) {
                tracing::warn!(error = %e, "hotkeys not registered");
            }
            build_tray(&handle)?;
            start_ticker(handle.clone());
            show_main(&handle);
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing the main window keeps the application in the tray.
            WindowEvent::CloseRequested { api, .. } if window.label() != "overlay" => {
                api.prevent_close();
                let _ = window.hide();
            }
            // The widget behaves like a popup: it hides when it loses focus.
            WindowEvent::Focused(false) if window.label() == "widget" => {
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_meetings,
            commands::get_meeting,
            commands::delete_meeting,
            commands::recover_meeting,
            commands::list_sources,
            commands::recording_state,
            commands::start_recording,
            commands::stop_recording,
            commands::add_source,
            commands::remove_source,
            commands::take_screenshot,
            commands::get_settings,
            commands::save_settings,
            processing_commands::process_meeting,
            processing_commands::queue_state,
            processing_commands::retry_job,
            processing_commands::cancel_job,
            processing_commands::get_transcript,
            processing_commands::edit_segment,
            processing_commands::meeting_image,
            processing_commands::get_processing_settings,
            processing_commands::save_processing_settings,
            processing_commands::set_hf_token,
            processing_commands::clear_hf_token,
            processing_commands::model_status,
            processing_commands::download_models,
            speaker_commands::get_meeting_voices,
            speaker_commands::assign_voice,
            speaker_commands::assign_voice_new,
            speaker_commands::merge_voices,
            speaker_commands::confirm_matches,
            speaker_commands::voice_clip,
            speaker_commands::list_participants,
            speaker_commands::create_participant,
            speaker_commands::rename_participant,
            speaker_commands::delete_participant,
            speaker_commands::reset_profile,
            speaker_commands::delete_voice_data,
            processing_commands::set_cloud_key,
            processing_commands::clear_cloud_key,
            processing_commands::cloud_key_present,
            processing_commands::set_external_key,
            processing_commands::clear_external_key,
            processing_commands::external_key_present,
            processing_commands::cloud_preview,
            processing_commands::confirm_cloud,
            cycle_commands::list_cycles,
            cycle_commands::list_cycle_repos,
            cycle_commands::get_cycle,
            cycle_commands::create_cycle,
            cycle_commands::save_cycle,
            cycle_commands::list_templates,
            cycle_commands::get_template,
            cycle_commands::save_template,
            cycle_commands::validate_template,
            cycle_commands::add_cycle_repo,
            cycle_commands::pull_cycle_repo,
            cycle_commands::report_defaults,
            draft_commands::get_draft,
            draft_commands::update_draft_item,
            draft_commands::move_draft_item,
            draft_commands::delete_draft_item,
            draft_commands::add_draft_item,
            draft_commands::set_draft_sections,
            draft_commands::set_report_mode,
            draft_commands::accept_new_draft,
            draft_commands::discard_new_draft,
            draft_commands::render_report,
            overlay::overlay_image,
            overlay::overlay_finish,
        ])
        .run(tauri::generate_context!());

    if let Err(e) = result {
        tracing::error!(error = %e, "application failed");
        std::process::exit(1);
    }
}

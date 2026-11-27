//! Desktop shell: tray icon, windows, global hotkeys and IPC commands.

mod briefing;
mod browser;
#[cfg(test)]
#[path = "../bundle_guard.rs"]
mod bundle_guard;
mod commands;
mod cycle_commands;
mod cycles;
mod detection;
mod downloads;
mod draft_commands;
mod hotkeys;
mod ipc;
mod issue_sync;
mod llm;
mod local_export;
mod logging;
mod overlay;
mod processing;
mod processing_commands;
mod project_commands;
mod project_github;
mod publish_commands;
mod publish_secrets;
mod publishing;
mod recording;
mod recording_dto;
#[cfg(test)]
mod release_check;
mod secrets;
mod settings;
mod setup;
mod setup_commands;
mod sources;
mod speaker_commands;
mod state;

use std::time::{Duration, Instant};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use state::AppState;

/// How often the widget gets a fresh recording state (levels, elapsed time).
const TICK: Duration = Duration::from_millis(250);
/// How often live 12-bar audio levels are emitted while a recording session is active.
const LEVELS_TICK: Duration = Duration::from_millis(60);

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

/// Background thread that pushes live audio levels (~60 ms) and full recording state (250 ms)
/// to the UI while recording.
fn start_ticker(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("recording-ticker".into())
        .spawn(move || {
            let mut last_state = Instant::now() - TICK;
            loop {
                std::thread::sleep(LEVELS_TICK);
                let state = app.state::<AppState>();
                if let Some(levels) = state.recorder.levels() {
                    commands::emit_levels(&app, &levels);
                }
                if last_state.elapsed() >= TICK {
                    last_state = Instant::now();
                    if let Some(s) = state.recorder.tick() {
                        commands::emit_state(&app, &s);
                    }
                    // Every way to start or stop a recording ends up here within one tick.
                    state.browser().sync_recording(state.recorder.meeting_id());
                }
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
        .plugin(tauri_plugin_dialog::init())
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
            handle.state::<AppState>().start_browser(handle.clone());
            handle.state::<AppState>().start_detection(handle.clone());
            // Jobs interrupted by a crash continue; nothing else starts on its own.
            handle.state::<AppState>().processing().resume_interrupted();
            // Publishing operations interrupted by a crash or by a lost network are taken up
            // again by their own thread; the UI is told after every change.
            {
                let publishing = handle.state::<AppState>().publishing().clone();
                let (to_ui, to_cycles) = (handle.clone(), handle.clone());
                publishing.connect(
                    move |state| {
                        if let Err(e) = to_ui.emit(publishing::EVENT_OUTBOX, state) {
                            tracing::warn!(error = %e, "could not emit the outbox state");
                        }
                    },
                    move |_| {
                        if let Err(e) = to_cycles.emit(cycle_commands::EVENT_CYCLES, ()) {
                            tracing::warn!(error = %e, "could not emit the cycles change");
                        }
                    },
                );
                publishing.start();
            }
            if let Ok(root) = handle.state::<AppState>().recordings_root() {
                handle.state::<AppState>().processing().sweep_clips(&root);
            }

            // A taken hotkey must not stop the application from starting.
            hotkeys::register_initial(&handle, &settings);
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
            commands::suspend_hotkeys,
            commands::resume_hotkeys,
            commands::hotkey_status,
            commands::save_last_sources,
            browser::bridge_status,
            browser::start_pairing,
            browser::cancel_pairing,
            browser::unpair_client,
            detection::get_detection_settings,
            detection::save_detection_settings,
            detection::detection_prompt,
            detection::answer_detection,
            processing_commands::process_meeting,
            processing_commands::queue_state,
            processing_commands::set_queue_gpu_override,
            processing_commands::force_process,
            processing_commands::schedule_job,
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
            setup_commands::setup_status,
            setup_commands::detect_llama_server,
            setup_commands::find_gguf_models,
            setup_commands::check_setup_path,
            setup_commands::prepare_worker_env,
            setup_commands::cancel_setup_task,
            setup_commands::download_llama_server,
            setup_commands::download_language_model,
            setup_commands::set_first_run_done,
            setup_commands::dismiss_legacy_venv_notice,
            project_commands::get_project_prefs,
            project_commands::save_project_prefs,
            project_commands::wizard_create_project,
            project_github::github_repo_status,
            project_github::wizard_create_project_with_github,
            project_commands::import_meeting,
            project_commands::export_meeting_markdown,
            project_commands::save_meeting_note_as,
            project_commands::auto_detect_models,
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
            speaker_commands::list_all_voice_profiles,
            speaker_commands::voice_profile_sample_clip,
            speaker_commands::enroll_voice_profile,
            speaker_commands::add_profile_to_project,
            speaker_commands::remove_profile_from_project,
            speaker_commands::merge_participants,
            speaker_commands::undo_voice_decision,
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
            cycle_commands::delete_project,
            cycle_commands::list_templates,
            cycle_commands::get_template,
            cycle_commands::save_template,
            cycle_commands::validate_template,
            cycle_commands::add_cycle_repo,
            cycle_commands::pull_cycle_repo,
            cycle_commands::report_defaults,
            briefing::get_briefing,
            briefing::open_briefing,
            briefing::set_task_done,
            publish_commands::publishing_secrets,
            publish_commands::set_github_token,
            publish_commands::clear_github_token,
            publish_commands::test_github_access,
            publish_commands::set_github_repo,
            publish_commands::set_target_rule,
            publish_commands::set_target_secret,
            publish_commands::clear_target_secret,
            publish_commands::test_discord_webhook,
            publish_commands::test_notion_access,
            publish_commands::notion_schema,
            publish_commands::set_notion_mapping,
            publish_commands::test_discord_bot,
            publish_commands::search_discord_members,
            publish_commands::list_discord_roles,
            publish_commands::set_participant_discord,
            publish_commands::reexport_notion,
            publish_commands::publish_plan,
            publish_commands::publish_meeting,
            publish_commands::confirm_public_repo,
            publish_commands::outbox_state,
            publish_commands::retry_outbox_op,
            publish_commands::cancel_outbox_op,
            publish_commands::cycle_tasks,
            publish_commands::list_corrections,
            publish_commands::add_correction,
            publish_commands::publish_corrections,
            publish_commands::delete_correction,
            draft_commands::get_draft,
            draft_commands::update_draft_item,
            draft_commands::move_draft_item,
            draft_commands::delete_draft_item,
            draft_commands::add_draft_item,
            draft_commands::set_draft_sections,
            draft_commands::set_report_mode,
            draft_commands::accept_new_draft,
            draft_commands::discard_new_draft,
            draft_commands::set_status_decision,
            draft_commands::add_status_proposal,
            draft_commands::render_report,
            overlay::overlay_image,
            overlay::overlay_ready,
            overlay::overlay_finish,
        ])
        .run(tauri::generate_context!());

    if let Err(e) = result {
        tracing::error!(error = %e, "application failed");
        std::process::exit(1);
    }
}

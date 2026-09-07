use slint::{ComponentHandle, Global};

use crate::{
    AudioDeviceState, EventFlagKind, MainWindow, SettingsEditorState, SlintAudioParameters,
    SlintConverterCustom, SlintEventProcessorConfig, SlintPostProcessorConfig, SlintRealtimeConfig,
    SlintRendererConfig, SlintSynthConfig, SlintSystemCustom, actions,
    app::AppContext,
    errors,
    slint_conv::{
        slint_to_converter_custom, slint_to_evproc, slint_to_postproc, slint_to_synth,
        slint_to_system_custom,
    },
    state::index_of,
    sync::{sync_audio_devices_to_slint, sync_device_to_slint},
    views::device_manager::WINDOWS_COMPAT_TEXT,
};
use maestro_core::audio_params::{AudioParameters, ChannelCount};

/// Dialog title used for problems raised while saving a component config.
const ERROR_TITLE: &str = "Settings";

fn set_flag(list: &[u8], index: i32, on: bool) -> Box<[u8]> {
    let Ok(index) = u8::try_from(index) else {
        return list.into();
    };
    let mut out: Vec<u8> = list.iter().copied().filter(|i| *i != index).collect();
    if on {
        out.push(index);
    }
    out.sort_unstable();
    out.into_boxed_slice()
}

pub fn setup(ui: &MainWindow, cx: &AppContext) {
    let state = SettingsEditorState::get(ui);

    // Open / close

    let c = cx.clone();
    state.on_open_settings(move |name| {
        c.with(|ui, data| actions::open_component_settings_by_name(ui, data, name.as_str()));
    });

    let c = cx.clone();
    state.on_close(move || {
        c.with_ui(|ui| SettingsEditorState::get(ui).set_open(false));
    });

    let c = cx.clone();
    state.on_exit_standalone(move || {
        c.with_ui(|ui| {
            let _ = ui.hide();
        });
    });

    // General

    let c = cx.clone();
    state.on_update_enabled(move |val| {
        c.with(|ui, data| actions::edit_selected_config(ui, data, |f| f.enabled = val));
    });

    let c = cx.clone();
    state.on_update_sflist(move |val| {
        c.with(|ui, data| {
            actions::edit_selected_config(ui, data, |f| f.sflist = val.as_str().to_string())
        });
    });

    // Audio parameters

    let c = cx.clone();
    state.on_update_audio_params(move |ap: SlintAudioParameters| {
        c.with(|ui, data| {
            actions::edit_selected_config(ui, data, |f| {
                f.audio_params = AudioParameters {
                    channels: match ap.channels {
                        1 => ChannelCount::Mono,
                        _ => ChannelCount::Stereo,
                    },
                    sample_rate: ap.sample_rate.max(1) as u32,
                };
            })
        });
    });

    // Realtime

    let c = cx.clone();
    AudioDeviceState::get(ui).on_refresh(move || {
        c.with(|ui, data| {
            let (host, device) = data
                .selected_config()
                .map(|c| {
                    (
                        c.realtime.audio_host.clone(),
                        c.realtime.output_device.clone(),
                    )
                })
                .unwrap_or_default();

            data.audio.refresh(host.as_deref(), device.as_deref());
            sync_audio_devices_to_slint(ui, data);
        });
    });

    let c = cx.clone();
    state.on_update_realtime(move |rt: SlintRealtimeConfig| {
        c.with(|ui, data| {
            let host = data.audio.host_id(rt.audio_host_index);
            let device = data.audio.device_id(rt.output_device_index);

            let hardware_changed = data.selected_config().is_some_and(|c| {
                c.realtime.audio_host != host || c.realtime.output_device != device
            });

            if hardware_changed {
                data.audio.refresh(host.as_deref(), device.as_deref());
            }

            actions::edit_selected_config(ui, data, |f| {
                f.realtime.device_audio_params = rt.device_audio_params;
                f.realtime.render_buffer_ms = rt.render_buffer_ms;
                f.realtime.precision_playback = rt.precision_playback;
                f.realtime.max_nps = rt.has_max_nps.then(|| rt.max_nps.max(0) as usize);

                f.realtime.audio_host = host;
                f.realtime.output_device = device;
                f.realtime.buffer_size =
                    (rt.has_buffer_size && rt.buffer_size > 0).then_some(rt.buffer_size as u32);
            })
        });
    });

    // Renderer

    let c = cx.clone();
    state.on_update_renderer(move |r: SlintRendererConfig| {
        c.with(|ui, data| {
            actions::edit_selected_config(ui, data, |f| {
                f.renderer.port_threads =
                    r.has_port_threads.then(|| r.port_threads.max(1) as usize);
                f.renderer.render_fps = r.has_render_fps.then_some(r.render_fps as f64);
                f.renderer.render_fps_variation = r.render_fps_variation.clamp(0.0, 100.0);
            })
        });
    });

    // Synth

    let c = cx.clone();
    state.on_update_synth(move |s: SlintSynthConfig| {
        c.with(|ui, data| {
            actions::edit_selected_config(ui, data, |f| f.renderer.synth = slint_to_synth(&s))
        });
    });

    // Event processor

    let c = cx.clone();
    state.on_toggle_event_processor(move |on| {
        c.with(|ui, data| actions::edit_selected_config(ui, data, |f| f.has_event_processor = on));
    });

    let c = cx.clone();
    state.on_toggle_event_flag(move |kind, index, on| {
        c.with(|ui, data| {
            actions::edit_selected_config(ui, data, |f| {
                let list = match kind {
                    EventFlagKind::BypassChannel => &mut f.event_processor.bypass_channels,
                    EventFlagKind::IgnoreChannel => &mut f.event_processor.ignore_channels,
                    EventFlagKind::BypassPort => &mut f.event_processor.bypass_ports,
                    EventFlagKind::IgnorePort => &mut f.event_processor.ignore_ports,
                };
                *list = set_flag(list, index, on);
                f.has_event_processor = true;
            })
        });
    });

    let c = cx.clone();
    state.on_update_event_processor(move |e: SlintEventProcessorConfig| {
        c.with(|ui, data| {
            actions::edit_selected_config(ui, data, |f| {
                f.event_processor = slint_to_evproc(&e);
                f.has_event_processor = true;
            })
        });
    });

    // Post processor

    let c = cx.clone();
    state.on_toggle_post_processor(move |on| {
        c.with(|ui, data| actions::edit_selected_config(ui, data, |f| f.has_post_processor = on));
    });

    let c = cx.clone();
    state.on_update_post_processor(move |p: SlintPostProcessorConfig| {
        c.with(|ui, data| {
            actions::edit_selected_config(ui, data, |f| {
                f.post_processor = slint_to_postproc(&p);
                f.has_post_processor = true;
            })
        });
    });

    // Component-specific payloads

    let c = cx.clone();
    state.on_update_converter_custom(move |custom: SlintConverterCustom| {
        c.with(|ui, data| {
            actions::edit_selected_config(ui, data, |f| {
                f.converter_custom = slint_to_converter_custom(&custom)
            })
        });
    });

    let c = cx.clone();
    state.on_update_system_custom(move |custom: SlintSystemCustom| {
        c.with(|ui, data| {
            // TODO remove when WMS are implemented
            actions::edit_selected_config(ui, data, |f| {
                f.system_custom = slint_to_system_custom(&custom);
                if f.system_custom.midi2_enabled && cfg!(windows) {
                    errors::report("Not supported", WINDOWS_COMPAT_TEXT);
                    f.system_custom.midi2_enabled = false;
                }
            });

            // in case MIDI 2.0 was selected on windows
            sync_device_to_slint(ui, data);
        });
    });

    // Save / discard

    let c = cx.clone();
    state.on_save_current(move || {
        c.with(|ui, data| {
            if let Some(pos) = index_of(data.selected_config_idx, data.config_files.len()) {
                actions::save_config_at(ui, data, pos, ERROR_TITLE);
            }
        });
    });

    let c = cx.clone();
    state.on_discard_current(move || {
        c.with(|ui, data| {
            if let Some(pos) = index_of(data.selected_config_idx, data.config_files.len()) {
                actions::discard_config_at(ui, data, pos);
            }
        });
    });
}

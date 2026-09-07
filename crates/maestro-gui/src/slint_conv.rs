use slint::{Model, ModelRc, VecModel};

use maestro_core::{
    file_renderer::{BITRATES_KBPS, DEFAULT_BITRATE_KBPS, OutputFormat, WavBitDepth},
    renderer::config::{
        AudioLimiterConfig, EventProcessorConfig, PostProcessorConfig, SynthConfig,
        bassmidi::{BASSMIDIConfig, BASSMIDIInterpolation, BASSMIDIThreading},
        fluidsynth::{FluidSynthConfig, FluidSynthInterpolation},
    },
};

use crate::{
    SlintEventProcessorConfig, SlintPostProcessorConfig, SlintSynthConfig, SynthKind, Tab,
    state::ConverterCustom,
};

pub fn tab_to_key(tab: Tab) -> &'static str {
    match tab {
        Tab::Welcome => "welcome",
        Tab::Soundfonts => "soundfonts",
        Tab::Converter => "converter",
        Tab::Device => "device",
        Tab::System => "system",
    }
}

pub fn tab_from_key(key: &str) -> Option<Tab> {
    match key {
        "welcome" => Some(Tab::Welcome),
        "soundfonts" => Some(Tab::Soundfonts),
        "converter" => Some(Tab::Converter),
        "device" => Some(Tab::Device),
        "system" => Some(Tab::System),
        _ => None,
    }
}

// Output format

/// keep in sync with `views/settings_editor/component.slint`
pub fn output_format_to_index(fmt: OutputFormat) -> i32 {
    match fmt {
        OutputFormat::Wav => 0,
        OutputFormat::Flac => 1,
        OutputFormat::Mp3 => 2,
        OutputFormat::Ogg => 3,
    }
}

pub fn output_format_from_index(idx: i32) -> OutputFormat {
    match idx {
        1 => OutputFormat::Flac,
        2 => OutputFormat::Mp3,
        3 => OutputFormat::Ogg,
        _ => OutputFormat::Wav,
    }
}

pub fn wav_bit_depth_to_index(depth: WavBitDepth) -> i32 {
    match depth {
        WavBitDepth::Float32 => 0,
        WavBitDepth::Int24 => 1,
        WavBitDepth::Int16 => 2,
    }
}

pub fn wav_bit_depth_from_index(idx: i32) -> WavBitDepth {
    match idx {
        1 => WavBitDepth::Int24,
        2 => WavBitDepth::Int16,
        _ => WavBitDepth::Float32,
    }
}

pub fn bitrate_to_index(kbps: u32) -> i32 {
    BITRATES_KBPS
        .iter()
        .enumerate()
        .min_by_key(|(_, rate)| rate.abs_diff(kbps))
        .map_or(0, |(i, _)| i as i32)
}

pub fn bitrate_from_index(idx: i32) -> u32 {
    usize::try_from(idx)
        .ok()
        .and_then(|i| BITRATES_KBPS.get(i).copied())
        .unwrap_or(DEFAULT_BITRATE_KBPS)
}

pub fn bitrate_labels() -> ModelRc<slint::SharedString> {
    let labels: Vec<slint::SharedString> = BITRATES_KBPS
        .iter()
        .map(|kbps| format!("{kbps} kbps").into())
        .collect();
    ModelRc::new(VecModel::from(labels))
}

// Synth

pub fn synth_to_slint(synth: &SynthConfig) -> SlintSynthConfig {
    let mut out = defaults_slint_synth();

    match synth {
        SynthConfig::FluidSynth(c) => {
            out.kind = SynthKind::Fluidsynth;
            out.fluid_voice_limit = c.voice_limit as i32;
            out.fluid_interpolation = i32::from(c.interpolation);
            out.fluid_min_note_length = c.minimum_note_length as i32;
            out.fluid_system_id = c.system_id;
            out.fluid_chorus = c.chorus_active;
            out.fluid_reverb = c.reverb_active;
            out.fluid_overflow_age = c.overflow_age as f32;
            out.fluid_overflow_percussion = c.overflow_percussion as f32;
            out.fluid_overflow_released = c.overflow_released as f32;
            out.fluid_overflow_sustained = c.overflow_sustained as f32;
            out.fluid_overflow_volume = c.overflow_volume as f32;
        }
        SynthConfig::BASSMIDI(c) => {
            out.kind = SynthKind::Bassmidi;
            out.bass_voice_limit = c.voice_limit as i32;
            out.bass_render_time_limit = c.render_time_limit;
            out.bass_interpolation = i32::from(c.interpolation) + 1;
            out.bass_multithreading = c.multithreading.is_some();
            if let Some(mt) = &c.multithreading {
                out.bass_thread_count = mt.thread_count.unwrap_or(0) as i32;
                out.bass_keyboard_divisions = mt.keyboard_divisions as i32;
            }
            out.bass_disable_effects = c.disable_effects;
            out.bass_fade_out_killing = c.fade_out_killing;
            out.bass_follow_overlaps = c.follow_overlaps;
            out.bass_sf_linear_attack_mod = c.sf_linear_attack_mod;
            out.bass_sf_linear_decay_vol = c.sf_linear_decay_vol;
            out.bass_sf_minfx = c.sf_minfx;
            out.bass_sf_nofx = c.sf_nofx;
            out.bass_sf_no_rampin = c.sf_no_rampin;
            out.bass_sf_sb_limits = c.sf_sb_limits;
            out.bass_sf_xg_drums = c.sf_xg_drums;
        }
    }

    out
}

fn defaults_slint_synth() -> SlintSynthConfig {
    let f = FluidSynthConfig::default();
    let b = BASSMIDIConfig::default();

    SlintSynthConfig {
        kind: SynthKind::Bassmidi,

        fluid_voice_limit: f.voice_limit as i32,
        fluid_interpolation: i32::from(f.interpolation),
        fluid_min_note_length: f.minimum_note_length as i32,
        fluid_system_id: f.system_id,
        fluid_chorus: f.chorus_active,
        fluid_reverb: f.reverb_active,
        fluid_overflow_age: f.overflow_age as f32,
        fluid_overflow_percussion: f.overflow_percussion as f32,
        fluid_overflow_released: f.overflow_released as f32,
        fluid_overflow_sustained: f.overflow_sustained as f32,
        fluid_overflow_volume: f.overflow_volume as f32,

        bass_voice_limit: b.voice_limit as i32,
        bass_render_time_limit: b.render_time_limit,
        bass_interpolation: i32::from(b.interpolation) + 1,
        bass_multithreading: b.multithreading.is_some(),
        bass_thread_count: 0,
        bass_keyboard_divisions: 1,
        bass_disable_effects: b.disable_effects,
        bass_fade_out_killing: b.fade_out_killing,
        bass_follow_overlaps: b.follow_overlaps,
        bass_sf_linear_attack_mod: b.sf_linear_attack_mod,
        bass_sf_linear_decay_vol: b.sf_linear_decay_vol,
        bass_sf_minfx: b.sf_minfx,
        bass_sf_nofx: b.sf_nofx,
        bass_sf_no_rampin: b.sf_no_rampin,
        bass_sf_sb_limits: b.sf_sb_limits,
        bass_sf_xg_drums: b.sf_xg_drums,
    }
}

pub fn slint_to_synth(sl: &SlintSynthConfig) -> SynthConfig {
    match sl.kind {
        SynthKind::Fluidsynth => SynthConfig::FluidSynth(FluidSynthConfig {
            voice_limit: sl.fluid_voice_limit.max(1) as u32,
            interpolation: FluidSynthInterpolation::try_from(sl.fluid_interpolation)
                .unwrap_or_default(),
            minimum_note_length: sl.fluid_min_note_length.max(0) as u32,
            system_id: sl.fluid_system_id,
            chorus_active: sl.fluid_chorus,
            reverb_active: sl.fluid_reverb,
            overflow_age: sl.fluid_overflow_age as f64,
            overflow_percussion: sl.fluid_overflow_percussion as f64,
            overflow_released: sl.fluid_overflow_released as f64,
            overflow_sustained: sl.fluid_overflow_sustained as f64,
            overflow_volume: sl.fluid_overflow_volume as f64,
        }),
        SynthKind::Bassmidi => SynthConfig::BASSMIDI(BASSMIDIConfig {
            voice_limit: sl.bass_voice_limit.max(1) as u32,
            render_time_limit: sl.bass_render_time_limit,
            interpolation: BASSMIDIInterpolation::try_from(sl.bass_interpolation - 1)
                .unwrap_or_default(),
            multithreading: sl.bass_multithreading.then(|| BASSMIDIThreading {
                thread_count: (sl.bass_thread_count > 0).then_some(sl.bass_thread_count as usize),
                keyboard_divisions: sl.bass_keyboard_divisions.clamp(1, 255) as u8,
            }),
            disable_effects: sl.bass_disable_effects,
            fade_out_killing: sl.bass_fade_out_killing,
            follow_overlaps: sl.bass_follow_overlaps,
            sf_linear_attack_mod: sl.bass_sf_linear_attack_mod,
            sf_linear_decay_vol: sl.bass_sf_linear_decay_vol,
            sf_minfx: sl.bass_sf_minfx,
            sf_nofx: sl.bass_sf_nofx,
            sf_no_rampin: sl.bass_sf_no_rampin,
            sf_sb_limits: sl.bass_sf_sb_limits,
            sf_xg_drums: sl.bass_sf_xg_drums,
        }),
    }
}

// Event processor

pub const EVENT_FLAG_SLOTS: usize = 16;

fn indices_to_flags(indices: &[u8]) -> ModelRc<bool> {
    let mut flags = vec![false; EVENT_FLAG_SLOTS];
    for i in indices {
        if let Some(flag) = flags.get_mut(*i as usize) {
            *flag = true;
        }
    }
    ModelRc::new(VecModel::from(flags))
}

fn flags_to_indices(flags: &ModelRc<bool>) -> Box<[u8]> {
    flags
        .iter()
        .enumerate()
        .filter(|(_, on)| *on)
        .map(|(i, _)| i as u8)
        .collect()
}

pub fn evproc_to_slint(e: &EventProcessorConfig) -> SlintEventProcessorConfig {
    SlintEventProcessorConfig {
        bypass_channels: indices_to_flags(&e.bypass_channels),
        ignore_channels: indices_to_flags(&e.ignore_channels),
        bypass_ports: indices_to_flags(&e.bypass_ports),
        ignore_ports: indices_to_flags(&e.ignore_ports),
        fixed_velocity: e.fixed_velocity as i32,
        transpose: e.transpose as i32,
        velocity_multiplier: e.velocity_multiplier,
        velocity_threshold: e.velocity_threshold as i32,
        velocity_curve: e.velocity_curve,
        key_range_low: e.key_range_low as i32,
        key_range_high: e.key_range_high as i32,
        ignore_program_change: e.ignore_program_change,
        ignore_sysex: e.ignore_sysex,
    }
}

pub fn slint_to_evproc(sl: &SlintEventProcessorConfig) -> EventProcessorConfig {
    EventProcessorConfig {
        bypass_ports: flags_to_indices(&sl.bypass_ports),
        ignore_ports: flags_to_indices(&sl.ignore_ports),
        bypass_channels: flags_to_indices(&sl.bypass_channels),
        ignore_channels: flags_to_indices(&sl.ignore_channels),
        fixed_velocity: sl.fixed_velocity.clamp(0, 127) as u8,
        transpose: sl.transpose.clamp(-127, 127) as i8,
        velocity_multiplier: sl.velocity_multiplier,
        velocity_threshold: sl.velocity_threshold.clamp(0, 127) as u8,
        velocity_curve: sl.velocity_curve.max(0.0),
        key_range_low: sl.key_range_low.clamp(0, 127) as u8,
        key_range_high: sl.key_range_high.clamp(0, 127) as u8,
        ignore_program_change: sl.ignore_program_change,
        ignore_sysex: sl.ignore_sysex,
    }
}

// Post processor

pub fn postproc_to_slint(p: &PostProcessorConfig) -> SlintPostProcessorConfig {
    let lim = p.limiter.unwrap_or_default();
    SlintPostProcessorConfig {
        volume: p.volume,
        has_limiter: p.limiter.is_some(),
        limiter_attack_ms: lim.attack_ms,
        limiter_release_ms: lim.release_ms,
    }
}

pub fn slint_to_postproc(sl: &SlintPostProcessorConfig) -> PostProcessorConfig {
    PostProcessorConfig {
        volume: sl.volume,
        limiter: sl.has_limiter.then_some(AudioLimiterConfig {
            attack_ms: sl.limiter_attack_ms,
            release_ms: sl.limiter_release_ms,
        }),
    }
}

// Converter custom

pub fn converter_custom_to_slint(c: &ConverterCustom) -> crate::SlintConverterCustom {
    crate::SlintConverterCustom {
        multithreaded_export: c.multithreaded_export,
        export_threads: c.export_threads as i32,
        output_format: output_format_to_index(c.output_format),
        wav_bit_depth: wav_bit_depth_to_index(c.wav_bit_depth),
        bitrate_index: bitrate_to_index(c.bitrate_kbps),
        output_path: c.output_path.as_str().into(),
    }
}

pub fn slint_to_converter_custom(sl: &crate::SlintConverterCustom) -> ConverterCustom {
    ConverterCustom {
        multithreaded_export: sl.multithreaded_export,
        // 0 threads means "let Rayon size the pool".
        export_threads: sl.export_threads.max(0) as usize,
        output_format: output_format_from_index(sl.output_format),
        wav_bit_depth: wav_bit_depth_from_index(sl.wav_bit_depth),
        bitrate_kbps: bitrate_from_index(sl.bitrate_index),
        output_path: sl.output_path.as_str().to_string(),
    }
}

// System custom

pub fn system_custom_to_slint(s: &crate::state::SystemCustomSettings) -> crate::SlintSystemCustom {
    crate::SlintSystemCustom {
        device_name: s.device_name.as_str().into(),
        num_ports: s.num_ports as i32,
        midi2_enabled: s.midi2_enabled,
        idle_timeout_minutes: s.idle_timeout_minutes as i32,
    }
}

pub fn slint_to_system_custom(sl: &crate::SlintSystemCustom) -> crate::state::SystemCustomSettings {
    crate::state::SystemCustomSettings {
        device_name: sl.device_name.as_str().to_string(),
        num_ports: sl.num_ports.max(1) as u8,
        midi2_enabled: sl.midi2_enabled,
        idle_timeout_minutes: sl.idle_timeout_minutes.max(0) as u32,
    }
}

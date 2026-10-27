#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct KestrelConfig {
    pub gpu: Option<KestrelGpu>,
    pub voice_limit: u32,
    pub interpolation: KestrelInterpolation,
    pub note_grid: bool,
    pub max_steal_percent: u32,

    pub filter: bool,
    pub lfo: bool,
    pub mod_envelope: bool,
    pub decay_curve: KestrelEnvelopeCurve,
    pub release_curve: KestrelEnvelopeCurve,
    pub release_floor_db: f32,
    pub max_layers: u32,
    pub max_param_variants: u32,
    pub gain_ramp: bool,
    pub filter_ramp: bool,

    pub steal_rule: KestrelStealRule,
    pub admit_rule: KestrelAdmitRule,
    pub steal_fade_frames: u32,

    pub resample_pool: bool,
    // 0 sizes it from the GPU's memory
    pub pool_budget_mib: u32,

    // A render block that is not a multiple of the frames adds up to one block of latency
    pub block_frames: u32,
    pub gate_frames: u32,
    pub workgroup_size: u32,
    pub render_workgroups: u32,
    pub submit_voices: u32,
    pub build_threads: u32,
    pub sort_voices: bool,
    pub kahan_reduce: bool,
    pub unchecked_shaders: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct KestrelGpu {
    pub vendor: u32,
    pub device: u32,
    pub backend: KestrelGpuBackend,
}

impl Default for KestrelConfig {
    fn default() -> Self {
        Self {
            gpu: None,
            voice_limit: 65536,
            interpolation: Default::default(),
            note_grid: false,
            max_steal_percent: 25,
            filter: true,
            lfo: true,
            mod_envelope: true,
            decay_curve: Default::default(),
            release_curve: Default::default(),
            release_floor_db: -100.0,
            max_layers: 16,
            max_param_variants: 32,
            gain_ramp: true,
            filter_ramp: true,
            steal_rule: Default::default(),
            admit_rule: Default::default(),
            steal_fade_frames: 96,
            resample_pool: true,
            pool_budget_mib: 0,
            block_frames: 480,
            gate_frames: 32,
            workgroup_size: 256,
            render_workgroups: 2048,
            submit_voices: 4_194_304,
            build_threads: 4,
            sort_voices: true,
            kahan_reduce: false,
            unchecked_shaders: false,
        }
    }
}

impl KestrelConfig {
    pub const MAX_VOICES: u32 = 16_777_216;

    pub fn clamped(&self) -> Self {
        let block_frames = (self.block_frames.clamp(128, 8192) / 32) * 32;

        Self {
            voice_limit: self.voice_limit.clamp(1, Self::MAX_VOICES),
            max_steal_percent: self.max_steal_percent.clamp(1, 100),
            release_floor_db: self.release_floor_db.clamp(-160.0, -20.0),
            max_layers: self.max_layers.clamp(1, 255),
            max_param_variants: self.max_param_variants.clamp(1, 63),
            steal_fade_frames: self.steal_fade_frames.clamp(1, block_frames - 1),
            pool_budget_mib: self.pool_budget_mib.min(65536),
            block_frames,
            gate_frames: self.gate_frames.clamp(4, 32).next_power_of_two(),
            workgroup_size: self.workgroup_size.clamp(32, 1024).next_power_of_two(),
            render_workgroups: self.render_workgroups.clamp(1, 65535),
            submit_voices: self.submit_voices.clamp(1, Self::MAX_VOICES),
            build_threads: self.build_threads.clamp(1, 64),
            ..*self
        }
    }
}

macro_rules! index_enum {
    ($name:ident { $($variant:ident = $i:literal),+ $(,)? } default $default:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
        #[repr(i32)]
        pub enum $name {
            $($variant = $i,)+
        }

        impl Default for $name {
            fn default() -> Self {
                Self::$default
            }
        }

        impl TryFrom<i32> for $name {
            type Error = String;

            fn try_from(value: i32) -> Result<Self, Self::Error> {
                match value {
                    $($i => Ok(Self::$variant),)+
                    _ => Err(format!(concat!("Unsupported ", stringify!($name), ": {}"), value)),
                }
            }
        }

        impl From<$name> for i32 {
            fn from(value: $name) -> Self {
                value as i32
            }
        }
    };
}

index_enum!(KestrelInterpolation { Nearest = 0, Linear = 1, Cubic = 2 } default Linear);
index_enum!(KestrelEnvelopeCurve { Exponential = 0, Linear = 1 } default Exponential);
index_enum!(KestrelStealRule { Quietest = 0, Oldest = 1, DropNew = 2 } default Quietest);
index_enum!(KestrelAdmitRule { Loudest = 0, Even = 1 } default Loudest);
index_enum!(KestrelGpuBackend { Vulkan = 0, Dx12 = 1, Metal = 2, Gl = 3 } default Vulkan);

use std::sync::OnceLock;

use crate::renderer::config::kestrel::{KestrelConfig, KestrelGpu, KestrelGpuBackend};

#[derive(Clone, Debug)]
pub struct KestrelAdapter {
    pub gpu: KestrelGpu,
    pub name: String,
    pub integrated: bool,
    pub max_voices: u32,
}

struct Surveyed {
    gpu: KestrelGpu,
    name: String,
    integrated: bool,
    binding_bytes: u64,
}

fn surveyed() -> &'static (Vec<Surveyed>, Option<usize>) {
    // opening every backend is slow, so the gpus are only looked for once
    static LIST: OnceLock<(Vec<Surveyed>, Option<usize>)> = OnceLock::new();
    LIST.get_or_init(|| {
        let Ok((found, pick)) = kestrel::gpu::survey(&kestrel::Config::default()) else {
            return Default::default();
        };
        let mut auto = None;
        let mut list = Vec::new();
        for (i, a) in found.into_iter().enumerate() {
            let backend = match format!("{:?}", a.backend).as_str() {
                "Vulkan" => KestrelGpuBackend::Vulkan,
                "Dx12" => KestrelGpuBackend::Dx12,
                "Metal" => KestrelGpuBackend::Metal,
                "Gl" => KestrelGpuBackend::Gl,
                _ => continue,
            };
            if a.is_software() {
                continue;
            }
            if pick == Some(i) {
                auto = Some(list.len());
            }
            list.push(Surveyed {
                gpu: KestrelGpu {
                    vendor: a.vendor,
                    device: a.device,
                    backend,
                },
                integrated: format!("{:?}", a.device_type) == "IntegratedGpu",
                name: a.name,
                binding_bytes: a.binding_bytes,
            });
        }
        (list, auto)
    })
}

pub(super) fn backend_word(backend: KestrelGpuBackend) -> &'static str {
    match backend {
        KestrelGpuBackend::Vulkan => "vulkan",
        KestrelGpuBackend::Dx12 => "dx12",
        KestrelGpuBackend::Metal => "metal",
        KestrelGpuBackend::Gl => "gl",
    }
}

pub(super) fn resolve(gpu: Option<KestrelGpu>, cfg: &kestrel::Config) -> Option<KestrelAdapter> {
    let (list, auto) = surveyed();
    let found = gpu
        .and_then(|g| list.iter().find(|s| s.gpu == g))
        .or_else(|| auto.map(|i| &list[i]))?;
    Some(KestrelAdapter {
        gpu: found.gpu,
        name: found.name.clone(),
        integrated: found.integrated,
        max_voices: kestrel::gpu::max_voices_for_config(found.binding_bytes, cfg),
    })
}

pub fn kestrel_adapters(config: &KestrelConfig) -> (Vec<KestrelAdapter>, Option<usize>) {
    let cfg = super::synth::base_config(&config.clamped(), 48000);
    let (list, auto) = surveyed();
    let adapters = list
        .iter()
        .map(|s| KestrelAdapter {
            gpu: s.gpu,
            name: s.name.clone(),
            integrated: s.integrated,
            max_voices: kestrel::gpu::max_voices_for_config(s.binding_bytes, &cfg),
        })
        .collect();
    (adapters, *auto)
}

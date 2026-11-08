use anyhow::{Result, bail};
use norrix_runtime::{Backend, Device, Hardware};
use norrix_torch::{Cuda, Kind, nn};

pub(crate) trait TryIntoDevice<T> {
    fn try_into_device(self) -> Result<T>;
}

impl TryIntoDevice<norrix_torch::Device> for Device {
    fn try_into_device(self) -> Result<norrix_torch::Device> {
        match &self.backend {
            Backend::Cpu => Ok(norrix_torch::Device::Cpu),
            Backend::Cuda | Backend::Rocm => {
                // PyTorch's cuDNN switch controls MIOpen on ROCm. MIOpen is not reliable
                // on Windows and causes runtime issues, so disable it there.
                Cuda::set_user_enabled_cudnn(if cfg!(windows) {
                    matches!(&self.backend, Backend::Cuda)
                } else {
                    true
                });
                Ok(norrix_torch::Device::Cuda(self.index))
            }
            Backend::Vulkan if self.index == 0 => Ok(if norrix_torch::utils::has_vulkan() {
                norrix_torch::Device::Vulkan
            } else {
                norrix_torch::Device::Cpu
            }),
            Backend::Metal
                if self.index == 0 && cfg!(all(target_os = "macos", target_arch = "aarch64")) =>
            {
                Ok(norrix_torch::Device::Mps)
            }
            Backend::Vulkan | Backend::Metal => {
                bail!(
                    "Torch cannot address {} device index {}",
                    self.backend,
                    self.index
                )
            }
            Backend::Other(_) => bail!(
                "the {} backend cannot be represented by a Torch device",
                self.backend
            ),
        }
    }
}

pub(crate) fn set_precision(var_store: &mut nn::VarStore) {
    let hardware = Hardware::discover();
    let device_supports_bfloat16 = hardware
        .device()
        .is_some_and(|device| match &device.backend {
            Backend::Cuda => device.compute_capability() >= 80,
            Backend::Rocm => device.target().is_some_and(|target| {
                matches!(target, "gfx908" | "gfx90a" | "gfx942" | "gfx950")
                    || target.starts_with("gfx11")
                    || target.starts_with("gfx12")
            }),
            Backend::Cpu | Backend::Vulkan | Backend::Metal | Backend::Other(_) => false,
        });
    let use_bfloat16 =
        matches!(var_store.device(), norrix_torch::Device::Cuda(_)) && device_supports_bfloat16;

    if var_store.is_empty() {
        var_store.set_kind(if use_bfloat16 {
            Kind::BFloat16
        } else {
            Kind::Float
        });
    } else if use_bfloat16 {
        var_store.bfloat16();
    } else {
        var_store.float();
    }
}

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use strum::EnumProperty;
use walkdir::WalkDir;

use crate::{
    Hardware, Store, download,
    runtime::{
        DiscoverablePackage, Package, RuntimePackage, graph::Component, loader, packages::Cuda,
        sealed,
    },
    source::extract,
};

const RELEASE: &str = "v2.13.0.7";
const INTEGRITY_FILE: &str = ".norrix-torch-integrity";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, strum::Display, strum::EnumProperty)]
pub enum Torch {
    // Keep macOS root-first so dyld resolves LibTorch's weak C++ symbols inside
    // one private RTLD_LOCAL image group instead of publishing them globally.
    #[cfg_attr(target_os = "macos", strum(serialize = "metal"))]
    #[cfg_attr(not(target_os = "macos"), strum(serialize = "cpu"))]
    #[strum(props(
        windows = "libiomp5md.dll,c10.dll,torch_global_deps.dll,torch_cpu.dll,torch.dll,norrix-torch.dll",
        linux = "libgomp.so.1,libc10.so,libtorch_global_deps.so,libtorch_cpu.so,libtorch.so,libnorrix-torch.so",
        macos = "libtorch.dylib,libtorch_global_deps.dylib,libtorch_cpu.dylib,libc10.dylib,libnorrix-torch.dylib"
    ))]
    Cpu,
    #[strum(
        serialize = "cuda",
        props(
            windows = "c10.dll,c10_cuda.dll,caffe2_nvrtc.dll,torch_global_deps.dll,torch_cpu.dll,torch_cuda.dll,torch.dll,norrix-torch.dll",
            linux = "libc10.so,libc10_cuda.so,libcaffe2_nvrtc.so,libtorch_global_deps.so,libtorch_cpu.so,libtorch_cuda.so,libtorch.so,libnorrix-torch.so"
        )
    )]
    Cuda,
    #[strum(
        serialize = "hip",
        props(
            windows = "c10.dll,c10_hip.dll,caffe2_nvrtc.dll,torch_global_deps.dll,torch_cpu.dll,torch_hip.dll,torch.dll,norrix-torch.dll",
            linux = "libc10.so,libc10_hip.so,libcaffe2_nvrtc.so,libtorch_global_deps.so,libtorch_cpu.so,libtorch_hip.so,libtorch.so,libnorrix-torch.so"
        )
    )]
    Rocm,
}

impl Torch {
    fn shim_names() -> (&'static str, &'static str) {
        if cfg!(target_os = "windows") {
            ("koharu-torch.dll", "norrix-torch.dll")
        } else if cfg!(target_os = "macos") {
            ("libkoharu-torch.dylib", "libnorrix-torch.dylib")
        } else {
            ("libkoharu-torch.so", "libnorrix-torch.so")
        }
    }

    async fn normalize_shim_name(root: &Path) -> Result<()> {
        let (legacy, current) = Self::shim_names();
        let legacy_path = root.join(legacy);
        let current_path = root.join(current);
        if legacy_path.is_file() && !current_path.exists() {
            tokio::fs::rename(&legacy_path, &current_path)
                .await
                .with_context(|| {
                    format!(
                        "failed to rename Torch shim {} to {}",
                        legacy_path.display(),
                        current_path.display()
                    )
                })?;
        }
        anyhow::ensure!(
            current_path.is_file(),
            "Torch package is missing shim library {}",
            current_path.display()
        );
        Ok(())
    }

    pub fn library_names(self) -> Result<impl Iterator<Item = &'static str>> {
        let property = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "macos"
        } else {
            anyhow::bail!("Torch does not support this target")
        };
        Ok(self
            .get_str(property)
            .with_context(|| format!("Torch {self} does not support this target"))?
            .split(','))
    }

    fn complete(self, root: &Path) -> bool {
        let Ok((_, archive_sha256)) = self.asset() else {
            return false;
        };
        let Ok(contents) = std::fs::read_to_string(root.join(INTEGRITY_FILE)) else {
            return false;
        };
        let mut lines = contents.lines();
        if lines.next() != Some(archive_sha256) {
            return false;
        }
        let Ok(names) = self.library_names() else {
            return false;
        };
        if names.into_iter().any(|name| !root.join(name).is_file()) {
            return false;
        }
        let mut manifest_files = std::collections::HashMap::new();
        for line in lines {
            let Some((name, expected_sha256)) = line.split_once('=') else {
                return false;
            };
            if manifest_files
                .insert(PathBuf::from(name), expected_sha256)
                .is_some()
            {
                return false;
            }
        }
        let Ok(files) = Self::library_files(root) else {
            return false;
        };
        if files.len() != manifest_files.len() {
            return false;
        }
        for path in files {
            let Some(expected_sha256) = manifest_files.remove(&path) else {
                return false;
            };
            let Ok(actual_sha256) = download::sha256_file(&root.join(&path)) else {
                return false;
            };
            if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
                return false;
            }
        }
        manifest_files.is_empty()
    }

    fn asset(self) -> Result<(String, &'static str)> {
        let target = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            "x86_64-pc-windows-msvc"
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            "x86_64-unknown-linux-gnu"
        } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            "aarch64-unknown-linux-gnu"
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "aarch64-apple-darwin"
        } else {
            anyhow::bail!("Torch {self} does not support this target")
        };
        let name = format!("{target}-{self}.tar.gz");
        let sha256 = match name.as_str() {
            "aarch64-apple-darwin-metal.tar.gz" => {
                "cc13ec11bd014eb334595e6255ec450f8a3bbc6d8c90ff87fb04769066163754"
            }
            "aarch64-unknown-linux-gnu-cuda.tar.gz" => {
                "10b59e19b60f4f65296a72bf03e5a98125d692184e4dca0a57298ba5b3d0a0d9"
            }
            "x86_64-pc-windows-msvc-cpu.tar.gz" => {
                "db6917ea59ef4a3d5eff1d4f4232c364768296f3c98bad50713b93e3594da135"
            }
            "x86_64-pc-windows-msvc-cuda.tar.gz" => {
                "6ac600e10eac53e45b9a8ca4c3d04771a0423c0b2e70adf422df46ff07766f8d"
            }
            "x86_64-pc-windows-msvc-hip.tar.gz" => {
                "4cf2b9861507a199bf57fc3e0bf4182a61438161b8ad13906ac946c859be498c"
            }
            "x86_64-unknown-linux-gnu-cpu.tar.gz" => {
                "467c182f60b15c8aae0db4e56e0b23652e6a0ef9d0daed3e10d7e96434ca5f20"
            }
            "x86_64-unknown-linux-gnu-cuda.tar.gz" => {
                "12498b1fe8cc5e7132911211f4c68b94be7a34ebfab4da8b5dc39f0b07b65134"
            }
            "x86_64-unknown-linux-gnu-hip.tar.gz" => {
                "79e8641ddcb0bd33f7f5b2ad651c8b60a48b05f90abd68aa8c4306aa955e268e"
            }
            _ => anyhow::bail!("Torch asset {name} has no pinned SHA-256 digest"),
        };
        Ok((name, sha256))
    }

    fn integrity_manifest(self, root: &Path) -> Result<String> {
        let (_, archive_sha256) = self.asset()?;
        let mut lines = vec![archive_sha256.to_owned()];
        for name in self.library_names()? {
            anyhow::ensure!(root.join(name).is_file(), "Torch package is missing {name}");
        }
        for path in Self::library_files(root)? {
            let name = path
                .to_str()
                .context("Torch library path is not valid UTF-8")?;
            lines.push(format!(
                "{name}={}",
                download::sha256_file(&root.join(&path))?
            ));
        }
        Ok(format!("{}\n", lines.join("\n")))
    }

    fn library_files(root: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        for entry in WalkDir::new(root) {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if name.ends_with(".dll") || name.ends_with(".dylib") || name.contains(".so") {
                files.push(path.strip_prefix(root)?.to_owned());
            }
        }
        files.sort();
        Ok(files)
    }
}

impl sealed::Sealed for Torch {}

impl Package for Torch {
    async fn install(self) -> Result<PathBuf> {
        let path = Store::root()
            .join("torch")
            .join(RELEASE)
            .join(self.to_string());
        let (asset, sha256) = self.asset()?;
        let sha256 = sha256.to_owned();

        Store::directory(
            path,
            move |path| self.complete(path),
            move |stage| async move {
                let url = format!(
                    "https://github.com/koharu-rs/torch/releases/download/{RELEASE}/{asset}"
                );
                let archive = tempfile::Builder::new().suffix(".tar.gz").tempfile()?;
                download::fetch(&url, archive.path()).await?;
                download::verify_sha256(archive.path().to_owned(), sha256.clone()).await?;
                extract(
                    archive.path(),
                    &stage,
                    &["**/*.dll", "**/*.dylib", "**/*.so", "**/*.so.*"],
                )?;
                Self::normalize_shim_name(&stage).await?;
                let manifest_root = stage.clone();
                let manifest =
                    tokio::task::spawn_blocking(move || self.integrity_manifest(&manifest_root))
                        .await
                        .context("Torch integrity manifest task failed")??;
                tokio::fs::write(stage.join(INTEGRITY_FILE), manifest).await?;
                Ok(())
            },
        )
        .await
    }
}

impl DiscoverablePackage for Torch {
    fn discover(hardware: &Hardware) -> Option<Self> {
        if hardware.supports_metal() {
            return Some(Self::Cpu);
        }
        if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            return hardware.supports_cuda().then_some(Self::Cuda);
        }
        if !cfg!(any(
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64")
        )) {
            return None;
        }
        if hardware.supports_cuda() {
            return Some(Self::Cuda);
        }
        if hardware.supports_rocm() {
            return Some(Self::Rocm);
        }
        tracing::warn!("no supported Torch accelerator was discovered; using CPU");
        Some(Self::Cpu)
    }
}

impl RuntimePackage for Torch {
    const NAME: &'static str = "Torch";

    fn dependencies(self, hardware: &Hardware) -> Result<Vec<Component>> {
        match self {
            Self::Cpu => Ok(Vec::new()),
            Self::Rocm => Ok(vec![Component::Rocm(hardware.rocm_target()?)]),
            Self::Cuda => {
                let packages = [
                    Cuda::Runtime13,
                    Cuda::JitLink13,
                    Cuda::Rtc13,
                    Cuda::Blas13,
                    Cuda::Fft12,
                    Cuda::Rand10,
                    Cuda::Sparse12,
                    Cuda::Solver12,
                    Cuda::Dnn925,
                ];
                Ok(packages.into_iter().map(Component::Cuda).collect())
            }
        }
    }

    async fn activate(self) -> Result<()> {
        let directory = self.install().await?;
        for library in self.library_names()? {
            loader::load(directory.join(library), false)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_without_cuda_selects_cpu_and_cuda_hardware_selects_cuda() {
        let cpu = Hardware::default();
        assert_eq!(Torch::discover(&cpu), Some(Torch::Cpu));

        let cuda = Hardware {
            devices: vec![crate::Device::cuda(0)],
            candidates: vec![0],
            selected: Some(0),
        };
        assert_eq!(Torch::discover(&cuda), Some(Torch::Cuda));

        let unsupported_accelerator = Hardware {
            devices: vec![crate::Device::vulkan(0)],
            candidates: vec![0],
            selected: Some(0),
        };
        assert_eq!(Torch::discover(&unsupported_accelerator), Some(Torch::Cpu));
    }

    #[tokio::test]
    async fn legacy_shim_name_is_normalized_for_runtime_loading() {
        let directory = tempfile::tempdir().unwrap();
        let (legacy, current) = Torch::shim_names();
        let legacy_path = directory.path().join(legacy);
        let current_path = directory.path().join(current);
        tokio::fs::write(&legacy_path, b"shim").await.unwrap();

        Torch::normalize_shim_name(directory.path()).await.unwrap();

        assert_eq!(tokio::fs::read(&current_path).await.unwrap(), b"shim");
        assert!(!legacy_path.exists());
    }

    #[test]
    fn cache_manifest_detects_missing_or_modified_torch_libraries() {
        let directory = tempfile::tempdir().unwrap();
        for name in Torch::Cpu.library_names().unwrap() {
            std::fs::write(directory.path().join(name), b"test library").unwrap();
        }
        let supporting_library = directory.path().join("support").join("dependency.dll");
        std::fs::create_dir_all(supporting_library.parent().unwrap()).unwrap();
        std::fs::write(&supporting_library, b"support library").unwrap();
        let manifest = Torch::Cpu.integrity_manifest(directory.path()).unwrap();
        std::fs::write(directory.path().join(INTEGRITY_FILE), manifest).unwrap();
        assert!(Torch::Cpu.complete(directory.path()));

        std::fs::write(&supporting_library, b"modified support library").unwrap();
        assert!(!Torch::Cpu.complete(directory.path()));
        std::fs::write(&supporting_library, b"support library").unwrap();

        let first_library = Torch::Cpu.library_names().unwrap().next().unwrap();
        std::fs::write(directory.path().join(first_library), b"modified library").unwrap();
        assert!(!Torch::Cpu.complete(directory.path()));

        std::fs::remove_file(directory.path().join(first_library)).unwrap();
        assert!(!Torch::Cpu.complete(directory.path()));
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[tokio::test]
    #[ignore = "downloads the pinned 75 MB Windows CPU runtime from GitHub"]
    async fn real_windows_cpu_package_downloads_verifies_reuses_and_loads() {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "norrix-torch-real-test-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        let directory = tempfile::TempDir::new_in(&path).unwrap();
        let root = directory.path().to_owned();
        std::mem::forget(directory);
        Store::configure(root.clone()).unwrap();

        let mut events = download::subscribe();
        let installed = Torch::Cpu.install().await.unwrap();
        assert!(Torch::Cpu.complete(&installed));
        Torch::Cpu.activate().await.unwrap();

        let mut started = 0;
        while let Ok(event) = events.try_recv() {
            if matches!(event, download::Event::Started { .. }) {
                started += 1;
            }
        }
        assert_eq!(started, 1, "the first install should download one archive");

        assert_eq!(Torch::Cpu.install().await.unwrap(), installed);
        assert!(
            matches!(
                events.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "a verified cache hit must not start another download"
        );
        eprintln!(
            "Verified real Windows Torch runtime at {}",
            installed.display()
        );
    }
}

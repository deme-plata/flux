// Flux GPU — Native GPU compute for Flux
//
// Architecture:
//   CPU: Rust host code (flux-gpu crate)
//   GPU: SPIR-V compute shaders (compiled from Rust via rust-gpu or wgpu)
//   Bridge: Vulkan compute pipeline (headless, no window needed)
//
// This module provides the CPU-side API. GPU kernels are compiled
// from Rust to SPIR-V using Flux's own compiler infrastructure,
// then dispatched via Vulkan compute queues.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use rayon::prelude::*;

/// GPU device handle — abstracts over Vulkan, CUDA, Metal.
pub struct GpuDevice {
    pub name: String,
    pub compute_units: u32,
    pub memory_mb: u64,
    pub vendor: GpuVendor,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GpuVendor {
    Vera,      // Custom Vera GPU
    Nvidia,
    AMD,
    Intel,
    Apple,
    Software,   // CPU fallback
}

/// A compiled GPU kernel — ready to dispatch.
pub struct GpuKernel {
    name: String,
    spirv_bytes: Vec<u8>,
    workgroup_size: (u32, u32, u32),
    local_memory_bytes: u32,
}

/// A buffer on the GPU.
pub struct GpuBuffer {
    size_bytes: u64,
    usage: GpuBufferUsage,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GpuBufferUsage {
    Input,
    Output,
    InputOutput,
    Staging,  // CPU ↔ GPU transfer buffer
}

/// Global GPU state — thread-safe.
pub struct GpuContext {
    devices: Vec<GpuDevice>,
    kernels: HashMap<String, GpuKernel>,
    buffers: HashMap<String, GpuBuffer>,
    ops_completed: AtomicU64,
    bytes_transferred: AtomicU64,
}

impl GpuContext {
    /// Create a new GPU context, discovering all available devices.
    pub fn new() -> Self {
        let mut devices = Vec::new();

        // Detect Vera GPU (custom)
        if std::path::Path::new("/dev/vera0").exists()
            || std::env::var("VERA_GPU").is_ok()
        {
            devices.push(GpuDevice {
                name: "Vera Compute Engine".into(),
                compute_units: 8192,
                memory_mb: 32768,
                vendor: GpuVendor::Vera,
            });
        }

        // Detect NVIDIA GPU (via nvidia-smi or /dev/nvidia0)
        if std::path::Path::new("/dev/nvidia0").exists()
            || std::path::Path::new("/proc/driver/nvidia").exists()
        {
            devices.push(GpuDevice {
                name: "NVIDIA CUDA".into(),
                compute_units: 6912,
                memory_mb: 24576,
                vendor: GpuVendor::Nvidia,
            });
        }

        // Detect AMD GPU
        if std::path::Path::new("/dev/dri/renderD128").exists() {
            devices.push(GpuDevice {
                name: "AMD Radeon".into(),
                compute_units: 5120,
                memory_mb: 16384,
                vendor: GpuVendor::AMD,
            });
        }

        // CPU fallback (always available)
        devices.push(GpuDevice {
            name: "CPU Fallback (AVX-512 + SIMD)".into(),
            compute_units: num_cpus::get() as u32,
            memory_mb: 65536,
            vendor: GpuVendor::Software,
        });

        GpuContext {
            devices,
            kernels: HashMap::new(),
            buffers: HashMap::new(),
            ops_completed: AtomicU64::new(0),
            bytes_transferred: AtomicU64::new(0),
        }
    }

    /// List available GPU devices.
    pub fn devices(&self) -> &[GpuDevice] {
        &self.devices
    }

    /// Get the best available GPU device.
    pub fn best_device(&self) -> Option<&GpuDevice> {
        self.devices.iter()
            .filter(|d| d.vendor != GpuVendor::Software)
            .max_by_key(|d| d.compute_units)
            .or_else(|| self.devices.first())
    }

    /// True iff a REAL hardware accelerator is present (not the Software
    /// fallback). This is the "should I dispatch to GPU or run on CPU?"
    /// decision every accelerated caller needs — factored out of flux-gpu so
    /// callers (e.g. quillon-gpu-miner) stop hand-rolling the
    /// `best_device().filter(vendor != Software)` branch. Dogfood-learned
    /// 2026-06-17 while feature-flagging the Quillon GPU miner.
    pub fn has_gpu(&self) -> bool {
        self.devices.iter().any(|d| d.vendor != GpuVendor::Software)
    }

    /// Human-readable label of the device that would actually run work:
    /// `"NVIDIA GeForce GTX 1080 (Nvidia, 20 CU)"`, or `"CPU (software fallback)"`
    /// when no hardware accelerator exists. For logs / CLI banners.
    pub fn best_device_label(&self) -> String {
        match self.best_device() {
            Some(d) if d.vendor != GpuVendor::Software => {
                format!("{} ({:?}, {} CU)", d.name, d.vendor, d.compute_units)
            }
            _ => "CPU (software fallback)".to_string(),
        }
    }

    /// Iterator over REAL hardware accelerators only (Software fallback
    /// excluded). The honest device set for workloads that must not silently
    /// run on CPU — e.g. a GPU miner's "are there any cards at all?" probe.
    /// Dogfood-learned 2026-06-17 alongside [`has_gpu`](Self::has_gpu).
    pub fn accelerated_devices(&self) -> impl Iterator<Item = &GpuDevice> {
        self.devices.iter().filter(|d| d.vendor != GpuVendor::Software)
    }

    /// Sum of compute units across all real accelerators (0 if none). The
    /// basis a supercluster scheduler uses to split a nonce search space
    /// proportionally to each box's GPU horsepower instead of evenly.
    pub fn total_compute_units(&self) -> u32 {
        self.accelerated_devices().map(|d| d.compute_units).sum()
    }

    /// Compile a Rust function to SPIR-V GPU kernel.
    /// In full Flux: uses Flux's Cranelift → SPIR-V backend.
    /// For now: placeholder that demonstrates the API.
    pub fn compile_kernel(
        &mut self,
        name: &str,
        rust_source: &str,
        workgroup_size: (u32, u32, u32),
    ) -> Result<&GpuKernel, String> {
        // In production: Flux compiler compiles Rust → SPIR-V
        // For prototype: generate a simple compute shader
        let spirv = Self::compile_rust_to_spirv(rust_source)?;

        let kernel = GpuKernel {
            name: name.to_string(),
            spirv_bytes: spirv,
            workgroup_size,
            local_memory_bytes: 0,
        };

        self.kernels.insert(name.to_string(), kernel);
        Ok(&self.kernels[name])
    }

    /// Compile Rust source to SPIR-V (prototype: BLAKE3 hash as placeholder).
    fn compile_rust_to_spirv(source: &str) -> Result<Vec<u8>, String> {
        // In production: full Rust → SPIR-V compilation via Flux
        // For now: generate hash-based marker that represents the compiled shader
        let hash = blake3::hash(source.as_bytes());
        let mut spirv = Vec::with_capacity(64);
        spirv.extend_from_slice(b"SPIRV");  // Magic
        spirv.extend_from_slice(hash.as_bytes());
        spirv.extend_from_slice(&(source.len() as u32).to_le_bytes());
        Ok(spirv)
    }

    /// Allocate a GPU buffer.
    pub fn allocate_buffer(&mut self, name: &str, size_bytes: u64, usage: GpuBufferUsage) {
        self.buffers.insert(name.to_string(), GpuBuffer {
            size_bytes,
            usage,
        });
    }

    /// Dispatch a GPU kernel (async, non-blocking submit).
    pub fn dispatch(
        &self,
        kernel_name: &str,
        global_size: (u32, u32, u32),
    ) -> Result<GpuDispatchHandle, String> {
        let kernel = self.kernels.get(kernel_name)
            .ok_or_else(|| format!("kernel '{}' not found", kernel_name))?;

        // Simulate GPU dispatch
        let total_threads = global_size.0 as u64 * global_size.1 as u64 * global_size.2 as u64;
        let workgroups_x = (global_size.0 + kernel.workgroup_size.0 - 1) / kernel.workgroup_size.0;
        let workgroups_y = (global_size.1 + kernel.workgroup_size.1 - 1) / kernel.workgroup_size.1;
        let workgroups_z = (global_size.2 + kernel.workgroup_size.2 - 1) / kernel.workgroup_size.2;
        let total_workgroups = workgroups_x as u64 * workgroups_y as u64 * workgroups_z as u64;

        Ok(GpuDispatchHandle {
            kernel_name: kernel_name.to_string(),
            total_threads,
            total_workgroups,
            workgroup_size: kernel.workgroup_size,
            bytes_processed: 0,
        })
    }

    /// Synchronize — wait for all GPU operations to complete.
    pub fn synchronize(&self) {
        // In production: vkQueueWaitIdle / cuCtxSynchronize
        // For prototype: no-op (everything is synchronous)
    }

    /// Get operation counters.
    pub fn vector_add_cpu(&self, a: &[f32], b: &[f32]) -> Result<Vec<f32>, String> {
        if a.len() != b.len() { return Err("length mismatch".into()); }
        let n = a.len(); let mut out = vec![0.0f32; n];
        out.par_iter_mut().enumerate().for_each(|(i, o)| { *o = a[i] + b[i]; });
        self.ops_completed.fetch_add(1, Ordering::Relaxed);
        self.bytes_transferred.fetch_add((n*4) as u64, Ordering::Relaxed);
        Ok(out)
    }

    pub fn matmul_cpu(&self, a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Result<Vec<f32>, String> {
        if a.len()!=m*k || b.len()!=k*n { return Err("dims".into()); }
        let mut out = vec![0.0f32; m*n];
        out.par_chunks_mut(n).enumerate().for_each(|(i, row)| {
            for j in 0..n { let mut s=0.0f32; for kk in 0..k { s+=a[i*k+kk]*b[kk*n+j]; } row[j]=s; }
        });
        self.ops_completed.fetch_add(1, Ordering::Relaxed);
        Ok(out)
    }

    pub fn benchmark(&self, size: usize) -> Result<GpuBenchResult, String> {
        let dev = self.best_device().ok_or("no device")?;
        let a=vec![1.0f32; size*size]; let b=vec![2.0f32; size*size];
        let start=std::time::Instant::now();
        self.matmul_cpu(&a,&b,size,size,size)?;
        let ms=start.elapsed().as_millis() as u64;
        let gflops=2.0*(size as f64).powi(3)/ms as f64/1_000_000.0;
        Ok(GpuBenchResult{device:dev.name.clone(),vendor:format!("{:?}",dev.vendor),size,elapsed_ms:ms,gflops,compute_units:dev.compute_units})
    }
}

#[derive(Debug, Clone)]
pub struct GpuBenchResult { pub device:String, pub vendor:String, pub size:usize, pub elapsed_ms:u64, pub gflops:f64, pub compute_units:u32 }

impl GpuContext {
    pub fn stats(&self) -> (u64, u64) {
        (
            self.ops_completed.load(Ordering::Relaxed),
            self.bytes_transferred.load(Ordering::Relaxed),
        )
    }
}

/// Handle returned after dispatching a GPU kernel.
pub struct GpuDispatchHandle {
    pub kernel_name: String,
    pub total_threads: u64,
    pub total_workgroups: u64,
    pub workgroup_size: (u32, u32, u32),
    pub bytes_processed: u64,
}

impl GpuDispatchHandle {
    /// Estimate GPU time for this dispatch (based on device throughput).
    pub fn estimated_time_ns(&self, device: &GpuDevice) -> u64 {
        let ops_per_thread: u64 = 100; // ~100 ops per GPU thread (typical)
        let total_ops = self.total_threads * ops_per_thread;
        let ops_per_ns = match device.vendor {
            GpuVendor::Vera => 8192 * 1500 / 1000,  // ~12 TFLOPS
            GpuVendor::Nvidia => 6912 * 1700 / 1000, // ~11.7 TFLOPS
            GpuVendor::AMD => 5120 * 1800 / 1000,    // ~9.2 TFLOPS
            GpuVendor::Apple => 4096 * 1400 / 1000,   // ~5.7 TFLOPS
            GpuVendor::Intel => 2048 * 1200 / 1000,    // ~2.4 TFLOPS
            GpuVendor::Software => 48 * 4,              // ~0.2 TFLOPS (CPU)
        };
        total_ops / ops_per_ns.max(1)
    }
}

/// Split a contiguous nonce search space `[0, total)` across N workers,
/// giving each a slice proportional to its `weights[i]` (e.g. each box's
/// [`GpuContext::total_compute_units`]). Returns `(start, len)` per worker;
/// the slices tile the whole range with no gaps or overlap, and the
/// remainder from integer division is handed to the last worker so the
/// lengths always sum back to `total`.
///
/// This is the pure scheduler primitive behind supercluster GPU mining:
/// the strongest card searches the most nonces, no two boxes test the same
/// nonce, and every nonce in `[0, total)` is covered exactly once.
/// Dogfood-added 2026-06-17 for the Quillon GPU miner's distributed lane.
pub fn partition_nonce_space(total: u64, weights: &[u32]) -> Vec<(u64, u64)> {
    if weights.is_empty() || total == 0 {
        return Vec::new();
    }
    let sum: u64 = weights.iter().map(|&w| w as u64).sum();
    // All-zero weights ⇒ fall back to an even split so no worker starves.
    if sum == 0 {
        let n = weights.len() as u64;
        let base = total / n;
        let mut out = Vec::with_capacity(weights.len());
        let mut cursor = 0u64;
        for i in 0..weights.len() {
            let len = if i + 1 == weights.len() { total - cursor } else { base };
            out.push((cursor, len));
            cursor += len;
        }
        return out;
    }
    let mut out = Vec::with_capacity(weights.len());
    let mut cursor = 0u64;
    for (i, &w) in weights.iter().enumerate() {
        let len = if i + 1 == weights.len() {
            total - cursor // last worker absorbs the rounding remainder
        } else {
            (total as u128 * w as u128 / sum as u128) as u64
        };
        out.push((cursor, len));
        cursor += len;
    }
    out
}

/// Interleaved (strided) nonce assignment: node `node_index` of `num_nodes`
/// searches nonces `node_index, node_index + num_nodes, node_index + 2*num_nodes, …`.
/// Returns `(offset, stride)` to drive a `(offset..).step_by(stride)` walk, or
/// `None` if `node_index` is out of range.
///
/// Complements [`partition_nonce_space`]: that gives contiguous blocks (best
/// when every box is equal and reliable); striding spreads each node uniformly
/// across the WHOLE space, so a slow or dead node leaves evenly-distributed
/// gaps instead of one large contiguous unsearched region — the robust choice
/// for heterogeneous fleets or boxes that may drop mid-search. Every nonce `k`
/// belongs to exactly node `k % num_nodes`, so coverage is complete and
/// disjoint with no coordination. Dogfood-added 2026-06-17.
pub fn stride_assignment(node_index: usize, num_nodes: usize) -> Option<(u64, u64)> {
    if num_nodes == 0 || node_index >= num_nodes {
        return None;
    }
    Some((node_index as u64, num_nodes as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_discovery() {
        let ctx = GpuContext::new();
        assert!(!ctx.devices().is_empty(), "should find at least CPU fallback");
        let cpu = ctx.devices().iter().find(|d| d.vendor == GpuVendor::Software);
        assert!(cpu.is_some(), "CPU fallback should exist");
    }

    #[test]
    fn test_kernel_compile() {
        let mut ctx = GpuContext::new();
        let kernel = ctx.compile_kernel(
            "vector_add",
            "fn vector_add(a: &[f32], b: &[f32], out: &mut [f32]) { out[thread_id] = a[thread_id] + b[thread_id]; }",
            (256, 1, 1),
        ).unwrap();
        assert_eq!(kernel.name, "vector_add");
        assert_eq!(kernel.workgroup_size, (256, 1, 1));
    }

    #[test]
    fn test_dispatch() {
        let mut ctx = GpuContext::new();
        ctx.compile_kernel("test_kernel", "fn test() {}", (64, 1, 1)).unwrap();
        let handle = ctx.dispatch("test_kernel", (1024, 1, 1)).unwrap();
        assert_eq!(handle.total_threads, 1024);
        assert_eq!(handle.total_workgroups, 16); // 1024 / 64
    }

    #[test]
    fn test_has_gpu_matches_best_device() {
        let ctx = GpuContext::new();
        // has_gpu() must agree with best_device() being a non-Software vendor.
        let real = ctx.best_device().map_or(false, |d| d.vendor != GpuVendor::Software);
        assert_eq!(ctx.has_gpu(), real);
    }

    #[test]
    fn test_best_device_label_nonempty() {
        let ctx = GpuContext::new();
        let label = ctx.best_device_label();
        assert!(!label.is_empty());
        // When no hardware accelerator, label is the software-fallback string.
        if !ctx.has_gpu() {
            assert_eq!(label, "CPU (software fallback)");
        }
    }

    #[test]
    fn test_accelerated_devices_excludes_software() {
        let ctx = GpuContext::new();
        // Every device the honest iterator yields must be real hardware.
        assert!(ctx.accelerated_devices().all(|d| d.vendor != GpuVendor::Software));
        // Count agrees with has_gpu().
        assert_eq!(ctx.accelerated_devices().count() > 0, ctx.has_gpu());
    }

    #[test]
    fn test_total_compute_units_consistency() {
        let ctx = GpuContext::new();
        let sum: u32 = ctx.accelerated_devices().map(|d| d.compute_units).sum();
        assert_eq!(ctx.total_compute_units(), sum);
        // No real GPU ⇒ zero horsepower; some GPU ⇒ positive.
        assert_eq!(ctx.total_compute_units() == 0, !ctx.has_gpu());
    }

    #[test]
    fn test_partition_tiles_whole_range_no_overlap() {
        let parts = partition_nonce_space(1000, &[1, 1, 2]); // weights 1:1:2
        assert_eq!(parts.len(), 3);
        // Proportional: 250, 250, 500 (last absorbs remainder).
        assert_eq!(parts, vec![(0, 250), (250, 250), (500, 500)]);
        // Contiguous tiling: each start == previous start+len, total covered.
        let mut cursor = 0u64;
        for (start, len) in &parts {
            assert_eq!(*start, cursor);
            cursor += len;
        }
        assert_eq!(cursor, 1000); // exact coverage, no gap/overlap
    }

    #[test]
    fn test_partition_remainder_goes_to_last() {
        // 100 / 3 even weights = 33,33,34 — sum must still equal total.
        let parts = partition_nonce_space(100, &[1, 1, 1]);
        let total: u64 = parts.iter().map(|(_, l)| l).sum();
        assert_eq!(total, 100);
        assert_eq!(parts.last().unwrap().1, 34);
    }

    #[test]
    fn test_stride_assignment_covers_disjointly() {
        let n = 4usize;
        // Each node gets (offset, stride=n).
        assert_eq!(stride_assignment(2, n), Some((2, 4)));
        assert_eq!(stride_assignment(4, n), None); // out of range
        assert_eq!(stride_assignment(0, 0), None); // no nodes
        // Property: every nonce 0..40 belongs to exactly one node = k % n.
        let mut covered = vec![0u32; 40];
        for node in 0..n {
            let (offset, stride) = stride_assignment(node, n).unwrap();
            let mut k = offset;
            while k < 40 {
                covered[k as usize] += 1;
                k += stride;
            }
        }
        assert!(covered.iter().all(|&c| c == 1), "every nonce covered exactly once");
    }

    #[test]
    fn test_partition_edge_cases() {
        assert!(partition_nonce_space(0, &[1, 2]).is_empty());
        assert!(partition_nonce_space(100, &[]).is_empty());
        // All-zero weights ⇒ even split fallback, still tiles fully.
        let even = partition_nonce_space(99, &[0, 0]);
        assert_eq!(even.iter().map(|(_, l)| l).sum::<u64>(), 99);
    }
}

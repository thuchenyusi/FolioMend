//! Optional, lazily initialized GPU preprocessing. PDF codecs stay on the CPU.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

const MIN_GPU_PIXELS: u64 = 512 * 512;
const MAX_GPU_BUFFER: u64 = 128 * 1024 * 1024;
static BACKEND: OnceLock<Mutex<Backend>> = OnceLock::new();

#[derive(Clone, Copy)]
pub(crate) struct ImageRequest<'a> {
    pub pixels: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub target_width: u32,
    pub target_height: u32,
    pub monochrome: bool,
}

impl ImageRequest<'_> {
    fn output_len(self) -> u64 {
        let row = if self.monochrome {
            self.width.div_ceil(8)
        } else {
            self.target_width
        };
        u64::from(row) * u64::from(self.target_height)
    }

    fn valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && matches!(self.channels, 1 | 3)
            && self.target_width > 0
            && self.target_height > 0
            && self.target_width <= self.width
            && self.target_height <= self.height
            && (!self.monochrome
                || (self.target_width == self.width && self.target_height == self.height))
            && u64::from(self.width) * u64::from(self.height) * u64::from(self.channels)
                == self.pixels.len() as u64
    }
}

#[derive(Default)]
struct Backend {
    initialized: bool,
    gpu: Option<Gpu>,
    status: String,
}

impl Backend {
    fn process(&mut self, request: ImageRequest<'_>) -> Option<Vec<u8>> {
        if !request.valid() || u64::from(request.width) * u64::from(request.height) < MIN_GPU_PIXELS
        {
            return None;
        }
        if !self.initialized {
            self.initialized = true;
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(Gpu::new)) {
                Ok(Ok(gpu)) => {
                    self.status = format!("GPU：{}", gpu.name);
                    self.gpu = Some(gpu);
                }
                Ok(Err(reason)) => self.status = format!("CPU（GPU 不可用：{reason}）"),
                Err(_) => self.status = "CPU（GPU 初始化失败）".into(),
            }
        }
        let gpu = self.gpu.as_ref()?;
        // An oversized individual image does not disable the GPU for later pages.
        if !gpu.fits(request) {
            return None;
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gpu.process(request))) {
            Ok(Ok(bytes)) => Some(bytes),
            result => {
                let reason = match result {
                    Ok(Err(reason)) => reason,
                    _ => "GPU 计算异常".into(),
                };
                log::warn!("Image preprocessing fell back to CPU: {reason}");
                self.gpu = None;
                self.status = format!("CPU（GPU 已回退：{reason}）");
                None
            }
        }
    }
}

pub(crate) fn preprocess(request: ImageRequest<'_>) -> Option<Vec<u8>> {
    if !request.valid() || u64::from(request.width) * u64::from(request.height) < MIN_GPU_PIXELS {
        return None;
    }
    BACKEND
        .get_or_init(|| Mutex::new(Backend::default()))
        .lock()
        .ok()?
        .process(request)
}

pub(crate) fn status() -> String {
    BACKEND
        .get()
        .and_then(|backend| backend.lock().ok())
        .map(|backend| {
            if backend.status.is_empty() {
                "CPU（小图像无需 GPU）".into()
            } else {
                backend.status.clone()
            }
        })
        .unwrap_or_else(|| "CPU".into())
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: Vec<wgpu::ComputePipeline>,
    name: String,
    failed: Arc<AtomicBool>,
}

impl Gpu {
    fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|_| "未检测到支持计算的显卡".to_string())?;
        let info = adapter.get_info();
        // Do not mistake a software graphics adapter for hardware acceleration.
        if matches!(
            info.device_type,
            wgpu::DeviceType::Cpu | wgpu::DeviceType::Other
        ) {
            return Err("未检测到硬件 GPU".into());
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("FolioMend image preprocessing"),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|_| "显卡驱动不支持所需计算能力".to_string())?;
        let failed = Arc::new(AtomicBool::new(false));
        let error_flag = Arc::clone(&failed);
        device.on_uncaptured_error(Box::new(move |error| {
            error_flag.store(true, Ordering::Relaxed);
            log::warn!("GPU error: {error}");
        }));
        let lost_flag = Arc::clone(&failed);
        device.set_device_lost_callback(move |_, _| {
            lost_flag.store(true, Ordering::Relaxed);
        });
        push_error_scopes(&device);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("FolioMend grayscale/triangle/monochrome"),
            source: wgpu::ShaderSource::Wgsl(include_str!("image_preprocess.wgsl").into()),
        });
        let pipelines = ["grayscale", "monochrome", "vertical", "horizontal"]
            .iter()
            .map(|entry| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: None,
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .collect();
        pop_error_scopes(&device).map_err(|error| {
            log::warn!("GPU pipeline initialization failed: {error}");
            "显卡无法运行图像计算".to_string()
        })?;
        Ok(Self {
            device,
            queue,
            pipelines,
            name: info.name,
            failed,
        })
    }

    fn fits(&self, request: ImageRequest<'_>) -> bool {
        let limit = MAX_GPU_BUFFER
            .min(u64::from(
                self.device.limits().max_storage_buffer_binding_size,
            ))
            .min(self.device.limits().max_buffer_size);
        let input = (request.pixels.len() as u64).div_ceil(4) * 4;
        let output = request.output_len().div_ceil(4) * 4;
        let intermediate = u64::from(request.width) * u64::from(request.target_height) * 4;
        input <= limit && output <= limit && intermediate <= limit
    }

    fn process(&self, request: ImageRequest<'_>) -> Result<Vec<u8>, String> {
        if self.failed.load(Ordering::Relaxed) {
            return Err("显卡设备已丢失".into());
        }
        push_error_scopes(&self.device);
        let result = self.compute(request);
        let errors = pop_error_scopes(&self.device);
        let bytes = result?;
        errors.map_err(|error| {
            log::warn!("GPU preprocessing failed: {error}");
            "GPU 图像计算失败".to_string()
        })?;
        if self.failed.load(Ordering::Relaxed) {
            return Err("显卡设备已丢失".into());
        }
        Ok(bytes)
    }

    fn compute(&self, request: ImageRequest<'_>) -> Result<Vec<u8>, String> {
        let mut upload = request.pixels.to_vec();
        upload.resize(upload.len().div_ceil(4) * 4, 0);
        let input = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Image input"),
                contents: &upload,
                usage: wgpu::BufferUsages::STORAGE,
            });
        let resized = !request.monochrome
            && (request.width != request.target_width || request.height != request.target_height);
        let temporary = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vertical triangle samples"),
            size: if resized {
                u64::from(request.width) * u64::from(request.target_height) * 4
            } else {
                4
            },
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let output_size = request.output_len().div_ceil(4) * 4;
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Image output"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Image readback"),
            size: output_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params: Vec<u8> = [
            request.width,
            request.height,
            u32::from(request.channels),
            request.target_width,
            request.target_height,
            request.output_len() as u32,
            0,
            0,
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Image dimensions"),
                contents: &params,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let stages: Vec<(usize, u32)> = if resized {
            vec![
                (2, request.width * request.target_height),
                (3, (output_size / 4) as u32),
            ]
        } else {
            vec![(
                if request.monochrome { 1 } else { 0 },
                (output_size / 4) as u32,
            )]
        };
        for (stage, count) in stages {
            let pipeline = &self.pipelines[stage];
            // Auto layouts only include bindings actually used by this entry point.
            let mut entries = vec![wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }];
            if stage != 3 {
                entries.push(wgpu::BindGroupEntry {
                    binding: 1,
                    resource: input.as_entire_binding(),
                });
            }
            if stage >= 2 {
                entries.push(wgpu::BindGroupEntry {
                    binding: 2,
                    resource: temporary.as_entire_binding(),
                });
            }
            if stage != 2 {
                entries.push(wgpu::BindGroupEntry {
                    binding: 3,
                    resource: output.as_entire_binding(),
                });
            }
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &entries,
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let groups = count.div_ceil(64);
            pass.dispatch_workgroups(groups.min(65535), groups.div_ceil(65535), 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output_size);
        self.queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.device
                .poll(wgpu::PollType::Poll)
                .map_err(|_| "GPU 回读失败".to_string())?;
            match receiver.try_recv() {
                Ok(result) => {
                    result.map_err(|_| "GPU 回读失败".to_string())?;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err("GPU 回读中断".into())
                }
                Err(_) if Instant::now() >= deadline => return Err("GPU 计算超时".into()),
                Err(_) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        let mapped = readback.slice(..).get_mapped_range();
        let bytes = mapped[..request.output_len() as usize].to_vec();
        drop(mapped);
        readback.unmap();
        Ok(bytes)
    }
}

fn push_error_scopes(device: &wgpu::Device) {
    for filter in [
        wgpu::ErrorFilter::OutOfMemory,
        wgpu::ErrorFilter::Internal,
        wgpu::ErrorFilter::Validation,
    ] {
        device.push_error_scope(filter);
    }
}

fn pop_error_scopes(device: &wgpu::Device) -> Result<(), String> {
    let mut error = None;
    for _ in 0..3 {
        if let Some(value) = pollster::block_on(device.pop_error_scope()) {
            error = Some(value.to_string());
        }
    }
    error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_gpu_returns_cpu_fallback_without_reinitializing() {
        let pixels = vec![200; 512 * 512];
        let mut backend = Backend {
            initialized: true,
            gpu: None,
            status: "CPU（GPU 不可用）".into(),
        };
        let request = ImageRequest {
            pixels: &pixels,
            width: 512,
            height: 512,
            channels: 1,
            target_width: 512,
            target_height: 512,
            monochrome: false,
        };
        assert!(backend.process(request).is_none());
        assert!(backend.process(request).is_none());
        assert_eq!(backend.status, "CPU（GPU 不可用）");
    }

    #[test]
    fn small_or_invalid_images_do_not_initialize_gpu() {
        let mut backend = Backend::default();
        let request = ImageRequest {
            pixels: &[0, 255],
            width: 2,
            height: 1,
            channels: 1,
            target_width: 2,
            target_height: 1,
            monochrome: false,
        };
        assert!(backend.process(request).is_none());
        assert!(backend
            .process(ImageRequest {
                width: 0,
                ..request
            })
            .is_none());
        assert!(!backend.initialized);
    }

    // Run explicitly on a hardware machine; the normal suite works on CPU-only CI.
    #[test]
    #[ignore = "requires a hardware GPU"]
    fn hardware_preprocessing_matches_cpu_and_recovers_from_device_failure() {
        let gpu = Gpu::new().expect("hardware GPU should initialize");
        eprintln!("Testing hardware GPU: {}", gpu.name);
        for (width, height) in [(513u32, 517u32), (1, 262145), (262145, 1)] {
            let gray: Vec<u8> = (0..width * height)
                .map(|i| (i.wrapping_mul(97) % 256) as u8)
                .collect();
            let rgb: Vec<u8> = gray
                .iter()
                .flat_map(|v| [*v, v.wrapping_add(7), v.wrapping_add(13)])
                .collect();
            for (pixels, channels) in [(gray.as_slice(), 1), (rgb.as_slice(), 3)] {
                let cpu: Vec<u8> = if channels == 1 {
                    pixels.to_vec()
                } else {
                    pixels
                        .chunks_exact(3)
                        .map(|p| {
                            ((u16::from(p[0]) * 77 + u16::from(p[1]) * 150 + u16::from(p[2]) * 29)
                                / 256) as u8
                        })
                        .collect()
                };
                let request = ImageRequest {
                    pixels,
                    width,
                    height,
                    channels,
                    target_width: width,
                    target_height: height,
                    monochrome: false,
                };
                assert_eq!(gpu.process(request).unwrap(), cpu);
                let mut packed = vec![0; width.div_ceil(8) as usize * height as usize];
                for (i, value) in cpu.iter().enumerate() {
                    if *value >= 160 {
                        packed[i / width as usize * width.div_ceil(8) as usize
                            + i % width as usize / 8] |= 1 << (7 - i % width as usize % 8);
                    }
                }
                assert_eq!(
                    gpu.process(ImageRequest {
                        monochrome: true,
                        ..request
                    })
                    .unwrap(),
                    packed
                );
                if width > 1 || height > 1 {
                    let target_width = (width / 2).max(1);
                    let target_height = (height / 2).max(1);
                    let source = image::GrayImage::from_raw(width, height, cpu).unwrap();
                    let expected = image::imageops::resize(
                        &source,
                        target_width,
                        target_height,
                        image::imageops::FilterType::Triangle,
                    )
                    .into_raw();
                    let actual = gpu
                        .process(ImageRequest {
                            target_width,
                            target_height,
                            ..request
                        })
                        .unwrap();
                    assert_eq!(actual.len(), expected.len());
                    assert!(actual
                        .iter()
                        .zip(&expected)
                        .all(|(a, b)| a.abs_diff(*b) <= 1));
                }
            }
        }
        // A realistic scan also exercises the two-dimensional dispatch path (> 65535 workgroups).
        let (width, height, target_width, target_height) = (3200, 4800, 1333, 2000);
        let pixels: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                let v = ((i * 31 + i / width * 7) % 256) as u8;
                [v, v.wrapping_add(3), v.wrapping_add(5)]
            })
            .collect();
        let request = ImageRequest {
            pixels: &pixels,
            width,
            height,
            channels: 3,
            target_width,
            target_height,
            monochrome: false,
        };
        let start = Instant::now();
        let actual = gpu.process(request).unwrap();
        let gpu_time = start.elapsed();
        let start = Instant::now();
        let gray = pixels
            .chunks_exact(3)
            .map(|p| {
                ((u16::from(p[0]) * 77 + u16::from(p[1]) * 150 + u16::from(p[2]) * 29) / 256) as u8
            })
            .collect();
        let source = image::GrayImage::from_raw(width, height, gray).unwrap();
        let expected = image::imageops::resize(
            &source,
            target_width,
            target_height,
            image::imageops::FilterType::Triangle,
        )
        .into_raw();
        let cpu_time = start.elapsed();
        assert_eq!(actual.len(), expected.len());
        assert!(actual
            .iter()
            .zip(&expected)
            .all(|(a, b)| a.abs_diff(*b) <= 1));
        eprintln!("3200x4800 RGB grayscale + triangle resize (including GPU transfer): GPU {gpu_time:?}, CPU {cpu_time:?}");
        gpu.failed.store(true, Ordering::Relaxed);
        let mut backend = Backend {
            initialized: true,
            gpu: Some(gpu),
            status: String::new(),
        };
        let pixels = vec![100; 512 * 512];
        assert!(backend
            .process(ImageRequest {
                pixels: &pixels,
                width: 512,
                height: 512,
                channels: 1,
                target_width: 512,
                target_height: 512,
                monochrome: false
            })
            .is_none());
        assert!(backend.gpu.is_none());
        assert!(backend.status.contains("已回退"));
    }
}

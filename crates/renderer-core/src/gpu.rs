//! 设备级上下文：一个实例、一块适配器、一台设备、一条队列。
//!
//! 这一层只负责"把 GPU 弄到手"——挑适配器、按需申请特性、创建设备。
//! 它不认识网格、材质、pass，也不持有任何渲染目标；上层按需借用其中的句柄。

use anyhow::{anyhow, Context, Result};

/// 已就绪的 GPU 环境。
pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    info: wgpu::AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl GpuContext {
    /// 请求适配器并创建设备。
    ///
    /// `required_features` 是一段回调而不是固定值：要开哪些特性取决于适配器实际
    /// 支持什么（比如时间戳查询要能拿到才要），而这个判断只有拿到适配器之后才能做。
    pub async fn request(
        instance: wgpu::Instance,
        backends: wgpu::Backends,
        compatible_surface: Option<&wgpu::Surface<'static>>,
        required_features: impl FnOnce(&wgpu::Adapter) -> wgpu::Features,
    ) -> Result<Self> {
        let adapter_count = instance.enumerate_adapters(backends).await.len();
        if adapter_count == 0 {
            return Err(anyhow!(
                "no {} adapter found. Install or update your GPU driver.",
                crate::backend::label(backends)
            ));
        }

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface,
                ..Default::default()
            })
            .await
            .context("failed to request a compatible GPU adapter")?;

        let info = adapter.get_info();
        log::info!("adapter: {} ({:?})", info.name, info.backend);

        let features = required_features(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: features,
                ..Default::default()
            })
            .await
            .context("failed to request GPU device")?;

        Ok(Self {
            instance,
            adapter,
            info,
            device,
            queue,
        })
    }

    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// 适配器句柄。创建 surface 配置、查询格式特性时需要它。
    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    pub fn info(&self) -> &wgpu::AdapterInfo {
        &self.info
    }

    pub fn adapter_name(&self) -> &str {
        &self.info.name
    }

    /// 设备本次实际启用的特性（是申请到的，不一定是适配器支持的全部）。
    pub fn features(&self) -> wgpu::Features {
        self.device.features()
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}

//! Windows screen capture via DXGI Desktop Duplication API.
//!
//! WHY DXGI: Zero GPU→CPU copy path. Frame lives on GPU texture until
//!           the hardware encoder (NVENC/QuickSync/AMF) consumes it.
//!           Provides dirty rect metadata — only changed regions encoded.
//!
//! REQUIREMENTS: Windows 8+ (DXGI 1.2). Runs in a single-threaded COM apartment.

#![cfg(target_os = "windows")]

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use miru_common::message::DisplayInfo;
use tracing::{debug, error, info, warn};
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::RECT,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
                D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING,
            },
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY},
                IDXGIAdapter1, IDXGIDevice, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication,
                IDXGISurface1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT,
                DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTPUT_DESC,
            },
        },
    },
};

use crate::{
    display::DisplayList,
    frame::{DirtyRect, PixelFormat, RawFrame},
    ScreenCapturer,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum time to wait for a new frame from DXGI.
const FRAME_TIMEOUT_MS: u32 = 100;

pub struct WindowsCapturer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    current_display: u8,
    width: u32,
    height: u32,
}

unsafe impl Send for WindowsCapturer {}

impl WindowsCapturer {
    pub fn new() -> Result<Self> {
        unsafe {
            // Create D3D11 device
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                Default::default(),
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("D3D11CreateDevice failed")?;

            let device =
                device.ok_or_else(|| anyhow::anyhow!("D3D11CreateDevice: device is None"))?;
            let context =
                context.ok_or_else(|| anyhow::anyhow!("D3D11CreateDevice: context is None"))?;

            let mut capturer = Self {
                device,
                context,
                duplication: unsafe { std::mem::zeroed() }, // placeholder
                staging: unsafe { std::mem::zeroed() },
                current_display: 0,
                width: 0,
                height: 0,
            };
            capturer.select_display_inner(0)?;
            Ok(capturer)
        }
    }

    unsafe fn select_display_inner(&mut self, index: u8) -> Result<()> {
        let dxgi_device: IDXGIDevice = self.device.cast()?;
        let adapter: IDXGIAdapter1 = dxgi_device.GetAdapter()?.cast()?;
        let factory: IDXGIFactory1 = adapter.GetParent()?;

        // Enumerate outputs
        let mut out_index = 0u32;
        let mut target_output = None;
        let mut desc_out: Option<DXGI_OUTPUT_DESC> = None;

        loop {
            match adapter.EnumOutputs(out_index) {
                Ok(output) => {
                    let output1: IDXGIOutput1 = output.cast()?;
                    if out_index == index as u32 {
                        let mut desc = DXGI_OUTPUT_DESC::default();
                        output.GetDesc(&mut desc)?;
                        desc_out = Some(desc);
                        target_output = Some(output1);
                        break;
                    }
                    out_index += 1;
                }
                Err(_) => bail!("display index {} out of range", index),
            }
        }

        let output1 =
            target_output.ok_or_else(|| anyhow::anyhow!("no output for display {}", index))?;
        let desc = desc_out.ok_or_else(|| anyhow::anyhow!("no desc for display {}", index))?;

        let duplication = output1.DuplicateOutput(&self.device)?;
        let mut dup_desc = Default::default();
        duplication.GetDesc(&mut dup_desc);

        let w = dup_desc.ModeDesc.Width;
        let h = dup_desc.ModeDesc.Height;

        // Create staging texture (CPU-readable)
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ,
            ..Default::default()
        };
        let mut staging = None;
        self.device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;

        self.duplication = duplication;
        self.staging = staging.ok_or_else(|| anyhow::anyhow!("CreateTexture2D returned None"))?;
        self.current_display = index;
        self.width = w;
        self.height = h;

        info!("DXGI: display {} selected ({}×{})", index, w, h);
        Ok(())
    }
}

impl ScreenCapturer for WindowsCapturer {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        // Enumerate via DXGI factory
        let mut infos = Vec::new();
        unsafe {
            let dxgi_device: IDXGIDevice = self.device.cast()?;
            let adapter: IDXGIAdapter1 = dxgi_device.GetAdapter()?.cast()?;
            let mut i = 0u32;
            while let Ok(output) = adapter.EnumOutputs(i) {
                let mut desc = DXGI_OUTPUT_DESC::default();
                output.GetDesc(&mut desc)?;
                let name = String::from_utf16_lossy(
                    &desc
                        .DeviceName
                        .iter()
                        .take_while(|&&c| c != 0)
                        .cloned()
                        .collect::<Vec<_>>(),
                );
                let r = desc.DesktopCoordinates;
                let w = (r.right - r.left) as u32;
                let h = (r.bottom - r.top) as u32;
                infos.push(DisplayInfo {
                    index: i as u8,
                    width: w,
                    height: h,
                    refresh_hz: 60,
                    name,
                    primary: i == 0,
                });
                i += 1;
            }
        }
        Ok(infos)
    }

    fn select_display(&mut self, index: u8) -> Result<()> {
        unsafe { self.select_display_inner(index) }
    }

    fn next_frame(&mut self) -> Result<Option<RawFrame>> {
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;

            match self.duplication.AcquireNextFrame(
                FRAME_TIMEOUT_MS,
                &mut frame_info,
                &mut resource,
            ) {
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                    warn!("DXGI access lost — reinitializing");
                    self.select_display_inner(self.current_display)?;
                    return Ok(None);
                }
                Err(e) => bail!("AcquireNextFrame: {e}"),
                Ok(_) => {}
            }

            // Skip frames with no desktop changes
            if frame_info.TotalMetadataBufferSize == 0 && frame_info.LastPresentTime == 0 {
                self.duplication.ReleaseFrame()?;
                return Ok(None);
            }

            // Collect dirty rects
            let dirty_rects = if frame_info.TotalMetadataBufferSize > 0 {
                let mut buf: Vec<RECT> = vec![Default::default(); 64];
                let mut moved_buf: Vec<u8> = vec![0; 256];
                let mut moved_size = 0u32;
                let mut dirty_size = 0u32;

                let _ = self
                    .duplication
                    .GetFrameMoveRects(&mut moved_buf, &mut moved_size);
                let _ = self
                    .duplication
                    .GetFrameDirtyRects(bytemuck::cast_slice_mut(&mut buf), &mut dirty_size);

                let count = dirty_size as usize / std::mem::size_of::<RECT>();
                buf[..count]
                    .iter()
                    .map(|r| DirtyRect {
                        x: r.left as u32,
                        y: r.top as u32,
                        w: (r.right - r.left) as u32,
                        h: (r.bottom - r.top) as u32,
                    })
                    .collect()
            } else {
                RawFrame::full_dirty(self.width, self.height)
            };

            let resource =
                resource.ok_or_else(|| anyhow::anyhow!("AcquireNextFrame: resource is None"))?;
            let texture: ID3D11Texture2D = match resource.cast() {
                Ok(t) => t,
                Err(e) => {
                    // ReleaseFrame must be called even on error; otherwise
                    // DXGI permanently refuses future AcquireNextFrame calls.
                    let _ = self.duplication.ReleaseFrame();
                    bail!("resource cast to ID3D11Texture2D: {e}");
                }
            };

            // GPU→GPU copy to staging
            self.context.CopyResource(&self.staging, &texture);
            self.duplication.ReleaseFrame()?;

            // Map staging for CPU read
            let mut mapped = Default::default();
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;

            let stride = mapped.RowPitch;
            let size = (stride * self.height) as usize;
            let slice = std::slice::from_raw_parts(mapped.pData as *const u8, size);
            let data = Bytes::copy_from_slice(slice);

            self.context.Unmap(&self.staging, 0);

            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            Ok(Some(RawFrame {
                display_idx: self.current_display,
                width: self.width,
                height: self.height,
                stride,
                format: PixelFormat::Bgra32,
                data,
                timestamp_ms: ts,
                dirty_rects,
            }))
        }
    }

    fn close(self) {
        // Drop releases COM references automatically
    }
}

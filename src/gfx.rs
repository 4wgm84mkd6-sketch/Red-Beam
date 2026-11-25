//! Direct3D 11 / Direct2D / DXGI setup.
//!
//! The UI thread owns one D3D device used only for composition. Every render
//! worker creates its *own* device on the same physical GPU, so a long PDF
//! rasterisation never blocks a UI frame. Finished pages are handed over as
//! shared GPU textures (no copy through system memory).

use std::mem::ManuallyDrop;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

/// Which GPU to use. Workers must use the same one as the UI so shared
/// textures can be opened.
#[derive(Clone)]
pub struct GpuChoice {
    pub adapter: Option<IDXGIAdapter>,
    pub warp: bool,
}
unsafe impl Send for GpuChoice {}

/// Prefer the high-performance GPU (discrete card on hybrid laptops).
fn pick_adapter() -> Option<IDXGIAdapter> {
    unsafe {
        let f6: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
        let a: IDXGIAdapter1 = f6.EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE).ok()?;
        let d = a.GetDesc1().ok()?;
        // Skip the "Microsoft Basic Render Driver" software adapter here.
        if (d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
            return None;
        }
        a.cast().ok()
    }
}

pub fn create_device(choice: &GpuChoice) -> Result<ID3D11Device> {
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    let driver = match (&choice.adapter, choice.warp) {
        (_, true) => D3D_DRIVER_TYPE_WARP,
        (Some(_), false) => D3D_DRIVER_TYPE_UNKNOWN,
        (None, false) => D3D_DRIVER_TYPE_HARDWARE,
    };
    let adapter = if choice.warp { None } else { choice.adapter.as_ref() };
    let mut last = Error::from(E_FAIL);
    for lv in [&levels[..], &levels[1..]] {
        let mut dev: Option<ID3D11Device> = None;
        let r = unsafe {
            D3D11CreateDevice(
                adapter,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(lv),
                D3D11_SDK_VERSION,
                Some(&mut dev),
                None,
                None,
            )
        };
        match (r, dev) {
            (Ok(()), Some(d)) => return Ok(d),
            (Err(e), _) => last = e,
            _ => {}
        }
    }
    Err(last)
}

pub struct Gfx {
    pub d3d: ID3D11Device,
    pub ctx: ID2D1DeviceContext,
    pub swap: IDXGISwapChain1,
    pub gpu: GpuChoice,
    pub adapter_name: String,
    pub dedicated_vram_mb: u64,
    adapter3: Option<IDXGIAdapter3>,
    pub max_bitmap: u32,
    pub size: (u32, u32),
    target: Option<ID2D1Bitmap1>,
}

impl Gfx {
    pub fn new(hwnd: HWND) -> Result<Self> {
        unsafe {
            let mut gpu = GpuChoice { adapter: pick_adapter(), warp: false };
            let d3d = match create_device(&gpu) {
                Ok(d) => d,
                Err(_) => {
                    gpu = GpuChoice { adapter: None, warp: false };
                    match create_device(&gpu) {
                        Ok(d) => d,
                        Err(_) => {
                            gpu.warp = true;
                            create_device(&gpu)?
                        }
                    }
                }
            };
            let dxgi_device: IDXGIDevice = d3d.cast()?;
            let adapter = dxgi_device.GetAdapter()?;
            if !gpu.warp {
                gpu.adapter = Some(adapter.clone());
            }
            let desc = adapter.GetDesc()?;
            let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            let adapter_name = String::from_utf16_lossy(&desc.Description[..end]).trim().to_string();
            let dedicated_vram_mb = (desc.DedicatedVideoMemory as u64) / (1024 * 1024);
            let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;

            let opts = D2D1_FACTORY_OPTIONS { debugLevel: D2D1_DEBUG_LEVEL_NONE };
            let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&opts))?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi_device)?;
            let ctx = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            ctx.SetDpi(96.0, 96.0);
            ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let max_bitmap = ctx.GetMaximumBitmapSize();

            let sc_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: 0,
                Height: 0,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_NONE,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: 0,
            };
            let swap = dxgi_factory.CreateSwapChainForHwnd(&d3d, hwnd, &sc_desc, None, None)?;
            let _ = dxgi_factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            let d = swap.GetDesc1()?;

            let mut g = Gfx {
                adapter3: adapter.cast().ok(),
                d3d,
                ctx,
                swap,
                gpu,
                adapter_name,
                dedicated_vram_mb,
                max_bitmap,
                size: (d.Width, d.Height),
                target: None,
            };
            g.create_target()?;
            Ok(g)
        }
    }

    fn create_target(&mut self) -> Result<()> {
        unsafe {
            let surface: IDXGISurface = self.swap.GetBuffer(0)?;
            let props = bitmap_props(D2D1_ALPHA_MODE_IGNORE, D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW);
            let bmp = self.ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.ctx.SetTarget(&bmp);
            self.target = Some(bmp);
        }
        Ok(())
    }

    /// Point drawing back at the window after an offscreen pass.
    pub fn restore_target(&self) {
        if let Some(t) = &self.target {
            unsafe { self.ctx.SetTarget(t) };
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) -> Result<()> {
        if w == 0 || h == 0 || self.size == (w, h) {
            return Ok(());
        }
        unsafe {
            self.ctx.SetTarget(None);
            self.target = None;
            self.swap.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
        }
        self.size = (w, h);
        self.create_target()
    }

    pub fn present(&self) -> Result<()> {
        unsafe { self.swap.Present(1, DXGI_PRESENT(0)).ok() }
    }

    /// Open a texture a render worker shared with us and wrap it for drawing.
    pub fn open_shared(&self, handle: isize) -> Result<ID2D1Bitmap1> {
        unsafe {
            let mut tex: Option<ID3D11Texture2D> = None;
            self.d3d.OpenSharedResource(HANDLE(handle as *mut _), &mut tex)?;
            let surf: IDXGISurface = tex.ok_or_else(|| Error::from(E_POINTER))?.cast()?;
            let props = bitmap_props(D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BITMAP_OPTIONS_NONE);
            self.ctx.CreateBitmapFromDxgiSurface(&surf, Some(&props))
        }
    }

    /// (used MB, budget MB) of local video memory for this process.
    pub fn vram_usage_mb(&self) -> Option<(u64, u64)> {
        let a = self.adapter3.as_ref()?;
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        unsafe { a.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info).ok()? };
        Some((info.CurrentUsage / (1 << 20), info.Budget / (1 << 20)))
    }
}

pub fn bitmap_props(alpha: D2D1_ALPHA_MODE, opts: D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: alpha },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: opts,
        colorContext: ManuallyDrop::new(None),
    }
}

pub fn is_device_lost(e: &Error) -> bool {
    let c = e.code();
    c == D2DERR_RECREATE_TARGET
        || c == DXGI_ERROR_DEVICE_REMOVED
        || c == DXGI_ERROR_DEVICE_RESET
        || c == DXGI_ERROR_DEVICE_HUNG
}

//! Direct3D 11 / Direct2D / DXGI setup. One D3D device is shared by the UI
//! thread (composition + HUD) and the render worker (PDF rasterisation). The
//! Direct2D factory is created multi-threaded, so D2D serialises access to the
//! device; raw DXGI calls (Present / ResizeBuffers) are bracketed with
//! `ID2D1Multithread::Enter/Leave`.

use std::mem::ManuallyDrop;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

pub struct Gfx {
    pub dxgi_device: IDXGIDevice,
    pub d2d_device: ID2D1Device,
    pub ctx: ID2D1DeviceContext,
    pub mt: ID2D1Multithread,
    pub swap: IDXGISwapChain1,
    pub adapter_name: String,
    pub dedicated_vram_mb: u64,
    pub is_warp: bool,
    pub max_bitmap: u32,
    pub size: (u32, u32),
    _d3d: ID3D11Device,
}

fn create_d3d() -> Result<(ID3D11Device, bool)> {
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    let mut last_err = Error::from(E_FAIL);
    for (driver, warp) in [(D3D_DRIVER_TYPE_HARDWARE, false), (D3D_DRIVER_TYPE_WARP, true)] {
        // Try with 11_1 first; very old runtimes reject it, so retry without.
        for lv in [&levels[..], &levels[1..]] {
            let mut dev: Option<ID3D11Device> = None;
            let r = unsafe {
                D3D11CreateDevice(
                    None,
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
                (Ok(()), Some(d)) => return Ok((d, warp)),
                (Err(e), _) => last_err = e,
                _ => {}
            }
        }
    }
    Err(last_err)
}

impl Gfx {
    pub fn new(hwnd: HWND) -> Result<Self> {
        unsafe {
            let (d3d, is_warp) = create_d3d()?;
            if let Ok(mt) = d3d.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            let dxgi_device: IDXGIDevice = d3d.cast()?;
            let adapter = dxgi_device.GetAdapter()?;
            let desc = adapter.GetDesc()?;
            let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            let adapter_name = String::from_utf16_lossy(&desc.Description[..end]).trim().to_string();
            let dedicated_vram_mb = (desc.DedicatedVideoMemory as u64) / (1024 * 1024);
            let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;

            let opts = D2D1_FACTORY_OPTIONS { debugLevel: D2D1_DEBUG_LEVEL_NONE };
            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_MULTI_THREADED, Some(&opts))?;
            let mt: ID2D1Multithread = d2d_factory.cast()?;
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
                dxgi_device,
                d2d_device,
                ctx,
                mt,
                swap,
                adapter_name,
                dedicated_vram_mb,
                is_warp,
                max_bitmap,
                size: (d.Width, d.Height),
                _d3d: d3d,
            };
            g.create_target()?;
            Ok(g)
        }
    }

    fn create_target(&mut self) -> Result<()> {
        unsafe {
            let surface: IDXGISurface = self.swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: ManuallyDrop::new(None),
            };
            let bmp = self.ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.ctx.SetTarget(&bmp);
        }
        Ok(())
    }

    pub fn resize(&mut self, w: u32, h: u32) -> Result<()> {
        if w == 0 || h == 0 || self.size == (w, h) {
            return Ok(());
        }
        unsafe {
            self.ctx.SetTarget(None);
            self.mt.Enter();
            let r = self.swap.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0));
            self.mt.Leave();
            r?;
        }
        self.size = (w, h);
        self.create_target()
    }

    pub fn present(&self) -> Result<()> {
        unsafe {
            self.mt.Enter();
            let hr = self.swap.Present(1, DXGI_PRESENT(0));
            self.mt.Leave();
            hr.ok()
        }
    }
}

pub fn is_device_lost(e: &Error) -> bool {
    let c = e.code();
    c == D2DERR_RECREATE_TARGET
        || c == DXGI_ERROR_DEVICE_REMOVED
        || c == DXGI_ERROR_DEVICE_RESET
        || c == DXGI_ERROR_DEVICE_HUNG
}

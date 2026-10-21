//! Small builders for the fixed-function states and constant buffers everything here needs.

use anyhow::Result;
use bytemuck::Pod;
use windows::Win32::Graphics::Direct3D11::*;

use crate::Gpu;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    Opaque,
    /// Premultiplied alpha: out = src + dst * (1 - src.a).
    Premultiplied,
    /// Straight alpha: out = src * src.a + dst * (1 - src.a).
    Alpha,
}

pub fn blend_state(gpu: &Gpu, mode: Blend) -> Result<ID3D11BlendState> {
    let mut rt = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: false.into(),
        SrcBlend: D3D11_BLEND_ONE,
        DestBlend: D3D11_BLEND_ZERO,
        BlendOp: D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: D3D11_BLEND_ONE,
        DestBlendAlpha: D3D11_BLEND_ZERO,
        BlendOpAlpha: D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    match mode {
        Blend::Opaque => {}
        Blend::Premultiplied => {
            rt.BlendEnable = true.into();
            rt.SrcBlend = D3D11_BLEND_ONE;
            rt.DestBlend = D3D11_BLEND_INV_SRC_ALPHA;
            rt.SrcBlendAlpha = D3D11_BLEND_ONE;
            rt.DestBlendAlpha = D3D11_BLEND_INV_SRC_ALPHA;
        }
        Blend::Alpha => {
            rt.BlendEnable = true.into();
            rt.SrcBlend = D3D11_BLEND_SRC_ALPHA;
            rt.DestBlend = D3D11_BLEND_INV_SRC_ALPHA;
            rt.SrcBlendAlpha = D3D11_BLEND_ONE;
            rt.DestBlendAlpha = D3D11_BLEND_INV_SRC_ALPHA;
        }
    }
    let desc = D3D11_BLEND_DESC { AlphaToCoverageEnable: false.into(), IndependentBlendEnable: false.into(), RenderTarget: [rt; 8] };
    let mut s = None;
    unsafe { gpu.device.CreateBlendState(&desc, Some(&mut s))? };
    Ok(s.unwrap())
}

pub fn raster_state(gpu: &Gpu, cull_back: bool, scissor: bool) -> Result<ID3D11RasterizerState> {
    let desc = D3D11_RASTERIZER_DESC {
        FillMode: D3D11_FILL_SOLID,
        CullMode: if cull_back { D3D11_CULL_BACK } else { D3D11_CULL_NONE },
        // Our meshes are counter-clockwise when viewed in the left-handed convention.
        FrontCounterClockwise: false.into(),
        DepthBias: 0,
        DepthBiasClamp: 0.0,
        SlopeScaledDepthBias: 0.0,
        DepthClipEnable: true.into(),
        ScissorEnable: scissor.into(),
        MultisampleEnable: false.into(),
        AntialiasedLineEnable: false.into(),
    };
    let mut s = None;
    unsafe { gpu.device.CreateRasterizerState(&desc, Some(&mut s))? };
    Ok(s.unwrap())
}

pub fn depth_state(gpu: &Gpu, test: bool, write: bool) -> Result<ID3D11DepthStencilState> {
    let desc = D3D11_DEPTH_STENCIL_DESC {
        DepthEnable: test.into(),
        DepthWriteMask: if write { D3D11_DEPTH_WRITE_MASK_ALL } else { D3D11_DEPTH_WRITE_MASK_ZERO },
        DepthFunc: D3D11_COMPARISON_LESS_EQUAL,
        StencilEnable: false.into(),
        StencilReadMask: 0xFF,
        StencilWriteMask: 0xFF,
        FrontFace: D3D11_DEPTH_STENCILOP_DESC::default(),
        BackFace: D3D11_DEPTH_STENCILOP_DESC::default(),
    };
    let mut s = None;
    unsafe { gpu.device.CreateDepthStencilState(&desc, Some(&mut s))? };
    Ok(s.unwrap())
}

pub fn sampler(gpu: &Gpu, linear: bool, wrap: bool) -> Result<ID3D11SamplerState> {
    let addr = if wrap { D3D11_TEXTURE_ADDRESS_WRAP } else { D3D11_TEXTURE_ADDRESS_CLAMP };
    let desc = D3D11_SAMPLER_DESC {
        Filter: if linear { D3D11_FILTER_ANISOTROPIC } else { D3D11_FILTER_MIN_MAG_MIP_POINT },
        AddressU: addr,
        AddressV: addr,
        AddressW: addr,
        MipLODBias: 0.0,
        MaxAnisotropy: if linear { 8 } else { 1 },
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
    };
    let mut s = None;
    unsafe { gpu.device.CreateSamplerState(&desc, Some(&mut s))? };
    Ok(s.unwrap())
}

/// A constant buffer holding one `T` (padded to 16 bytes).
pub struct ConstantBuffer<T: Pod> {
    pub buf: ID3D11Buffer,
    _t: std::marker::PhantomData<T>,
}

impl<T: Pod> ConstantBuffer<T> {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: (size_of::<T>() as u32).div_ceil(16) * 16,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let mut b = None;
        unsafe { gpu.device.CreateBuffer(&desc, None, Some(&mut b))? };
        Ok(ConstantBuffer { buf: b.unwrap(), _t: Default::default() })
    }

    pub fn update(&self, ctx: &ID3D11DeviceContext, v: &T) {
        write_dynamic(ctx, &self.buf, bytemuck::bytes_of(v));
    }
}

/// Overwrites a dynamic buffer with `bytes` (map-discard).
pub fn write_dynamic(ctx: &ID3D11DeviceContext, buf: &ID3D11Buffer, bytes: &[u8]) {
    unsafe {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        if ctx.Map(buf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).is_ok() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), m.pData as *mut u8, bytes.len());
            ctx.Unmap(buf, 0);
        }
    }
}

/// A growable dynamic vertex or index buffer.
pub struct DynBuffer {
    pub buf: Option<ID3D11Buffer>,
    capacity: usize,
    bind: D3D11_BIND_FLAG,
}

impl DynBuffer {
    pub fn vertices() -> Self {
        DynBuffer { buf: None, capacity: 0, bind: D3D11_BIND_VERTEX_BUFFER }
    }

    pub fn indices() -> Self {
        DynBuffer { buf: None, capacity: 0, bind: D3D11_BIND_INDEX_BUFFER }
    }

    pub fn write(&mut self, gpu: &Gpu, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.buf.is_none() || bytes.len() > self.capacity {
            let cap = bytes.len().next_power_of_two().max(4096);
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: cap as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: self.bind.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: 0,
                StructureByteStride: 0,
            };
            let mut b = None;
            unsafe { gpu.device.CreateBuffer(&desc, None, Some(&mut b))? };
            self.buf = b;
            self.capacity = cap;
        }
        write_dynamic(&gpu.ctx, self.buf.as_ref().unwrap(), bytes);
        Ok(())
    }
}

/// An immutable vertex or index buffer.
pub fn static_buffer(gpu: &Gpu, bytes: &[u8], bind: D3D11_BIND_FLAG) -> Result<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: bytes.len() as u32,
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let init = D3D11_SUBRESOURCE_DATA { pSysMem: bytes.as_ptr().cast(), SysMemPitch: 0, SysMemSlicePitch: 0 };
    let mut b = None;
    unsafe { gpu.device.CreateBuffer(&desc, Some(&init), Some(&mut b))? };
    Ok(b.unwrap())
}

pub fn viewport(ctx: &ID3D11DeviceContext, w: u32, h: u32) {
    let vp = D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: w as f32, Height: h as f32, MinDepth: 0.0, MaxDepth: 1.0 };
    unsafe { ctx.RSSetViewports(Some(&[vp])) };
}

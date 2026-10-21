//! HLSL compilation with the system's `d3dcompiler_47.dll` (part of Windows), so there's no build
//! step and shaders live next to the code that uses them.

use anyhow::{Result, anyhow};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D11::*;
use windows::core::PCSTR;

use crate::Gpu;

pub fn compile(source: &str, name: &str, entry: &str, target: &str) -> Result<Vec<u8>> {
    let name_c = std::ffi::CString::new(name)?;
    let entry_c = std::ffi::CString::new(entry)?;
    let target_c = std::ffi::CString::new(target)?;
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let hr = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR(name_c.as_ptr().cast()),
            None,
            None,
            PCSTR(entry_c.as_ptr().cast()),
            PCSTR(target_c.as_ptr().cast()),
            D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    let message = errors.map(|e| blob_string(&e)).unwrap_or_default();
    if let Err(e) = hr {
        return Err(anyhow!("compiling {name}:{entry} ({target}): {e}\n{message}"));
    }
    let code = code.ok_or_else(|| anyhow!("compiling {name}:{entry}: no output"))?;
    Ok(blob_bytes(&code).to_vec())
}

fn blob_bytes(b: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) }
}

fn blob_string(b: &ID3DBlob) -> String {
    String::from_utf8_lossy(blob_bytes(b)).trim_end_matches('\0').to_string()
}

/// A vertex + pixel shader pair, with an input layout when the vertex shader takes vertex buffers.
pub struct Program {
    pub vs: ID3D11VertexShader,
    pub ps: ID3D11PixelShader,
    pub layout: Option<ID3D11InputLayout>,
}

impl Program {
    /// Compiles `VSMain` and `PSMain` (or the given entry points) from one source.
    pub fn new(gpu: &Gpu, source: &str, name: &str, layout: &[D3D11_INPUT_ELEMENT_DESC]) -> Result<Program> {
        Program::with_entries(gpu, source, name, "VSMain", "PSMain", layout)
    }

    pub fn with_entries(
        gpu: &Gpu,
        source: &str,
        name: &str,
        vs_entry: &str,
        ps_entry: &str,
        layout: &[D3D11_INPUT_ELEMENT_DESC],
    ) -> Result<Program> {
        let vs_code = compile(source, name, vs_entry, "vs_5_0")?;
        let ps_code = compile(source, name, ps_entry, "ps_5_0")?;
        unsafe {
            let mut vs = None;
            gpu.device.CreateVertexShader(&vs_code, None, Some(&mut vs))?;
            let mut ps = None;
            gpu.device.CreatePixelShader(&ps_code, None, Some(&mut ps))?;
            let layout = if layout.is_empty() {
                None
            } else {
                let mut l = None;
                gpu.device.CreateInputLayout(layout, &vs_code, Some(&mut l))?;
                l
            };
            Ok(Program { vs: vs.unwrap(), ps: ps.unwrap(), layout })
        }
    }

    pub fn bind(&self, ctx: &ID3D11DeviceContext) {
        unsafe {
            ctx.IASetInputLayout(self.layout.as_ref());
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
        }
    }
}

/// Builds a per-vertex input element. `name` must be a static, NUL-terminated semantic.
pub fn element(name: &'static [u8], index: u32, format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT, offset: u32) -> D3D11_INPUT_ELEMENT_DESC {
    debug_assert!(name.last() == Some(&0));
    D3D11_INPUT_ELEMENT_DESC {
        SemanticName: PCSTR(name.as_ptr()),
        SemanticIndex: index,
        Format: format,
        InputSlot: 0,
        AlignedByteOffset: offset,
        InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
        InstanceDataStepRate: 0,
    }
}

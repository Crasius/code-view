//! The OpenGL driver's own strings: which API version, GPU (or software rasterizer) and vendor
//! draw the frames. The same on every system; glow makes the queries unsafe.
#![expect(unsafe_code, reason = "glow's string queries; each unsafe block makes one")]

use eframe::glow::{self, HasContext as _};

/// `GL_VERSION`, `GL_RENDERER` and `GL_VENDOR`.
pub struct DriverStrings {
    pub version: String,
    pub renderer: String,
    pub vendor: String,
}

/// The driver strings of `gl`. eframe hands out its context only on the thread that paints with
/// it, where the context is current.
pub fn driver_strings(gl: &glow::Context) -> DriverStrings {
    DriverStrings {
        version: string(gl, glow::VERSION),
        renderer: string(gl, glow::RENDERER),
        vendor: string(gl, glow::VENDOR),
    }
}

fn string(gl: &glow::Context, parameter: u32) -> String {
    // SAFETY: a string query on eframe's current context, made on the thread that owns it.
    unsafe { gl.get_parameter_string(parameter) }
}

//! Diagnostic: render a trivial egui UI, trying the rendering path eframe won't
//! expose. eframe's glow path uses a GLES-over-WGL context (Wine's WGL rejects
//! it) and its wgpu path doesn't composite egui draws under CrossOver/MoltenVK.
//! wgpu's GL backend, however, reached a context via **EGL** — so this probe
//! forces glutin onto EGL and tries GLES, the one untried path. If its text
//! shows under Wine, the app can render the same way.

use std::ffi::CString;
use std::num::NonZeroU32;
use std::sync::Arc;

use egui_glow::glow;
use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext, Version,
};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::surface::{GlSurface, Surface, SwapInterval, WindowSurface};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

struct App {
    win: Option<Window>,
    surface: Option<Surface<WindowSurface>>,
    gl_ctx: Option<PossiblyCurrentContext>,
    painter: Option<egui_glow::Painter>,
    state: Option<egui_winit::State>,
    ctx: egui::Context,
}

#[allow(clippy::expect_used, clippy::too_many_lines)]
impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.win.is_some() {
            return;
        }
        let window = el
            .create_window(Window::default_attributes().with_title("Concierge"))
            .expect("window");
        let raw_win = window.window_handle().expect("rwh").as_raw();
        let raw_disp = window.display_handle().expect("dh").as_raw();

        // Force EGL on Windows. WGL context creation returns OS error 8341
        // under this CrossOver; wgpu's GL backend reached a context via EGL.
        #[cfg(target_os = "windows")]
        let display = unsafe { Display::new(raw_disp, DisplayApiPreference::Egl) }
            .inspect_err(|e| eprintln!("EGL display unavailable: {e}"))
            .or_else(|_| unsafe {
                Display::new(raw_disp, DisplayApiPreference::WglThenEgl(Some(raw_win)))
            })
            .expect("no gl display");
        #[cfg(not(target_os = "windows"))]
        let display =
            unsafe { Display::new(raw_disp, DisplayApiPreference::Cgl) }.expect("no gl display");
        eprintln!("display created");

        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .compatible_with_native_window(raw_win)
            .build();
        let config = unsafe { display.find_configs(template) }
            .expect("find_configs")
            .reduce(|a, b| {
                if b.num_samples() < a.num_samples() {
                    b
                } else {
                    a
                }
            })
            .expect("a gl config");
        eprintln!("config chosen");

        // Try GLES 3.0, GLES 2.0, then desktop GL — whatever this stack gives.
        let mk = |api| {
            ContextAttributesBuilder::new()
                .with_context_api(api)
                .build(Some(raw_win))
        };
        let not_current = unsafe {
            display
                .create_context(&config, &mk(ContextApi::Gles(Some(Version::new(3, 0)))))
                .or_else(|e| {
                    eprintln!("GLES 3.0 failed: {e}");
                    display.create_context(&config, &mk(ContextApi::Gles(Some(Version::new(2, 0)))))
                })
                .or_else(|e| {
                    eprintln!("GLES 2.0 failed: {e}");
                    display.create_context(&config, &mk(ContextApi::OpenGl(None)))
                })
                .expect("no gl context")
        };
        eprintln!("context created");

        let attrs = glutin::surface::SurfaceAttributesBuilder::<WindowSurface>::new().build(
            raw_win,
            NonZeroU32::new(800).unwrap(),
            NonZeroU32::new(600).unwrap(),
        );
        let surface = unsafe { display.create_window_surface(&config, &attrs) }.expect("surface");
        let gl_ctx = not_current.make_current(&surface).expect("make current");
        let _ = surface.set_swap_interval(&gl_ctx, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));

        let gl = unsafe {
            glow::Context::from_loader_function(|s| {
                display.get_proc_address(&CString::new(s).unwrap()).cast()
            })
        };
        {
            use glow::HasContext as _;
            eprintln!("GL_VERSION: {}", unsafe {
                gl.get_parameter_string(glow::VERSION)
            });
        }
        let painter = egui_glow::Painter::new(Arc::new(gl), "", None, false).expect("painter");
        let state = egui_winit::State::new(
            self.ctx.clone(),
            self.ctx.viewport_id(),
            el,
            None,
            None,
            None,
        );

        self.win = Some(window);
        self.surface = Some(surface);
        self.gl_ctx = Some(gl_ctx);
        self.painter = Some(painter);
        self.state = Some(state);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let (Some(window), Some(surface), Some(gl_ctx), Some(painter), Some(state)) = (
            self.win.as_ref(),
            self.surface.as_ref(),
            self.gl_ctx.as_ref(),
            self.painter.as_mut(),
            self.state.as_mut(),
        ) else {
            return;
        };
        let _ = state.on_window_event(window, &event);
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => {
                let raw_input = state.take_egui_input(window);
                let out = self.ctx.run(raw_input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.heading("Concierge glow (EGL/GLES) probe");
                        ui.label("If you can read this, egui renders via EGL under Wine.");
                    });
                });
                state.handle_platform_output(window, out.platform_output);
                let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
                let size: [u32; 2] = window.inner_size().into();
                {
                    use glow::HasContext as _;
                    unsafe {
                        painter.gl().clear_color(0.1, 0.1, 0.12, 1.0);
                        painter.gl().clear(glow::COLOR_BUFFER_BIT);
                    }
                }
                painter.paint_and_update_textures(
                    size,
                    out.pixels_per_point,
                    &prims,
                    &out.textures_delta,
                );
                surface.swap_buffers(gl_ctx).expect("swap");
                window.request_redraw();
            }
            _ => {}
        }
    }
}

#[allow(clippy::expect_used)]
fn main() {
    let el = EventLoop::new().expect("event loop");
    let mut app = App {
        win: None,
        surface: None,
        gl_ctx: None,
        painter: None,
        state: None,
        ctx: egui::Context::default(),
    };
    el.run_app(&mut app).expect("run");
}

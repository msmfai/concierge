//! Diagnostic: render a trivial egui UI through a **desktop OpenGL** context
//! created by hand. eframe's glow path requests a GLES-over-WGL context, which
//! CrossOver/Wine's WGL rejects; a desktop-GL context (CrossOver exposes Apple
//! GL up to 4.1) composites the traditional way, bypassing MoltenVK entirely.
//! If this window shows its text under Wine while the wgpu build stays blank,
//! the fix is to render the real app through desktop GL too.

use std::num::NonZeroU32;
use std::sync::Arc;

use egui_glow::glow;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{
    ContextApi, ContextAttributesBuilder, GlProfile, NotCurrentGlContext, PossiblyCurrentContext,
    Version,
};
use glutin::display::GetGlDisplay;
use glutin::prelude::GlDisplay;
use glutin::surface::{GlSurface, Surface, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
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

impl ApplicationHandler for App {
    #[allow(clippy::expect_used)]
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.win.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Concierge");
        let template = ConfigTemplateBuilder::new().with_alpha_size(8);
        let (window, gl_config) = DisplayBuilder::new()
            .with_window_attributes(Some(attrs))
            .build(el, template, |configs| configs.into_iter().next().unwrap())
            .expect("display build");
        let window = window.expect("window");
        let rwh = window.window_handle().expect("rwh").as_raw();
        let display = gl_config.display();

        // The whole point: request DESKTOP OpenGL 3.3 core, never GLES.
        let ctx_attrs = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
            .with_profile(GlProfile::Core)
            .build(Some(rwh));
        let not_current =
            unsafe { display.create_context(&gl_config, &ctx_attrs) }.unwrap_or_else(|e| {
                eprintln!("desktop GL 3.3 core failed: {e}; retrying with default attrs");
                let fallback = ContextAttributesBuilder::new().build(Some(rwh));
                unsafe { display.create_context(&gl_config, &fallback) }.expect("no GL context")
            });

        let surface_attrs = window
            .build_surface_attributes(<_>::default())
            .expect("surface attrs");
        let surface = unsafe { display.create_window_surface(&gl_config, &surface_attrs) }
            .expect("gl surface");
        let gl_ctx = not_current.make_current(&surface).expect("make current");
        let _ = surface.set_swap_interval(&gl_ctx, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));

        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s).cast())
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

    #[allow(clippy::expect_used)]
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
                        ui.heading("Concierge glow (desktop GL) probe");
                        ui.label("If you can read this, egui renders over OpenGL under Wine.");
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

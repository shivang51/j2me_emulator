use std::sync::{LazyLock, Mutex};

use image::GenericImageView;
use pixels::wgpu::Color;
use pixels::{Pixels, SurfaceTexture};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::jvm::JVM;

struct DrawState {
    pub pixels: Option<Pixels<'static>>,
}

impl DrawState {
    pub fn clear(&mut self) {
        if let Some(pixels) = &mut self.pixels {
            pixels.clear_color(Color::BLACK);
        }
    }
}

static DRAW_STATE: LazyLock<Mutex<DrawState>> =
    LazyLock::new(|| Mutex::new(DrawState { pixels: None }));

#[derive(Default)]
pub struct App {
    window: Option<&'static Window>,
    pub jvm: Option<JVM>,
}

impl App {
    fn draw(&mut self) {
        if let Some(jvm) = &mut self.jvm {
            let _ = jvm.paint();
        }

        let mut draw_state = DRAW_STATE.lock().unwrap();
        if let Some(pixels) = &mut draw_state.pixels {
            pixels.clear_color(Color::BLACK);
            pixels.render().unwrap();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = event_loop
            .create_window(Window::default_attributes().with_title("No file loaded - J2ME"))
            .unwrap();
        let window_ref: &'static Window = Box::leak(Box::new(window));
        let size = window_ref.inner_size();
        let surface = SurfaceTexture::new(size.width, size.height, window_ref);
        if let Some(jar) = &self.jvm.as_ref().unwrap().loaded_jar {
            window_ref.set_title(&format!("{} - J2ME", jar.manifest.name));
        }

        println!(
            "[+] Window {} resumed with size {}x{}",
            window_ref.title(),
            size.width,
            size.height
        );

        self.window = Some(window_ref);

        let mut draw_state = DRAW_STATE.lock().unwrap();
        draw_state.pixels = Some(Pixels::new(size.width, size.height, surface).unwrap());
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("The close button was pressed; stopping");
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                self.draw();
            }
            WindowEvent::Resized(new_size) => {
                println!("Window resized to {}x{}", new_size.width, new_size.height);
                let mut draw_state = DRAW_STATE.lock().unwrap();
                if let Some(pixels) = &mut draw_state.pixels {
                    pixels
                        .resize_surface(new_size.width, new_size.height)
                        .unwrap();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

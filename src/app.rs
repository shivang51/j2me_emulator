use std::sync::LazyLock;

use parking_lot::Mutex;
use pixels::wgpu::Color;
use pixels::{Pixels, SurfaceTexture};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::jvm::JVM;

pub struct DrawState {
    pub pixels: Option<Pixels<'static>>,
    pub width: u32,
    pub height: u32,
}

pub static DRAW_STATE: LazyLock<Mutex<DrawState>> = LazyLock::new(|| {
    Mutex::new(DrawState {
        pixels: None,
        width: 0,
        height: 0,
    })
});

#[derive(Default)]
pub struct App {
    window: Option<&'static Window>,
    pub jvm: Option<JVM>,
}

impl App {
    fn draw(&mut self) {
        {
            let mut draw_state = DRAW_STATE.lock();
            if let Some(pixels) = &mut draw_state.pixels {
                pixels.clear_color(Color::BLACK);
            }
        }

        if let Some(jvm) = &mut self.jvm {
            let res = jvm.paint();
            if let Err(e) = res {
                eprintln!("[App] jvm.paint() failed: {}", e);
            }
        }

        let mut draw_state = DRAW_STATE.lock();
        if let Some(pixels) = &mut draw_state.pixels {
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

        if size.width > 0 && size.height > 0 {
            let mut draw_state = DRAW_STATE.lock();
            draw_state.pixels = Some(Pixels::new(size.width, size.height, surface).unwrap());
            draw_state.width = size.width;
            draw_state.height = size.height;
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("The close button was pressed; stopping");
                std::process::exit(0);
            }
            WindowEvent::RedrawRequested => {
                self.draw();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
                    println!("Window resized to {}x{}", new_size.width, new_size.height);
                    let mut draw_state = DRAW_STATE.lock();
                    if let Some(pixels) = &mut draw_state.pixels {
                        pixels
                            .resize_surface(new_size.width, new_size.height)
                            .unwrap();
                        draw_state.width = new_size.width;
                        draw_state.height = new_size.height;
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }
}

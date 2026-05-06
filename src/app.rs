use image::GenericImageView;
use pixels::wgpu::Color;
use pixels::{Pixels, SurfaceTexture};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::jvm::JVM;

#[derive(Default)]
pub struct App {
    window: Option<&'static Window>,
    pixels: Option<Pixels<'static>>,
    pub jvm: Option<JVM>,
}

impl App {
    fn lerp(start: u8, end: u8, t: f32) -> u8 {
        (start as f32 + (end as f32 - start as f32) * t) as u8
    }

    fn draw(&mut self) {
        if let Some(pixels) = &mut self.pixels {
            let w = pixels.texture().width();
            let h = pixels.texture().height();

            pixels.clear_color(Color::WHITE);

            let frame = pixels.frame_mut();

            let resources = {
                let jvm = self.jvm.as_ref().unwrap();
                let state = jvm.state.lock().unwrap();
                state.resources.clone()
            };

            let mut p_x: u32 = 0;
            let mut p_y: u32 = 0;

            let mut max_row_height: u32 = 0;

            for (name, data) in &resources {
                let res = image::load_from_memory(data);

                if let Err(e) = res {
                    continue;
                }

                let img = res.unwrap();

                if p_x + img.width() > w {
                    p_x = 0;
                    p_y += max_row_height + 10;
                    max_row_height = 0;
                }

                max_row_height = max_row_height.max(img.height());

                for (x, y, img_px) in img.pixels() {
                    let px = p_x + x;
                    let py = p_y + y;
                    
                    if px >= w || py >= h {
                        continue;
                    }
                    
                    let px_idx = (py * w + px) as usize * 4;
                    if px_idx + 3 < frame.len() {
                        frame[px_idx] = img_px[0]; // R
                        frame[px_idx + 1] = img_px[1]; // G
                        frame[px_idx + 2] = img_px[2]; // B
                        frame[px_idx + 3] = img_px[3]; // A
                    }
                }

                p_x += img.width() + 10;
            }

            // p_x = 0;
            // p_y = 0;
            //
            // for pixel in frame.chunks_exact_mut(4) {
            //     let tx = p_x as f32 / w as f32; // -1 to 1
            //     let ty = p_y as f32 / h as f32;
            //     pixel[0] = App::lerp(0x00, 0xFF, tx); // G
            //     pixel[1] = App::lerp(0x00, 0xFF, ty); // G
            //     pixel[2] = 0xFF; // B
            //     pixel[3] = 0xFF; // A
            //
            //     if p_x == w - 1 {
            //         p_x = 0;
            //         p_y += 1;
            //     } else {
            //         p_x += 1;
            //     }
            // }

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
        let pixels = Pixels::new(size.width - 200, size.height - 200, surface).unwrap();

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
        self.pixels = Some(pixels);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("The close button was pressed; stopping");
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                if let Some(jvm) = &mut self.jvm {
                    let _ = jvm.paint();
                }
                self.draw();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(new_size) => {
                println!("Window resized to {}x{}", new_size.width, new_size.height);
                if let Some(pixels) = &mut self.pixels {
                    pixels
                        .resize_surface(new_size.width - 200, new_size.height - 200)
                        .unwrap();
                }
            }
            _ => {
                println!("Unhandled window event: {:?}", event);
            }
        }
    }
}

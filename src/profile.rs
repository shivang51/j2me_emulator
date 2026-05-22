use std::time::Duration;

use std::sync::Mutex;

pub struct Profile {
    method_name: String,
    time: std::time::Instant,
}

static ALL_DURATIONS: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());

impl Profile {
    pub fn clear() {
        let mut guard = ALL_DURATIONS.lock().unwrap();
        guard.clear();
    }

    pub fn dump(count: usize) {
        let guard = ALL_DURATIONS.lock().unwrap();
        println!("--- Profiling Results (last {} calls) ---", count);
        for (method, duration) in guard.iter().rev().take(count) {
            println!("Graphics.{} took {:?}", method, duration);
        }
    }

    pub fn this(method_name: &str) -> Self {
        Self {
            method_name: method_name.to_string(),
            time: std::time::Instant::now(),
        }
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        let elapsed = self.time.elapsed();

        let mut guard = ALL_DURATIONS.lock().unwrap();
        guard.push((self.method_name.clone(), elapsed));
    }
}

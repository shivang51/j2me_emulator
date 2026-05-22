use std::time::Duration;

use std::sync::Mutex;

pub struct Profile {
    method_name: String,
    time: std::time::Instant,
}

static ALL_DURATIONS: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());

impl Profile {
    pub fn clear() {
        if !cfg!(feature = "profiler") {
            return;
        }

        let mut guard = ALL_DURATIONS.lock().unwrap();
        guard.clear();
    }

    pub fn dump(count: usize) {
        if !cfg!(feature = "profiler") {
            return;
        }

        let guard = ALL_DURATIONS.lock().unwrap();

        let mut call_count: std::collections::HashMap<String, i32> =
            std::collections::HashMap::new();

        let total_time_per_method: std::collections::HashMap<String, Duration> = guard.iter().fold(
            std::collections::HashMap::new(),
            |mut acc, (method, duration)| {
                *acc.entry(format!("{}", method.clone(),))
                    .or_insert(Duration::ZERO) += *duration;
                *call_count.entry(method.clone()).or_insert(0) += 1;
                acc
            },
        );

        for (method, duration) in total_time_per_method.iter().take(count) {
            println!(
                "[Called {} times] Graphics.{} took {:?}",
                call_count.get(method).unwrap(),
                method,
                duration
            );
        }
        println!("--- Profiling Results (last {} calls) ---", count);
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
        if !cfg!(feature = "profiler") {
            return;
        }

        let elapsed = self.time.elapsed();

        let mut guard = ALL_DURATIONS.lock().unwrap();
        guard.push((self.method_name.clone(), elapsed));
    }
}

mod services;

use services::jar_extractor::JarExtractor;

fn main() {
    const JAR_PATH: &str = "test_files/tower_defense_(base_128x128_188767.jar";

    let mut jar_extractor = JarExtractor::for_path(JAR_PATH.to_string());

    let data = jar_extractor.run();

    match data {
        Ok(result) => println!("Jar extraction successful: {}", result.data),
        Err(e) => println!("Jar extraction failed: {}", e.message),
    }
}

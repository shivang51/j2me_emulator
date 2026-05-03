use std::{fs::File, io::Read};

use crate::services::jar_extractor::jar_manifest::JarManifest;

#[derive(Debug)]
pub struct JavaClass {
    pub name: String,
    pub content: Vec<u8>,
}

#[derive(Debug)]
pub struct JarFileData {
    pub manifest: JarManifest,
    pub classes: Vec<JavaClass>,
}

#[derive(Debug)]
pub struct JarExtractor {
    pub jar_path: String,
    pub data: Option<JarFileData>,
}

#[derive(Debug)]
pub struct JarExtractorError {
    pub message: String,
}

#[derive(Debug)]
pub struct JarExtractorResult {
    pub data: String,
}

impl JarExtractor {
    pub fn for_path(jar_path: String) -> Self {
        JarExtractor {
            jar_path,
            data: None,
        }
    }

    pub fn run(&mut self) -> Result<JarExtractorResult, JarExtractorError> {
        let file = File::open(&self.jar_path);

        if let Err(e) = file {
            return Result::Err(JarExtractorError {
                message: format!("Failed to open jar file: {}", e),
            });
        }

        println!("Successfully opened jar file: {}", self.jar_path);

        let archive = zip::ZipArchive::new(file.unwrap());

        if let Err(e) = archive {
            return Result::Err(JarExtractorError {
                message: format!("Failed to read jar file as zip archive: {}", e),
            });
        }

        let mut archive = archive.unwrap();

        let manifest_content = self.read_manifest(&mut archive);

        if let Err(e) = manifest_content {
            return Result::Err(e);
        }

        println!("Successfully read manifest");

        let classes = self.extract_classes(&mut archive);

        self.data = Some(JarFileData {
            manifest: JarManifest::from_string(manifest_content.unwrap()),
            classes: classes, // Placeholder for class extraction logic
        });

        return Result::Ok(JarExtractorResult {
            data: "Extracted data from jar file".to_string(),
        });
    }

    fn extract_classes(&self, archive: &mut zip::ZipArchive<File>) -> Vec<JavaClass> {
        let mut classes = Vec::new();

        for i in 0..archive.len() {
            let file = archive.by_index(i).unwrap();
            let name = file.name().to_string();

            if name.ends_with(".class") {
                let mut content = Vec::new();
                let mut class_file = file;
                class_file.read_to_end(&mut content).unwrap();
                println!("Extracted class file: {}", name);
                classes.push(JavaClass { name, content });
            }
        }

        return classes;
    }

    fn read_manifest(
        &self,
        archive: &mut zip::ZipArchive<File>,
    ) -> Result<String, JarExtractorError> {
        let manifest_file = archive.by_name("META-INF/MANIFEST.MF");

        if let Err(e) = manifest_file {
            return Result::Err(JarExtractorError {
                message: format!("Failed to find MANIFEST.MF in jar file: {}", e),
            });
        }

        let mut maifest_content = String::new();

        let result = manifest_file.unwrap().read_to_string(&mut maifest_content);

        if let Err(e) = result {
            return Result::Err(JarExtractorError {
                message: format!("Failed to read MANIFEST.MF content: {}", e),
            });
        }

        return Result::Ok(maifest_content);
    }
}

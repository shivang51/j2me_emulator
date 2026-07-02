use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct JarManifest {
    pub version: String,
    pub main_class: String,
    pub name: String,
    pub icon: String,
    pub vendor: String,
    pub properties: HashMap<String, String>,
}

// USED REFERENCE MANIFEST CONTENT FOR WRITING THE PARSER
// Manifest-Version: 1.0
// Ant-Version: Apache Ant 1.7.1
// Created-By: 14.0-b16 (Sun Microsystems Inc.)
// MIDlet-1: Base Defense,/res/icon/defense.png,bbs.TowerDefense
// MIDlet-Vendor: mob.ua
// MIDlet-Delete-Confirm: more games http://seclub.org
// MIDlet-Name: Base Defense
// MIDlet-Version: 1.0
// MicroEdition-Configuration: CLDC-1.0
// MicroEdition-Profile: MIDP-2.0
// Nokia-MIDlet-Category: Game
// SiteURL: wap.mob.ua

impl JarManifest {
    pub fn from_string(manifest_str: String) -> Self {
        let mut properties = HashMap::new();
        let mut lines: Vec<String> = Vec::new();

        for line in manifest_str.lines() {
            let line = line.trim_end_matches('\r');
            if line.starts_with(' ') {
                if let Some(previous) = lines.last_mut() {
                    previous.push_str(line.trim_start());
                }
            } else if !line.is_empty() {
                lines.push(line.to_string());
            }
        }

        for line in lines {
            if let Some((key, value)) = line.split_once(':') {
                properties.insert(key.trim().to_string(), value.trim().to_string());
            }
        }

        let version = properties
            .get("Manifest-Version")
            .cloned()
            .unwrap_or_default();
        let vendor = properties.get("MIDlet-Vendor").cloned().unwrap_or_default();
        let mut name = properties.get("MIDlet-Name").cloned().unwrap_or_default();
        let mut main_class = String::new();
        let mut icon = String::new();

        if let Some(midlet_info) = properties.get("MIDlet-1") {
            let parts: Vec<&str> = midlet_info.split(',').collect();
            if parts.len() >= 3 {
                if name.is_empty() {
                    name = parts[0].trim().to_string();
                }

                icon = parts[1].trim().to_string();
                if icon.starts_with('/') {
                    icon = icon[1..].to_string();
                }

                main_class = parts[2].trim().to_string();
            }
        }

        JarManifest {
            version,
            main_class,
            name,
            icon,
            vendor,
            properties,
        }
    }
}

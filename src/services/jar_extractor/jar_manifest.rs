#[derive(Debug, Clone)]
pub struct JarManifest {
    pub version: String,
    pub main_class: String,
    pub name: String,
    pub icon: String,
    pub vendor: String,
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
        let mut version = String::new();
        let mut main_class = String::new();
        let mut name = String::new();
        let mut icon = String::new();
        let mut vendor = String::new();

        for line in manifest_str.lines() {
            if line.starts_with("Manifest-Version:") {
                version = line["Manifest-Version:".len()..].trim().to_string();
            } else if line.starts_with("MIDlet-1:") {
                let midlet_info = line["MIDlet-1:".len()..].trim();
                let parts: Vec<&str> = midlet_info.split(',').collect();
                if parts.len() >= 3 {
                    name = parts[0].trim().to_string();
                    icon = parts[1].trim().to_string();
                    main_class = parts[2].trim().to_string();
                }
            } else if line.starts_with("MIDlet-Vendor:") {
                vendor = line["MIDlet-Vendor:".len()..].trim().to_string();
            } else if line.starts_with("MIDlet-Name:") {
                name = line["MIDlet-Name:".len()..].trim().to_string();
            }
        }

        JarManifest {
            version,
            main_class,
            name,
            icon,
            vendor,
        }
    }
}

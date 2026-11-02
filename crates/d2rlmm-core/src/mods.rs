//! The mod library: discovering installed mods, their manifests, and their config.
//!
//! A mod is a directory under `<config>/d2rlmm/mods/<id>/`, exactly as D2RMM lays it
//! out — `mod.json` for metadata and schema, `mod.js` for behaviour, and the mod's
//! own assets beside them. Directory name doubles as the mod id, which is why ids
//! have to survive being a path component.
//!
//! Config lives in the mod folder as `config.json` (`{ "fieldId": value }`), which
//! is what upstream does and what makes a mod folder self-contained: copy it to
//! another machine and the settings come along.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{io_at, Error, Result};
use crate::schema::ModConfig;

/// A mod's `mod.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModInfo {
    #[serde(rename = "$schema")]
    pub schema: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub website: Option<String>,
    pub version: Option<String>,
    /// Raw config field schema; interpreted by [`crate::schema`].
    pub config: Vec<Value>,
}

/// An installed mod, loaded from disk.
#[derive(Debug, Clone)]
pub struct Mod {
    pub id: String,
    pub dir: PathBuf,
    pub info: ModInfo,
    pub saved_config: Map<String, Value>,
}

impl Mod {
    pub fn load(dir: &Path) -> Result<Self> {
        let id = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::msg(format!("mod folder has no name: {}", dir.display())))?
            .to_string();

        let manifest_path = dir.join("mod.json");
        let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::msg(format!(
                    "'{id}' is not a mod: no mod.json in {}",
                    dir.display()
                ))
            } else {
                Error::io(&manifest_path, e)
            }
        })?;
        // Manifests in the wild carry comments and trailing commas, same as game
        // JSON, so they get the tolerant parser.
        let info: ModInfo = match crate::jsonc::parse(&text) {
            Ok(value) => serde_json::from_value(value).map_err(|e| {
                Error::msg(format!("{}: invalid mod.json: {e}", manifest_path.display()))
            })?,
            Err(e) => {
                return Err(Error::msg(format!(
                    "{}: invalid mod.json: {e}",
                    manifest_path.display()
                )))
            }
        };

        let saved_config = match std::fs::read_to_string(dir.join("config.json")) {
            Ok(text) => match crate::jsonc::parse(&text) {
                Ok(Value::Object(map)) => map,
                _ => Map::new(),
            },
            Err(_) => Map::new(),
        };

        Ok(Self {
            id,
            dir: dir.to_path_buf(),
            info,
            saved_config,
        })
    }

    pub fn display_name(&self) -> &str {
        if self.info.name.is_empty() {
            &self.id
        } else {
            &self.info.name
        }
    }

    pub fn version(&self) -> &str {
        self.info.version.as_deref().unwrap_or("unversioned")
    }

    /// The mod's config, with defaults resolved.
    pub fn config(&self) -> ModConfig {
        ModConfig::new(self.info.config.clone(), self.saved_config.clone())
    }

    pub fn save_config(&mut self, config: &ModConfig) -> Result<()> {
        let path = self.dir.join("config.json");
        let value = Value::Object(config.saved().clone());
        let text = crate::jsonc::encode(&value)?;
        io_at(&path, std::fs::write(&path, text))?;
        self.saved_config = config.saved().clone();
        Ok(())
    }

    /// The JavaScript to run, and whether it came from `mod.js`.
    pub fn code(&self) -> Result<String> {
        let js_path = self.dir.join("mod.js");
        if js_path.is_file() {
            return std::fs::read_to_string(&js_path).map_err(|e| {
                Error::Mod {
                    mod_id: self.id.clone(),
                    message: format!("could not read mod.js: {e}"),
                }
            });
        }
        if self.dir.join("mod.ts").is_file() {
            return Err(Error::Mod {
                mod_id: self.id.clone(),
                message: concat!(
                    "this mod ships only TypeScript (mod.ts). D2rlmm runs mod.js; ",
                    "TypeScript transpilation is not implemented yet. Ask the mod ",
                    "author for a mod.js build, or use D2RMM for this mod."
                )
                .to_string(),
            });
        }
        Err(Error::Mod {
            mod_id: self.id.clone(),
            message: "no mod.js in the mod folder".to_string(),
        })
    }

    pub fn has_code(&self) -> bool {
        self.dir.join("mod.js").is_file()
    }
}

/// Loads every mod in the library, skipping folders that are not mods.
///
/// A broken mod is reported as an error rather than silently dropped: "the mod I
/// just installed is not in the list" is a worse experience than a red row that
/// says why.
pub fn discover(mods_dir: &Path) -> (Vec<Mod>, Vec<Error>) {
    let mut mods = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(mods_dir) else {
        return (mods, errors);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match Mod::load(&path) {
            Ok(m) => mods.push(m),
            Err(e) => errors.push(e),
        }
    }
    mods.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
    (mods, errors)
}

/// Sanitises a mod id so it is safe as a single path component.
pub fn sanitize_id(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        "mod".to_string()
    } else {
        cleaned
    }
}

/// Imports a mod from a `.zip`, returning the new mod id.
///
/// Archives in the wild come in two shapes: `mod.json` at the root, or everything
/// inside a single top-level folder. Both are handled by finding `mod.json` and
/// taking its parent as the mod root, so a zip that wraps its contents the way
/// GitHub's "download ZIP" does still installs correctly.
pub fn import_zip(archive: &Path, mods_dir: &Path) -> Result<String> {
    let file = std::fs::File::open(archive).map_err(|e| Error::io(archive, e))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| Error::msg(format!("{} is not a readable zip: {e}", archive.display())))?;

    // Locate the shallowest mod.json; its folder is the mod root.
    let mut root: Option<String> = None;
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|e| Error::msg(format!("{}: {e}", archive.display())))?;
        let name = entry
            .name()
            .map_err(|e| Error::msg(format!("{}: {e}", archive.display())))?
            .replace('\\', "/");
        if name.ends_with("mod.json") {
            let parent = name.trim_end_matches("mod.json").trim_end_matches('/');
            let depth = if parent.is_empty() {
                0
            } else {
                parent.matches('/').count() + 1
            };
            let better = match &root {
                None => true,
                Some(existing) => {
                    let existing_depth = if existing.is_empty() {
                        0
                    } else {
                        existing.matches('/').count() + 1
                    };
                    depth < existing_depth
                }
            };
            if better {
                root = Some(parent.to_string());
            }
        }
    }

    let Some(root) = root else {
        return Err(Error::msg(format!(
            "{} contains no mod.json — this does not look like a D2RMM mod",
            archive.display()
        )));
    };

    let id_seed = if root.is_empty() {
        archive
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("mod")
            .to_string()
    } else {
        root.rsplit('/').next().unwrap_or("mod").to_string()
    };
    let id = sanitize_id(&id_seed);
    let destination = mods_dir.join(&id);

    // Replace an existing install of the same mod, which is what re-importing means.
    if destination.exists() {
        std::fs::remove_dir_all(&destination).map_err(|e| Error::io(&destination, e))?;
    }
    std::fs::create_dir_all(&destination).map_err(|e| Error::io(&destination, e))?;

    let prefix = if root.is_empty() {
        String::new()
    } else {
        format!("{root}/")
    };
    let mut extracted = 0usize;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| Error::msg(format!("{}: {e}", archive.display())))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|e| Error::msg(format!("{}: {e}", archive.display())))?
            .replace('\\', "/");
        let Some(relative) = name.strip_prefix(&prefix) else {
            continue;
        };
        if relative.is_empty() {
            continue;
        }
        // Archive entries can try to escape; refuse anything that is not a plain
        // relative path.
        let target_rel = Path::new(relative);
        if target_rel.is_absolute()
            || target_rel
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            continue;
        }
        let target = destination.join(target_rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let mut out = std::fs::File::create(&target).map_err(|e| Error::io(&target, e))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| Error::io(&target, e))?;
        extracted += 1;
    }

    if extracted == 0 {
        let _ = std::fs::remove_dir_all(&destination);
        return Err(Error::msg(format!(
            "{} had a mod.json but no files under it",
            archive.display()
        )));
    }
    // Fail early if the result is not loadable, rather than leaving a broken folder
    // in the library.
    if let Err(e) = Mod::load(&destination) {
        let _ = std::fs::remove_dir_all(&destination);
        return Err(e);
    }
    Ok(id)
}

/// Removes a mod folder.
pub fn remove(mods_dir: &Path, id: &str) -> Result<()> {
    let dir = mods_dir.join(sanitize_id(id));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    }
    Ok(())
}

/// Creates a mod folder from an unpacked directory (used by "install from folder").
pub fn import_dir(source: &Path, mods_dir: &Path) -> Result<String> {
    if !source.join("mod.json").is_file() {
        return Err(Error::msg(format!(
            "{} has no mod.json — pick the mod's own folder",
            source.display()
        )));
    }
    let id_seed = source
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("mod")
        .to_string();
    let id = sanitize_id(&id_seed);
    let destination = mods_dir.join(&id);
    if destination.exists() {
        std::fs::remove_dir_all(&destination).map_err(|e| Error::io(&destination, e))?;
    }
    copy_tree(source, &destination)?;
    Mod::load(&destination)?;
    Ok(id)
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|e| Error::io(to, e))?;
    for entry in walkdir::WalkDir::new(from).min_depth(1) {
        let entry = entry.map_err(|e| Error::msg(format!("walking {}: {e}", from.display())))?;
        let rel = entry
            .path()
            .strip_prefix(from)
            .map_err(|_| Error::msg("path escaped its root".to_string()))?;
        let target = to.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| Error::io(&target, e))?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            std::fs::copy(entry.path(), &target).map_err(|e| Error::io(&target, e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_ids() {
        assert_eq!(sanitize_id("Expanded Stash"), "Expanded Stash");
        // Leading dots are stripped: a folder called `..` is not a folder.
        assert_eq!(sanitize_id("../evil"), "_evil");
        assert_eq!(sanitize_id(""), "mod");
    }

    #[test]
    fn loads_a_mod_and_its_config() {
        let dir = std::env::temp_dir().join("d2rlmm-mod-load-test/MyMod");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mod.json"),
            r#"{ "name": "My Mod", "version": "1.2", "config": [
                { "id": "n", "type": "number", "name": "N", "defaultValue": 3 }
            ] }"#,
        )
        .unwrap();
        std::fs::write(dir.join("mod.js"), "// noop").unwrap();
        std::fs::write(dir.join("config.json"), r#"{"n": 9}"#).unwrap();

        let m = Mod::load(&dir).unwrap();
        assert_eq!(m.id, "MyMod");
        assert_eq!(m.display_name(), "My Mod");
        assert_eq!(m.version(), "1.2");
        assert_eq!(m.config().get("n"), Some(&serde_json::json!(9)));
        assert!(m.has_code());
        assert!(m.code().is_ok());
    }

    #[test]
    fn reports_typescript_only_mods_clearly() {
        let dir = std::env::temp_dir().join("d2rlmm-mod-ts-test/TsMod");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mod.json"), r#"{"name":"TS"}"#).unwrap();
        std::fs::write(dir.join("mod.ts"), "export const x = 1;").unwrap();

        let m = Mod::load(&dir).unwrap();
        let err = m.code().unwrap_err().to_string();
        assert!(err.contains("TypeScript"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_a_folder_without_a_manifest() {
        let dir = std::env::temp_dir().join("d2rlmm-mod-missing/NotAMod");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let err = Mod::load(&dir).unwrap_err().to_string();
        assert!(err.contains("no mod.json"), "unexpected error: {err}");
    }
}

use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LoaderKind {
    Fabric,
    Forge,
    NeoForge,
    Unknown,
}

impl LoaderKind {
    pub fn from_args(args: &[String]) -> Self {
        let target = args.iter().enumerate().find_map(|(i, arg)| {
            if arg == "--target-loader" {
                args.get(i + 1).map(String::as_str)
            } else {
                arg.strip_prefix("--target-loader=")
            }
        });

        let explicit_mode = args
            .windows(2)
            .find(|pair| pair[0] == "--mode")
            .map(|pair| pair[1].as_str())
            .or_else(|| args.iter().find_map(|arg| arg.strip_prefix("--mode=")));

        let positional_mode =
            args.iter()
                .skip(1)
                .find_map(|arg| match arg.to_ascii_lowercase().as_str() {
                    "fabric" | "forge" | "neoforge" => Some(arg.as_str()),
                    _ => None,
                });

        match target
            .or(explicit_mode)
            .or(positional_mode)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("fabric") => Self::Fabric,
            Some("forge") => Self::Forge,
            Some("neoforge") => Self::NeoForge,
            Some(_) => Self::Unknown,
            None => Self::Fabric,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDependency {
    pub mod_id: String,
    pub mandatory: bool,
    pub version_requirement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModMetadata {
    pub id: String,
    pub name: String,
    pub version: String,
    pub jar_path: PathBuf,
    pub loaders: BTreeSet<LoaderKind>,
    pub minecraft_requirement: Option<String>,
    pub dependencies: Vec<ModDependency>,
    pub conflicts: Vec<String>,
    pub metadata_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanIssue {
    pub jar_path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModScanReport {
    pub mods: Vec<ModMetadata>,
    pub issues: Vec<ScanIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignmentStatus {
    Assigned(LoaderKind),
    Rejected(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModAssignment {
    pub mod_id: String,
    pub jar_path: PathBuf,
    pub status: AssignmentStatus,
}

const MAX_METADATA_BYTES: u64 = 4 * 1024 * 1024;

fn read_metadata<R: Read>(reader: &mut R) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("metadata read failed: {e}"))?;

    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err("metadata exceeds 4 MiB limit".to_owned());
    }

    String::from_utf8(bytes).map_err(|e| format!("metadata is not UTF-8: {e}"))
}

fn valid_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
        })
}

fn fabric_version_requirement(value: &JsonValue, field: &str) -> Result<String, String> {
    match value {
        JsonValue::String(requirement) if !requirement.trim().is_empty() => Ok(requirement.clone()),
        JsonValue::Array(items) if !items.is_empty() => {
            let mut alternatives = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let requirement = item.as_str().ok_or_else(|| {
                    format!("{field}[{index}] must be a string version requirement")
                })?;
                if requirement.trim().is_empty() {
                    return Err(format!("{field}[{index}] must not be empty"));
                }
                alternatives.push(requirement.to_owned());
            }
            Ok(alternatives.join(" || "))
        }
        JsonValue::String(_) => Err(format!("{field} version requirement must not be empty")),
        JsonValue::Array(_) => Err(format!("{field} version alternatives must not be empty")),
        _ => Err(format!("{field} must be a string or array of strings")),
    }
}

fn fabric_dependencies(value: &JsonValue) -> Result<Vec<ModDependency>, String> {
    let mut result = Vec::new();

    for (field, mandatory) in [("depends", true), ("recommends", false)] {
        let Some(raw_dependencies) = value.get(field) else {
            continue;
        };
        let dependencies = raw_dependencies
            .as_object()
            .ok_or_else(|| format!("Fabric {field} must be an object"))?;

        for (id, requirement) in dependencies {
            if !valid_mod_id(id) {
                return Err(format!("Fabric {field} contains invalid Mod ID: {id}"));
            }

            let version_requirement =
                fabric_version_requirement(requirement, &format!("Fabric {field}.{id}"))?;

            if matches!(id.as_str(), "minecraft" | "java" | "fabricloader") {
                continue;
            }

            result.push(ModDependency {
                mod_id: id.clone(),
                mandatory,
                version_requirement: Some(version_requirement),
            });
        }
    }

    Ok(result)
}

fn parse_fabric(path: &Path, raw: &str) -> Result<ModMetadata, String> {
    let value: JsonValue =
        serde_json::from_str(raw).map_err(|e| format!("invalid fabric.mod.json: {e}"))?;

    let id = value
        .get("id")
        .and_then(JsonValue::as_str)
        .ok_or("Fabric metadata missing string id")?
        .to_owned();

    if !valid_mod_id(&id) {
        return Err(format!("invalid Mod ID: {id}"));
    }

    let version = value
        .get("version")
        .and_then(JsonValue::as_str)
        .ok_or("Fabric metadata missing string version")?
        .to_owned();

    let name = value
        .get("name")
        .and_then(JsonValue::as_str)
        .unwrap_or(&id)
        .to_owned();

    let mut minecraft_requirement = None;
    if let Some(depends) = value.get("depends") {
        let dependencies = depends
            .as_object()
            .ok_or("Fabric depends must be an object")?;

        if let Some(requirement) = dependencies.get("minecraft") {
            minecraft_requirement = Some(fabric_version_requirement(
                requirement,
                "Fabric depends.minecraft",
            )?);
        }
    }

    let mut conflicts = Vec::new();
    if let Some(raw_breaks) = value.get("breaks") {
        let items = raw_breaks
            .as_object()
            .ok_or("Fabric breaks must be an object")?;

        for (conflict_id, requirement) in items {
            if !valid_mod_id(conflict_id) {
                return Err(format!(
                    "Fabric breaks contains invalid Mod ID: {conflict_id}"
                ));
            }

            // Validate the predicate even though v1 planning is ID-based.
            let _ =
                fabric_version_requirement(requirement, &format!("Fabric breaks.{conflict_id}"))?;
            conflicts.push(conflict_id.clone());
        }
    }

    Ok(ModMetadata {
        id,
        name,
        version,
        jar_path: path.to_path_buf(),
        loaders: [LoaderKind::Fabric].into_iter().collect(),
        minecraft_requirement,
        dependencies: fabric_dependencies(&value)?,
        conflicts,
        metadata_source: "fabric.mod.json".to_owned(),
    })
}

fn parse_toml(path: &Path, raw: &str, neoforge: bool) -> Result<ModMetadata, String> {
    let value: toml::Value =
        toml::from_str(raw).map_err(|e| format!("invalid TOML metadata: {e}"))?;

    let mods = value
        .get("mods")
        .and_then(toml::Value::as_array)
        .ok_or("TOML metadata missing [[mods]]")?;

    if mods.len() != 1 {
        return Err("JAR must declare exactly one mod in [[mods]] for v1 assignment".to_owned());
    }

    let entry = &mods[0];
    let id = entry
        .get("modId")
        .and_then(toml::Value::as_str)
        .ok_or("TOML metadata missing string modId")?
        .to_owned();

    if !valid_mod_id(&id) {
        return Err(format!("invalid Mod ID: {id}"));
    }

    let version = match entry.get("version") {
        None => "${file.jarVersion}".to_owned(),
        Some(value) => value
            .as_str()
            .ok_or("TOML metadata version must be a string")?
            .to_owned(),
    };

    let name = match entry.get("displayName") {
        None => id.clone(),
        Some(value) => value
            .as_str()
            .ok_or("TOML metadata displayName must be a string")?
            .to_owned(),
    };

    let mut dependencies = Vec::new();
    let mut conflicts = Vec::new();

    if let Some(tables_value) = value.get("dependencies") {
        let tables = tables_value
            .as_table()
            .ok_or("TOML dependencies must be a table")?;

        if let Some(entries_value) = tables.get(&id) {
            let entries = entries_value
                .as_array()
                .ok_or("TOML Mod dependencies must be an array")?;

            for (index, dependency) in entries.iter().enumerate() {
                let dep_id = dependency
                    .get("modId")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| format!("dependency #{index} missing string modId"))?;

                if dep_id != "minecraft" && !valid_mod_id(dep_id) {
                    return Err(format!("dependency #{index} has invalid Mod ID: {dep_id}"));
                }

                let mandatory = match dependency.get("mandatory") {
                    None => true,
                    Some(value) => value
                        .as_bool()
                        .ok_or_else(|| format!("dependency {dep_id} mandatory must be boolean"))?,
                };

                let version_requirement = match dependency.get("versionRange") {
                    None => None,
                    Some(value) => Some(
                        value
                            .as_str()
                            .ok_or_else(|| {
                                format!("dependency {dep_id} versionRange must be a string")
                            })?
                            .to_owned(),
                    ),
                };

                let dependency_type = match dependency.get("type") {
                    None => None,
                    Some(value) => Some(
                        value
                            .as_str()
                            .ok_or_else(|| format!("dependency {dep_id} type must be a string"))?,
                    ),
                };

                if dependency_type == Some("incompatible") {
                    conflicts.push(dep_id.to_owned());
                } else {
                    dependencies.push(ModDependency {
                        mod_id: dep_id.to_owned(),
                        mandatory,
                        version_requirement,
                    });
                }
            }
        }
    }

    let minecraft_requirement = dependencies
        .iter()
        .find(|dep| dep.mod_id == "minecraft")
        .and_then(|dep| dep.version_requirement.clone());

    Ok(ModMetadata {
        id,
        name,
        version,
        jar_path: path.to_path_buf(),
        loaders: [if neoforge {
            LoaderKind::NeoForge
        } else {
            LoaderKind::Forge
        }]
        .into_iter()
        .collect(),
        minecraft_requirement,
        dependencies,
        conflicts,
        metadata_source: if neoforge {
            "META-INF/neoforge.mods.toml".to_owned()
        } else {
            "META-INF/mods.toml".to_owned()
        },
    })
}

fn normalize_semver(input: &str) -> Result<semver::Version, String> {
    let input = input.trim();
    if let Ok(version) = semver::Version::parse(input) {
        return Ok(version);
    }

    let (core, suffix) = match input.find(['+', '-']) {
        Some(index) => (&input[..index], &input[index..]),
        None => (input, ""),
    };

    let components: Vec<_> = core.split('.').collect();
    if components.is_empty()
        || components.len() > 3
        || components
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(format!("unsupported version syntax: {input}"));
    }

    let mut normalized = components.join(".");
    for _ in components.len()..3 {
        normalized.push_str(".0");
    }
    normalized.push_str(suffix);

    semver::Version::parse(&normalized).map_err(|_| format!("unsupported version syntax: {input}"))
}

fn fabric_version_matches(actual: &str, requirement: &str) -> Result<bool, String> {
    if requirement.trim() == "*" {
        return Ok(true);
    }

    let actual = normalize_semver(actual)?;
    let alternatives: Vec<_> = requirement.split("||").collect();

    if alternatives.is_empty() {
        return Err("empty Fabric version requirement".to_owned());
    }

    for alternative in alternatives {
        let alternative = alternative.trim();
        if alternative.is_empty() {
            return Err("empty alternative in Fabric version requirement".to_owned());
        }

        let requirement = semver::VersionReq::parse(alternative)
            .map_err(|error| format!("unsupported Fabric version requirement: {error}"))?;

        if requirement.matches(&actual) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn maven_version_matches(actual: &str, requirement: &str) -> Result<bool, String> {
    let requirement = requirement.trim();
    if requirement.is_empty() {
        return Err("empty Maven version requirement".to_owned());
    }

    let actual = normalize_semver(actual)?;

    let starts_interval = requirement.starts_with('[') || requirement.starts_with('(');
    let ends_interval = requirement.ends_with(']') || requirement.ends_with(')');

    if !starts_interval && !ends_interval {
        let expected = normalize_semver(requirement)?;
        return Ok(actual == expected);
    }

    if !(starts_interval && ends_interval) {
        return Err(format!("malformed Maven version range: {requirement}"));
    }

    let lower_inclusive = requirement.starts_with('[');
    let upper_inclusive = requirement.ends_with(']');
    let inner = &requirement[1..requirement.len() - 1];

    if !inner.contains(',') {
        let expected = normalize_semver(inner)?;
        return Ok(actual == expected);
    }

    let mut bounds = inner.split(',');
    let lower = bounds.next().unwrap_or("").trim();
    let upper = bounds.next().unwrap_or("").trim();

    if bounds.next().is_some() {
        return Err(format!("unsupported Maven version range: {requirement}"));
    }

    let lower_ok = if lower.is_empty() {
        true
    } else {
        let bound = normalize_semver(lower)?;
        if lower_inclusive {
            actual >= bound
        } else {
            actual > bound
        }
    };

    let upper_ok = if upper.is_empty() {
        true
    } else {
        let bound = normalize_semver(upper)?;
        if upper_inclusive {
            actual <= bound
        } else {
            actual < bound
        }
    };

    Ok(lower_ok && upper_ok)
}

fn dependency_version_matches(
    loader: LoaderKind,
    actual: &str,
    requirement: &str,
) -> Result<bool, String> {
    match loader {
        LoaderKind::Fabric => fabric_version_matches(actual, requirement),
        LoaderKind::Forge | LoaderKind::NeoForge => maven_version_matches(actual, requirement),
        LoaderKind::Unknown => Err("cannot validate version for unknown Loader".to_owned()),
    }
}

/// Inspect original central-directory records before ZipArchive's name map
/// can collapse duplicate filenames.
fn reject_duplicate_metadata_entries(
    path: &Path,
    central_directory_start: u64,
) -> Result<(), String> {
    let mut file = File::open(path)
        .map_err(|error| format!("cannot reopen JAR central directory: {error}"))?;
    let file_len = file
        .metadata()
        .map_err(|error| format!("cannot inspect JAR length: {error}"))?
        .len();

    if central_directory_start >= file_len {
        return Err("invalid ZIP central directory offset".to_owned());
    }

    file.seek(SeekFrom::Start(central_directory_start))
        .map_err(|error| format!("cannot seek to ZIP central directory: {error}"))?;

    let mut fabric_count = 0usize;
    let mut forge_count = 0usize;
    let mut neo_count = 0usize;

    loop {
        let position = file
            .stream_position()
            .map_err(|error| format!("cannot read ZIP directory position: {error}"))?;

        if position.checked_add(4).is_none_or(|end| end > file_len) {
            return Err("truncated ZIP central directory".to_owned());
        }

        let mut signature = [0u8; 4];
        file.read_exact(&mut signature)
            .map_err(|error| format!("cannot read ZIP directory signature: {error}"))?;

        let magic = u32::from_le_bytes(signature);

        match magic {
            // Central directory file header.
            0x02014b50 => {
                let mut header = [0u8; 42];
                file.read_exact(&mut header)
                    .map_err(|error| format!("truncated ZIP central directory header: {error}"))?;

                let name_len = u16::from_le_bytes([header[24], header[25]]) as u64;
                let extra_len = u16::from_le_bytes([header[26], header[27]]) as u64;
                let comment_len = u16::from_le_bytes([header[28], header[29]]) as u64;

                let variable_len = name_len
                    .checked_add(extra_len)
                    .and_then(|value| value.checked_add(comment_len))
                    .ok_or_else(|| "ZIP central directory length overflow".to_owned())?;

                let variable_end = file
                    .stream_position()
                    .map_err(|error| format!("cannot inspect ZIP directory position: {error}"))?
                    .checked_add(variable_len)
                    .ok_or_else(|| "ZIP central directory offset overflow".to_owned())?;

                if variable_end > file_len {
                    return Err("truncated ZIP central directory variable fields".to_owned());
                }

                let mut name = vec![0u8; name_len as usize];
                file.read_exact(&mut name)
                    .map_err(|error| format!("cannot read ZIP entry name: {error}"))?;

                match name.as_slice() {
                    b"fabric.mod.json" => fabric_count += 1,
                    b"META-INF/mods.toml" => forge_count += 1,
                    b"META-INF/neoforge.mods.toml" => neo_count += 1,
                    _ => {}
                }

                if fabric_count > 1 || forge_count > 1 || neo_count > 1 {
                    return Err("duplicate Mod metadata ZIP entry".to_owned());
                }

                file.seek(SeekFrom::Start(variable_end))
                    .map_err(|error| format!("cannot skip ZIP directory fields: {error}"))?;
            }

            // ZIP64 end of central directory or classic EOCD.
            0x06064b50 | 0x06054b50 => return Ok(()),

            // Optional central directory digital signature.
            0x05054b50 => {
                let mut length = [0u8; 2];
                file.read_exact(&mut length)
                    .map_err(|error| format!("truncated ZIP directory signature: {error}"))?;

                let length = u16::from_le_bytes(length) as u64;
                let end = file
                    .stream_position()
                    .map_err(|error| format!("cannot inspect ZIP directory position: {error}"))?
                    .checked_add(length)
                    .ok_or_else(|| "ZIP directory signature offset overflow".to_owned())?;

                if end > file_len {
                    return Err("truncated ZIP directory signature data".to_owned());
                }

                file.seek(SeekFrom::Start(end))
                    .map_err(|error| format!("cannot skip ZIP directory signature: {error}"))?;
            }

            _ => {
                return Err(format!(
                    "invalid ZIP central directory record signature at offset {position}"
                ));
            }
        }
    }
}

pub fn scan_mod_jar(path: &Path) -> Result<ModMetadata, String> {
    let file = File::open(path).map_err(|e| format!("cannot open JAR: {e}"))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("invalid JAR/ZIP: {e}"))?;

    reject_duplicate_metadata_entries(path, archive.central_directory_start())?;

    let mut fabric_count = 0usize;
    let mut forge_count = 0usize;
    let mut neo_count = 0usize;

    // Enumerate central-directory entries by index. file_names() may
    // expose deduplicated names and is therefore unsuitable for duplicate
    // metadata detection.
    for index in 0..archive.len() {
        let name = archive
            .by_index(index)
            .map_err(|error| format!("cannot inspect ZIP entry {index}: {error}"))?
            .name()
            .to_owned();

        match name.as_str() {
            "fabric.mod.json" => fabric_count += 1,
            "META-INF/mods.toml" => forge_count += 1,
            "META-INF/neoforge.mods.toml" => neo_count += 1,
            _ => {}
        }
    }

    if fabric_count > 1 || forge_count > 1 || neo_count > 1 {
        return Err("duplicate Mod metadata ZIP entry".to_owned());
    }

    let fabric = fabric_count == 1;
    let forge = forge_count == 1;
    let neo = neo_count == 1;

    let count = usize::from(fabric) + usize::from(forge) + usize::from(neo);
    if count == 0 {
        return Err("no supported Mod metadata found".to_owned());
    }
    if count > 1 {
        return Err("conflicting or multiple Loader metadata files".to_owned());
    }

    if fabric {
        let mut entry = archive
            .by_name("fabric.mod.json")
            .map_err(|e| format!("cannot open Fabric metadata: {e}"))?;
        let raw = read_metadata(&mut entry)?;
        parse_fabric(path, &raw)
    } else if neo {
        let mut entry = archive
            .by_name("META-INF/neoforge.mods.toml")
            .map_err(|e| format!("cannot open NeoForge metadata: {e}"))?;
        let raw = read_metadata(&mut entry)?;
        parse_toml(path, &raw, true)
    } else {
        let mut entry = archive
            .by_name("META-INF/mods.toml")
            .map_err(|e| format!("cannot open Forge metadata: {e}"))?;
        let raw = read_metadata(&mut entry)?;
        parse_toml(path, &raw, false)
    }
}

pub fn scan_mod_directories(directories: &[PathBuf]) -> ModScanReport {
    let mut report = ModScanReport {
        mods: Vec::new(),
        issues: Vec::new(),
    };

    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                report.issues.push(ScanIssue {
                    jar_path: directory.clone(),
                    reason: format!("cannot read Mod directory: {error}"),
                });
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.issues.push(ScanIssue {
                        jar_path: directory.clone(),
                        reason: format!("directory entry error: {error}"),
                    });
                    continue;
                }
            };

            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    report.issues.push(ScanIssue {
                        jar_path: path,
                        reason: format!("cannot inspect directory entry type: {error}"),
                    });
                    continue;
                }
            };

            // DirEntry::file_type does not follow symlinks.
            if !file_type.is_file()
                || !path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
            {
                continue;
            }

            match scan_mod_jar(&path) {
                Ok(metadata) => report.mods.push(metadata),
                Err(reason) => report.issues.push(ScanIssue {
                    jar_path: path,
                    reason,
                }),
            }
        }
    }

    report.mods.sort_by(|a, b| a.id.cmp(&b.id));
    report
}

pub fn plan_assignments(report: &ModScanReport, target_loader: LoaderKind) -> Vec<ModAssignment> {
    let mut counts = BTreeMap::<String, usize>::new();
    for metadata in &report.mods {
        *counts.entry(metadata.id.clone()).or_default() += 1;
    }

    // Only unique Mods explicitly supporting the requested Loader can satisfy dependencies.
    let compatible: BTreeMap<&str, &ModMetadata> = report
        .mods
        .iter()
        .filter(|metadata| {
            counts.get(&metadata.id).copied() == Some(1)
                && metadata.loaders.contains(&target_loader)
        })
        .map(|metadata| (metadata.id.as_str(), metadata))
        .collect();

    report
        .mods
        .iter()
        .map(|metadata| {
            let status = if counts.get(&metadata.id).copied().unwrap_or(0) != 1 {
                AssignmentStatus::Rejected("duplicate Mod ID".to_owned())
            } else if !metadata.loaders.contains(&target_loader) {
                AssignmentStatus::Rejected(format!(
                    "Mod does not declare support for target Loader {}",
                    target_loader.as_str()
                ))
            } else {
                let mut rejection: Option<String> = None;

                for dependency in metadata.dependencies.iter().filter(|dep| dep.mandatory) {
                    // Minecraft's version is retained in metadata, but the active game version
                    // is not yet supplied to this planner.
                    if dependency.mod_id == "minecraft" {
                        continue;
                    }

                    let Some(installed) = compatible.get(dependency.mod_id.as_str()) else {
                        rejection = Some(format!(
                            "missing required dependency compatible with target Loader: {}",
                            dependency.mod_id
                        ));
                        break;
                    };

                    if let Some(requirement) = dependency.version_requirement.as_deref() {
                        match dependency_version_matches(
                            target_loader,
                            &installed.version,
                            requirement,
                        ) {
                            Ok(true) => {}
                            Ok(false) => {
                                rejection = Some(format!(
                                    "dependency {} version {} does not satisfy {}",
                                    dependency.mod_id, installed.version, requirement
                                ));
                                break;
                            }
                            Err(error) => {
                                rejection = Some(format!(
                                    "cannot safely validate dependency {}: {}",
                                    dependency.mod_id, error
                                ));
                                break;
                            }
                        }
                    }
                }

                if rejection.is_none() {
                    if let Some(conflict) = metadata
                        .conflicts
                        .iter()
                        .find(|id| compatible.contains_key(id.as_str()))
                    {
                        rejection =
                            Some(format!("declared conflict with installed Mod: {conflict}"));
                    }
                }

                match rejection {
                    Some(reason) => AssignmentStatus::Rejected(reason),
                    None => AssignmentStatus::Assigned(target_loader),
                }
            };

            ModAssignment {
                mod_id: metadata.id.clone(),
                jar_path: metadata.jar_path.clone(),
                status,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn test_path(label: &str) -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("jvm-combiner-{}-{id}-{label}", std::process::id()))
    }

    fn make_jar(entries: &[(&str, &str)]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        for (name, content) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut writer, content.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn parses_fabric_metadata() {
        let bytes = make_jar(&[(
            "fabric.mod.json",
            r#"{"schemaVersion":1,"id":"example_mod","version":"1.0","name":"Example","depends":{"minecraft":"~1.21.4","fabricloader":">=0.16","helper":"*"} }"#,
        )]);
        let path = std::env::temp_dir().join("jvm-combiner-fabric-test.jar");
        std::fs::write(&path, bytes).unwrap();

        let metadata = scan_mod_jar(&path).unwrap();
        assert_eq!(metadata.id, "example_mod");
        assert_eq!(metadata.loaders, [LoaderKind::Fabric].into_iter().collect());
        assert!(metadata.dependencies.iter().any(|d| d.mod_id == "helper"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parses_neoforge_metadata() {
        let bytes = make_jar(&[(
            "META-INF/neoforge.mods.toml",
            r#"modLoader="javafml"
loaderVersion="[4,)"
[[mods]]
modId="example_neo"
version="1.0"
displayName="Example Neo"
[[dependencies.example_neo]]
modId="minecraft"
mandatory=true
versionRange="[1.21.4]"
"#,
        )]);
        let path = std::env::temp_dir().join("jvm-combiner-neo-test.jar");
        std::fs::write(&path, bytes).unwrap();

        let metadata = scan_mod_jar(&path).unwrap();
        assert_eq!(metadata.id, "example_neo");
        assert!(metadata.loaders.contains(&LoaderKind::NeoForge));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_ambiguous_metadata() {
        let bytes = make_jar(&[
            ("fabric.mod.json", r#"{"id":"one","version":"1"}"#),
            (
                "META-INF/mods.toml",
                r#"[[mods]]
modId="one"
version="1"
"#,
            ),
        ]);
        let path = std::env::temp_dir().join("jvm-combiner-ambiguous-test.jar");
        std::fs::write(&path, bytes).unwrap();

        assert!(scan_mod_jar(&path).unwrap_err().contains("multiple Loader"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_malformed_fabric_dependency_shapes() {
        let path = PathBuf::from("malformed-fabric.jar");
        let cases = [
            r#"{"id":"bad_dep","version":"1","depends":[]}"#,
            r#"{"id":"bad_dep","version":"1","depends":{"helper":7}}"#,
            r#"{"id":"bad_dep","version":"1","depends":{"helper":["*","",">=1"]}}"#,
            r#"{"id":"bad_dep","version":"1","recommends":null}"#,
            r#"{"id":"bad_dep","version":"1","breaks":{"helper":false}}"#,
        ];

        for raw in cases {
            assert!(
                parse_fabric(&path, raw).is_err(),
                "malformed metadata was accepted: {raw}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_metadata_zip_entries() {
        fn put_u16(out: &mut Vec<u8>, value: u16) {
            out.extend_from_slice(&value.to_le_bytes());
        }

        fn put_u32(out: &mut Vec<u8>, value: u32) {
            out.extend_from_slice(&value.to_le_bytes());
        }

        fn crc32(data: &[u8]) -> u32 {
            let mut crc = !0u32;
            for &byte in data {
                crc ^= byte as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 {
                        (crc >> 1) ^ 0xEDB88320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }

        // Construct a raw ZIP because ZipWriter intentionally rejects duplicate names.
        let name = b"fabric.mod.json";
        let payload = br#"{"id":"duplicate","version":"1"}"#;
        let checksum = crc32(payload);

        let mut bytes = Vec::new();
        let mut central = Vec::new();

        for _ in 0..2 {
            let local_offset = bytes.len() as u32;

            // Local file header.
            put_u32(&mut bytes, 0x04034b50);
            put_u16(&mut bytes, 20);
            put_u16(&mut bytes, 0);
            put_u16(&mut bytes, 0);
            put_u16(&mut bytes, 0);
            put_u16(&mut bytes, 0);
            put_u32(&mut bytes, checksum);
            put_u32(&mut bytes, payload.len() as u32);
            put_u32(&mut bytes, payload.len() as u32);
            put_u16(&mut bytes, name.len() as u16);
            put_u16(&mut bytes, 0);
            bytes.extend_from_slice(name);
            bytes.extend_from_slice(payload);

            // Central directory entry.
            put_u32(&mut central, 0x02014b50);
            put_u16(&mut central, 20);
            put_u16(&mut central, 20);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u32(&mut central, checksum);
            put_u32(&mut central, payload.len() as u32);
            put_u32(&mut central, payload.len() as u32);
            put_u16(&mut central, name.len() as u16);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u32(&mut central, 0);
            put_u32(&mut central, local_offset);
            central.extend_from_slice(name);
        }

        let central_offset = bytes.len() as u32;
        let central_size = central.len() as u32;
        bytes.extend_from_slice(&central);

        // End of central directory.
        put_u32(&mut bytes, 0x06054b50);
        put_u16(&mut bytes, 0);
        put_u16(&mut bytes, 0);
        put_u16(&mut bytes, 2);
        put_u16(&mut bytes, 2);
        put_u32(&mut bytes, central_size);
        put_u32(&mut bytes, central_offset);
        put_u16(&mut bytes, 0);

        let path = test_path("duplicate-metadata.jar");
        std::fs::write(&path, bytes).unwrap();

        let result = scan_mod_jar(&path);
        assert!(
            result.unwrap_err().contains("duplicate Mod metadata"),
            "scanner must reject duplicate metadata ZIP entries"
        );

        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn directory_scan_ignores_jar_symlinks() {
        use std::os::unix::fs::symlink;

        let root = test_path("symlink-scan");
        let outside = test_path("outside-valid.jar");
        std::fs::create_dir_all(&root).unwrap();

        std::fs::write(
            &outside,
            make_jar(&[("fabric.mod.json", r#"{"id":"outside_mod","version":"1"}"#)]),
        )
        .unwrap();

        symlink(&outside, root.join("linked.jar")).unwrap();

        let report = scan_mod_directories(std::slice::from_ref(&root));
        assert!(report.mods.is_empty());
        assert!(report.issues.is_empty());

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(outside);
    }

    fn test_metadata(
        id: &str,
        version: &str,
        loader: LoaderKind,
        dependencies: Vec<ModDependency>,
        conflicts: Vec<String>,
    ) -> ModMetadata {
        ModMetadata {
            id: id.to_owned(),
            name: id.to_owned(),
            version: version.to_owned(),
            jar_path: PathBuf::from(format!("{id}.jar")),
            loaders: [loader].into_iter().collect(),
            minecraft_requirement: None,
            dependencies,
            conflicts,
            metadata_source: "test".to_owned(),
        }
    }

    #[test]
    fn parses_forge_toml_dependencies_and_version_ranges() {
        let raw = r#"
modLoader="javafml"
loaderVersion="[1,)"
[[mods]]
modId="example_forge"
version="1.0.0"
displayName="Example Forge"

[[dependencies.example_forge]]
modId="helper"
mandatory=true
versionRange="[1.2,2.0)"
ordering="NONE"
side="BOTH"
"#;

        let metadata = parse_toml(Path::new("example.jar"), raw, false).unwrap();

        assert!(metadata.loaders.contains(&LoaderKind::Forge));
        assert_eq!(metadata.dependencies.len(), 1);
        assert_eq!(metadata.dependencies[0].mod_id, "helper");
        assert!(metadata.dependencies[0].mandatory);
        assert_eq!(
            metadata.dependencies[0].version_requirement.as_deref(),
            Some("[1.2,2.0)")
        );
    }

    #[test]
    fn parses_neoforge_incompatible_dependency_as_conflict() {
        let raw = r#"
modLoader="javafml"
loaderVersion="[4,)"
[[mods]]
modId="example_neo"
version="1.0.0"

[[dependencies.example_neo]]
modId="bad_helper"
type="incompatible"
mandatory=false
versionRange="[1,)"
"#;

        let metadata = parse_toml(Path::new("example.jar"), raw, true).unwrap();

        assert!(metadata.loaders.contains(&LoaderKind::NeoForge));
        assert!(metadata.dependencies.is_empty());
        assert_eq!(metadata.conflicts, vec!["bad_helper"]);
    }

    #[test]
    fn rejects_toml_dependency_without_mod_id() {
        let raw = r#"
[[mods]]
modId="example"
version="1"

[[dependencies.example]]
mandatory=true
versionRange="[1,2)"
"#;

        let error = parse_toml(Path::new("example.jar"), raw, false).unwrap_err();
        assert!(error.contains("missing string modId"));
    }

    #[test]
    fn validates_fabric_semver_requirements() {
        assert!(fabric_version_matches("1.2.3", "^1.2").unwrap());
        assert!(fabric_version_matches("1.9.0", ">=1.2, <2.0").unwrap());
        assert!(fabric_version_matches("1.2.3", ">=1.0 || ^2.0").unwrap());
        assert!(!fabric_version_matches("2.0.0", "^1.2").unwrap());
        assert!(fabric_version_matches("1.2.3", "*").unwrap());
        assert!(fabric_version_matches("1.2.3", "latest").is_err());
    }

    #[test]
    fn validates_maven_exact_and_interval_requirements() {
        assert!(maven_version_matches("1.21.4", "[1.21.4]").unwrap());
        assert!(maven_version_matches("1.21.5", "[1.21.4,1.22)").unwrap());
        assert!(maven_version_matches("1.22.0", "[1.21.4,)").unwrap());
        assert!(!maven_version_matches("1.22.0", "[1.21.4,1.22)").unwrap());
        assert!(!maven_version_matches("1.21.3", "[1.21.4,)").unwrap());
        assert!(maven_version_matches("1.21.4", "[1.21.4,2.0)").unwrap());
        assert!(maven_version_matches("1.21.4", "[1.21.4,2.0").is_err());
    }

    #[test]
    fn rejects_dependency_provided_by_incompatible_loader() {
        let dependent = test_metadata(
            "dependent",
            "1.0.0",
            LoaderKind::Fabric,
            vec![ModDependency {
                mod_id: "helper".to_owned(),
                mandatory: true,
                version_requirement: None,
            }],
            vec![],
        );
        let helper = test_metadata("helper", "1.0.0", LoaderKind::Forge, vec![], vec![]);

        let report = ModScanReport {
            mods: vec![dependent, helper],
            issues: vec![],
        };

        let plan = plan_assignments(&report, LoaderKind::Fabric);
        let dependent_result = plan.iter().find(|item| item.mod_id == "dependent").unwrap();

        assert!(matches!(
            &dependent_result.status,
            AssignmentStatus::Rejected(reason)
                if reason.contains("target Loader")
        ));
    }

    #[test]
    fn rejects_dependency_version_mismatch() {
        let dependent = test_metadata(
            "dependent",
            "1.0.0",
            LoaderKind::Fabric,
            vec![ModDependency {
                mod_id: "helper".to_owned(),
                mandatory: true,
                version_requirement: Some("^2.0".to_owned()),
            }],
            vec![],
        );
        let helper = test_metadata("helper", "1.5.0", LoaderKind::Fabric, vec![], vec![]);

        let report = ModScanReport {
            mods: vec![dependent, helper],
            issues: vec![],
        };

        let plan = plan_assignments(&report, LoaderKind::Fabric);
        let dependent_result = plan.iter().find(|item| item.mod_id == "dependent").unwrap();

        assert!(matches!(
            &dependent_result.status,
            AssignmentStatus::Rejected(reason)
                if reason.contains("does not satisfy")
        ));
    }

    #[test]
    fn rejects_installed_mod_declared_as_conflict() {
        let first = test_metadata(
            "first",
            "1.0.0",
            LoaderKind::NeoForge,
            vec![],
            vec!["second".to_owned()],
        );
        let second = test_metadata("second", "1.0.0", LoaderKind::NeoForge, vec![], vec![]);

        let report = ModScanReport {
            mods: vec![first, second],
            issues: vec![],
        };

        let plan = plan_assignments(&report, LoaderKind::NeoForge);
        let first_result = plan.iter().find(|item| item.mod_id == "first").unwrap();

        assert!(matches!(
            &first_result.status,
            AssignmentStatus::Rejected(reason)
                if reason.contains("declared conflict")
        ));
    }

    #[test]
    fn scans_all_three_loader_metadata_paths_from_real_zip_archives() {
        let fixtures = [
            (
                "fabric-e2e.jar",
                "fabric.mod.json",
                r#"{"schemaVersion":1,"id":"fabric_e2e","version":"1.2.0"}"#,
                "fabric_e2e",
                LoaderKind::Fabric,
            ),
            (
                "forge-e2e.jar",
                "META-INF/mods.toml",
                "[[mods]]\nmodId=\"forge_e2e\"\nversion=\"1.2.0\"\n",
                "forge_e2e",
                LoaderKind::Forge,
            ),
            (
                "neoforge-e2e.jar",
                "META-INF/neoforge.mods.toml",
                "[[mods]]\nmodId=\"neo_e2e\"\nversion=\"1.2.0\"\n",
                "neo_e2e",
                LoaderKind::NeoForge,
            ),
        ];

        for (label, entry, raw, expected_id, expected_loader) in fixtures {
            let path = test_path(label);
            std::fs::write(&path, make_jar(&[(entry, raw)])).unwrap();
            let metadata = scan_mod_jar(&path).unwrap();
            assert_eq!(metadata.id, expected_id, "fixture {label}");
            assert!(
                metadata.loaders.contains(&expected_loader),
                "fixture {label}"
            );
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn rejects_missing_metadata_and_corrupt_zip_archives() {
        let no_metadata = test_path("no-metadata.jar");
        std::fs::write(&no_metadata, make_jar(&[("README.txt", "not a mod")])).unwrap();
        assert!(
            scan_mod_jar(&no_metadata)
                .unwrap_err()
                .contains("no supported Mod metadata")
        );

        let corrupt = test_path("corrupt.jar");
        std::fs::write(&corrupt, b"this is not a ZIP archive").unwrap();
        assert!(
            scan_mod_jar(&corrupt)
                .unwrap_err()
                .contains("invalid JAR/ZIP")
        );

        let _ = std::fs::remove_file(no_metadata);
        let _ = std::fs::remove_file(corrupt);
    }

    #[test]
    fn rejects_oversized_metadata_before_parsing() {
        let path = test_path("oversized-metadata.jar");
        let oversized = " ".repeat(MAX_METADATA_BYTES as usize + 1);
        std::fs::write(&path, make_jar(&[("fabric.mod.json", &oversized)])).unwrap();

        let error = scan_mod_jar(&path).unwrap_err();
        assert!(
            error.contains("exceeds 4 MiB limit"),
            "unexpected error: {error}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_malformed_metadata_inside_valid_zip() {
        let path = test_path("malformed-metadata.jar");
        std::fs::write(
            &path,
            make_jar(&[("fabric.mod.json", "{ definitely not valid JSON")]),
        )
        .unwrap();

        assert!(
            scan_mod_jar(&path)
                .unwrap_err()
                .contains("invalid fabric.mod.json")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn directory_scan_isolates_bad_jars_and_keeps_valid_mods() {
        let directory = test_path("mixed-mod-directory");
        std::fs::create_dir_all(&directory).unwrap();

        std::fs::write(
            directory.join("valid.jar"),
            make_jar(&[("fabric.mod.json", r#"{"id":"valid_mod","version":"1.0.0"}"#)]),
        )
        .unwrap();
        std::fs::write(directory.join("broken.jar"), b"broken archive").unwrap();
        std::fs::write(directory.join("ignore.txt"), b"not a jar").unwrap();

        let report = scan_mod_directories(std::slice::from_ref(&directory));
        assert_eq!(report.mods.len(), 1);
        assert_eq!(report.mods[0].id, "valid_mod");
        assert_eq!(report.issues.len(), 1);
        assert!(report.issues[0].jar_path.ends_with("broken.jar"));

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn optional_missing_dependency_does_not_block_assignment() {
        let metadata = test_metadata(
            "optional_user",
            "1.0.0",
            LoaderKind::Fabric,
            vec![ModDependency {
                mod_id: "optional_helper".to_owned(),
                mandatory: false,
                version_requirement: None,
            }],
            vec![],
        );
        let report = ModScanReport {
            mods: vec![metadata],
            issues: vec![],
        };
        let plan = plan_assignments(&report, LoaderKind::Fabric);

        assert!(matches!(
            plan[0].status,
            AssignmentStatus::Assigned(LoaderKind::Fabric)
        ));
    }

    #[test]
    fn accepts_complete_multilevel_dependency_chain() {
        let leaf = test_metadata("leaf", "1.0.0", LoaderKind::NeoForge, vec![], vec![]);
        let middle = test_metadata(
            "middle",
            "1.0.0",
            LoaderKind::NeoForge,
            vec![ModDependency {
                mod_id: "leaf".to_owned(),
                mandatory: true,
                version_requirement: Some("[1.0,2.0)".to_owned()),
            }],
            vec![],
        );
        let root = test_metadata(
            "root",
            "1.0.0",
            LoaderKind::NeoForge,
            vec![ModDependency {
                mod_id: "middle".to_owned(),
                mandatory: true,
                version_requirement: Some("[1.0,2.0)".to_owned()),
            }],
            vec![],
        );
        let report = ModScanReport {
            mods: vec![root, middle, leaf],
            issues: vec![],
        };
        let plan = plan_assignments(&report, LoaderKind::NeoForge);

        assert_eq!(plan.len(), 3);
        assert!(plan.iter().all(|item| {
            matches!(
                item.status,
                AssignmentStatus::Assigned(LoaderKind::NeoForge)
            )
        }));
    }

    #[test]
    fn rejects_missing_required_dependency() {
        let metadata = ModMetadata {
            id: "dependent".into(),
            name: "Dependent".into(),
            version: "1".into(),
            jar_path: PathBuf::from("dependent.jar"),
            loaders: [LoaderKind::Fabric].into_iter().collect(),
            minecraft_requirement: None,
            dependencies: vec![ModDependency {
                mod_id: "missing".into(),
                mandatory: true,
                version_requirement: None,
            }],
            conflicts: vec![],
            metadata_source: "test".into(),
        };
        let report = ModScanReport {
            mods: vec![metadata],
            issues: vec![],
        };
        let plan = plan_assignments(&report, LoaderKind::Fabric);
        assert!(matches!(plan[0].status, AssignmentStatus::Rejected(_)));
    }
}

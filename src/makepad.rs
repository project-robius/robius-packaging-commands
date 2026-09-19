use std::{collections::HashMap, fs, path::{Path, PathBuf}, sync::OnceLock};

use cargo_metadata::MetadataCommand;

pub(crate) static FORCE_MAKEPAD: OnceLock<bool> = OnceLock::new();
pub(crate) static IS_MAKEPAD_APP: OnceLock<bool> = OnceLock::new();


/// Returns the value of the `MAKEPAD_PACKAGE_DIR` environment variable
/// that must be set for the given package format.
///
/// * For macOS app bundles, this should be set to the current directory `.`
///   * This only works because we enable the Makepad `apple_bundle` cfg option,
///     which tells Makepad to invoke Apple's `NSBundle` API to retrieve the resource path at runtime.
///     This resource path points to the bundle's `Contents/Resources/` directory.
/// * For AppImage packages, this should be set to the /usr/lib/<binary> directory. 
///   Since AppImages execute with a simulated working directory of `usr/`,
///   we just need a relative path that goes there, i.e.,  "lib/robrix`.
///   * Note that this must be a relative path, not an absolute path.
/// * For Debian `.deb` packages, this should be set to `/usr/lib/<main-binary-name>`.
///   * This is the directory in which `dpkg` copies app resource files to
///     when a user installs the `.deb` package.
/// * For Windows NSIS packages, this should be set to `.` (the current dir).
///  * This is because the NSIS installer script copies the resources to the same directory
///    as the installed binaries.
pub(crate) fn makepad_package_dir_value(package_format: &str, main_binary_name: &str) -> String {
    match package_format {
        "app" | "dmg" => format!("."),
        "appimage" => format!("lib/{}", main_binary_name),
        "deb" | "pacman" => format!("/usr/lib/{}", main_binary_name),
        "nsis" => format!("."),
        _other => panic!("Unsupported package format: {}", _other),
    }
}


/// Returns whether the package being built is a makepad app, i.e., it depends on `makepad-widgets`.
pub(crate) fn is_makepad_app() -> bool {
    *IS_MAKEPAD_APP.get_or_init(|| {
        MetadataCommand::new()
            .exec()
            .ok()
            .map(|cargo_metadata| cargo_metadata
                .packages
                .iter()
                .any(|package| package.name == "makepad-widgets")
            )
            .unwrap_or(false)
    })
}

/// Every makepad crate that ships a `resources` directory, keyed by the
/// `makepad_<crate>` name its resources get packaged under.
///
/// This used to read the `<crate>.path` files that makepad's build scripts leave in the target
/// dir, but those are a build-script side effect: anything that prunes the target dir deletes
/// them, and a warm build cache means the build script never re-runs to write them again.
pub(crate) fn get_makepad_resources_paths() -> HashMap<String, PathBuf> {
    let Ok(metadata) = MetadataCommand::new().exec() else {
        return HashMap::new();
    };
    metadata
        .packages
        .iter()
        .filter(|package| package.name.starts_with("makepad-"))
        .filter_map(|package| {
            let crate_dir = package.manifest_path.parent()?.to_owned().into_std_path_buf();
            if !crate_dir.join("resources").is_dir() {
                return None;
            }
            Some((package.name.replace('-', "_"), crate_dir))
        })
        .collect()
}

/// Recursively copies the Makepad-specific resource files.
///
/// This uses `cargo-metadata` to determine the location of the `makepad-widgets` crate,
/// and then copies the `resources` directory from that crate to a makepad-specific subdirectory
/// of the given `dist_resources_dir` path, which is currently `./dist/resources/makepad_widgets/`.
/// The font assets an app's binary declares, read from the `makepad.font-assets.v1`
/// section that `app_main!` embeds. `cargo-makepad` packages mobile builds from the same
/// manifest, so desktop ships exactly the fonts the app can reach too.
pub(crate) struct FontManifest {
    assets: Vec<String>,
}

impl FontManifest {
    const HEADER: &'static [u8] = b"format=makepad.font-assets.v1\n";

    /// Scans the raw bytes rather than parsing ELF/Mach-O/PE, so one reader covers every
    /// desktop target. The manifest is line-oriented, and a base manifest can also sit in
    /// the binary as plain data, so the longest occurrence is the app's own.
    ///
    /// `Ok(None)` means the binary has no manifest at all, which is what a makepad from
    /// before `app_main!` started embedding one produces. Those builds get every font.
    pub(crate) fn from_binary(path: &Path) -> std::io::Result<Option<Self>> {
        let bytes = fs::read(path)?;
        let mut best: Option<Vec<String>> = None;
        let mut search = 0;
        while let Some(found) = find(&bytes[search..], Self::HEADER) {
            let start = search + found;
            let mut assets = Vec::new();
            let mut cursor = start + Self::HEADER.len();
            while let Some(newline) = find(&bytes[cursor..], b"\n") {
                let line = &bytes[cursor..cursor + newline];
                cursor += newline + 1;
                if let Some(asset) = line.strip_prefix(b"asset=") {
                    assets.push(String::from_utf8_lossy(asset).into_owned());
                } else if !line.starts_with(b"set=") {
                    break;
                }
            }
            if best.as_ref().map_or(true, |b| assets.len() > b.len()) {
                best = Some(assets);
            }
            search = start + Self::HEADER.len();
        }
        Ok(best.map(|assets| Self { assets }))
    }

    /// `logical_path` is the manifest's form, e.g. `makepad_widgets/resources/Foo.ttf`.
    fn declares(&self, logical_path: &str) -> bool {
        self.assets.iter().any(|asset| asset == logical_path)
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
}

pub(crate) fn copy_makepad_resources<P>(dist_resources_dir: P, path_to_binary: &Path) -> std::io::Result<()>
where
    P: AsRef<Path>
{
    let manifest = FontManifest::from_binary(path_to_binary)?;
    if manifest.is_none() {
        println!("No font manifest in {}; this makepad predates them, so every font is packaged.",
            path_to_binary.display());
    }
    let makepad_resources_paths = get_makepad_resources_paths();
    if makepad_resources_paths.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Missing resource paths: cargo metadata reported no makepad crate with a `resources` directory".to_string(),
        ));
    }
    println!("Copying Makepad resources...");
    for (resources_dir_name, resources_dir_path) in &makepad_resources_paths {
        let source_path = resources_dir_path.join("resources");

        let makepad_widgets_resources_dest = dist_resources_dir.as_ref()
            .join(resources_dir_name)
            .join("resources");

        if source_path.exists() {
            println!("--> From: {}\n      to:   {}", source_path.display(), makepad_widgets_resources_dest.display());
            let mut packaged = Vec::new();
            let mut skipped = Vec::new();
            copy_resources_filtered(&source_path, &makepad_widgets_resources_dest, &mut |relative| {
                if !is_font_file(relative) {
                    return true;
                }
                let logical_path = format!("{}/resources/{}", resources_dir_name, relative.display());
                let wanted = manifest.as_ref().is_none_or(|m| m.declares(&logical_path));
                if wanted { packaged.push(logical_path) } else { skipped.push(logical_path) }
                wanted
            })?;
            println!("    Packaged fonts: {}", packaged.join(", "));
            if !skipped.is_empty() {
                println!("    Skipped fonts not in the app's manifest: {}", skipped.join(", "));
            }
        }
    }
    Ok(())
}

/// Like `copy_recursively`, but `keep` sees each file's path relative to `source` and can
/// drop it. Directories are always created so non-font resources land unchanged.
fn copy_resources_filtered(
    source: &Path,
    destination: &Path,
    keep: &mut dyn FnMut(&Path) -> bool,
) -> std::io::Result<()> {
    fn walk(root: &Path, dir: &Path, destination: &Path, keep: &mut dyn FnMut(&Path) -> bool) -> std::io::Result<()> {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let dest = destination.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                walk(root, &entry.path(), &dest, keep)?;
            } else if keep(entry.path().strip_prefix(root).unwrap_or(&entry.path())) {
                fs::copy(entry.path(), dest)?;
            }
        }
        Ok(())
    }
    walk(source, source, destination, keep)
}

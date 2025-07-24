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

pub(crate) fn get_makepad_resources_paths() -> HashMap<String, PathBuf> {
    let paths_dir = PathBuf::from("target/release/");
    fs::read_dir(&paths_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();

            if !path.is_file() {
                return None;
            }

            let filename = path.file_name()?.to_str()?;

            let package_name = filename
                .strip_prefix("makepad-")?
                .strip_suffix(".path")?;

            let content = fs::read_to_string(&path).ok()?;
            let resources_dir_path = PathBuf::from(content.trim());
            let resources_dir_name = format!("makepad_{}", package_name.replace('-', "_"));
            Some((resources_dir_name, resources_dir_path))
        })
        .collect()
}

/// Recursively copies the Makepad-specific resource files.
///
/// This uses `cargo-metadata` to determine the location of the `makepad-widgets` crate,
/// and then copies the `resources` directory from that crate to a makepad-specific subdirectory
/// of the given `dist_resources_dir` path, which is currently `./dist/resources/makepad_widgets/`.
pub(crate) fn copy_makepad_resources<P>(dist_resources_dir: P) -> std::io::Result<()>
where
    P: AsRef<Path>
{
    let makepad_resources_paths = get_makepad_resources_paths();
    if makepad_resources_paths.is_empty() {
        // This situation can happen if the user use local Makepad repository and deletes the all `makepad-*.path` files.
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Missing resource paths: no `.path` files found in the Makepad build directory (./target/release/)",
        ));
    }
    println!("Copying Makepad resources...");
    for (resources_dir_name, resources_dir_path) in makepad_resources_paths {
        let source_path = resources_dir_path.join("resources");

        let makepad_widgets_resources_dest = dist_resources_dir.as_ref()
            .join(resources_dir_name)
            .join("resources");

        if source_path.exists() {
            println!("--> From: {}\n      to:   {}", source_path.display(), makepad_widgets_resources_dest.display());
            super::copy_recursively(&source_path, &makepad_widgets_resources_dest)?;
        }
    }
    Ok(())
}

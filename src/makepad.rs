use std::{fs::read_to_string, io::{Error, ErrorKind}, path::Path, process::Command, sync::OnceLock};

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
        // First ensure the project is compiled to generate .path files
        if let Ok(output) = Command::new("cargo")
            .args(&["check", "--quiet"])
            .output()
        {
            if output.status.success() {
                // Check if makepad-widgets.path exists in target/debug/
                Path::new("target/debug/makepad-widgets.path").exists()
            } else {
                false
            }
        } else {
            false
        }
    })
}


/// Recursively copies the Makepad-specific resource files.
///
/// This reads the makepad-widgets path from `target/debug/makepad-widgets.path` file,
/// and then copies the `resources` directory from that crate to a makepad-specific subdirectory
/// of the given `dist_resources_dir` path, which is currently `./dist/resources/makepad_widgets/`.
pub(crate) fn copy_makepad_resources<P>(dist_resources_dir: P) -> std::io::Result<()>
where
    P: AsRef<Path>
{
    let makepad_widgets_resources_dest = dist_resources_dir.as_ref()
        .join("makepad_widgets")
        .join("resources");
    let makepad_widgets_resources_src = {
        let path_file = Path::new("target/debug/makepad-widgets.path");
        if !path_file.exists() {
            // Try to run cargo check to generate the .path file
            if let Ok(output) = Command::new("cargo")
                .args(&["check", "--quiet"])
                .output()
            {
                if !output.status.success() || !path_file.exists() {
                    let _ = IS_MAKEPAD_APP.set(false);
                    return Err(Error::new(
                        ErrorKind::NotFound,
                        "makepad-widgets.path file not found even after running cargo check. This project is not makepad app"
                    ));
                }
            } else {
                return Err(Error::new(
                    ErrorKind::Other,
                    "Failed to run cargo check to generate makepad-widgets.path file."
                ));
            }
        }

        println!("Makepad App: {}", true);
        let _ = IS_MAKEPAD_APP.set(true);

        let makepad_widgets_path = read_to_string(path_file)?
            .trim()
            .to_string();

        Path::new(&makepad_widgets_path)
            .join("resources")
    };

    println!("Copying makepad-widgets resources...\n  --> From: {}\n      to:   {}",
        makepad_widgets_resources_src.display(),
        makepad_widgets_resources_dest.display(),
    );
    super::copy_recursively(&makepad_widgets_resources_src, &makepad_widgets_resources_dest)?;
    println!("  --> Done!");
    Ok(())
}

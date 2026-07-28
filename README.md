# robius-packaging-commands

[![Latest Version](https://img.shields.io/crates/v/robius-packaging-commands.svg)](https://crates.io/crates/robius_packaging_commands)
[![Project Robius Matrix Chat](https://img.shields.io/matrix/robius-general%3Amatrix.org?server_fqdn=matrix.org&style=flat&logo=matrix&label=Project%20Robius%20Matrix%20Chat&color=B7410E)](https://matrix.to/#/#robius:matrix.org)

A multi-platform companion tool to help package your Rust app when using `cargo-packager`.

## Quick example of usage
### Workspace example (app crate is not workspace root)
In a workspace, you can run `cargo packager` from the app crate directory. The tool will use the
current directory for `./resources` and `./dist`, and use `--path-to-binary` to locate the target dir.

```toml
[package.metadata.packager]
product_name = "Robrix"
out_dir = "./dist"

before-each-package-command = """
robius-packaging-commands before-each-package \
    --binary-name robrix \
    --path-to-binary ../../target/release/robrix
"""

resources = [
    { src = "./dist/resources/makepad_widgets", target = "makepad_widgets" },
    { src = "./dist/resources/makepad_fonts_chinese_bold", target = "makepad_fonts_chinese_bold" },
    { src = "./dist/resources/makepad_fonts_chinese_bold_2", target = "makepad_fonts_chinese_bold_2" },
    { src = "./dist/resources/makepad_fonts_chinese_regular", target = "makepad_fonts_chinese_regular" },
    { src = "./dist/resources/makepad_fonts_chinese_regular_2", target = "makepad_fonts_chinese_regular_2" },
    { src = "./dist/resources/makepad_fonts_emoji", target = "makepad_fonts_emoji" },
    { src = "./dist/resources/robrix", target = "robrix" },
]
```

This program should be invoked by `cargo-packager`'s "before-package" and "before-each-package" hooks,
which you must specify in your `Cargo.toml` file under the `[package.metadata.packager]` section.

It uses the current working directory as the app root for `./resources` and `./dist`,
while `--path-to-binary` is used to locate the target directory (useful in workspaces).

> [!IMPORTANT]
> You *must* build in release mode (using `cargo packager --release`).

> [!IMPORTANT]
> To build a Linux `.deb` package, you need to install `dpkg-dev`:
> ```sh
> sudo apt-get install dpkg-dev
> ```

See the example below for an app called "Robrix" with a binary named "robrix".

```toml
## Configuration for `cargo packager`
[package.metadata.packager]
product_name = "Robrix"

[package.metadata.packager.macos]
## You can use `-` as the value for `signing_identity`,
## if you just want to test the packaging on macOS without signing the app.
signature_identity = "-"
...

## Note: for Makepad apps, you only need to specify `before-packaging-command`
##       if you're using Makepad versions **BEFORE** v1.0.
##       If using Makepad v1.0 or higher, you can omit this.
##
## This runs just one time before packaging starts.
before-packaging-command = """
robius-packaging-commands before-packaging \
    --binary-name robrix \
    --path-to-binary ./target/release/robrix
"""

...

## This runs once before building each separate kind of package,
## so it is used to build your app specifically for each package kind.
before-each-package-command = """
robius-packaging-commands before-each-package \
    --binary-name robrix \
    --path-to-binary ./target/release/robrix
"""

## Note: if you're using Makepad versions **BEFORE** v1.0, you only need these resources:
resources = [
    { src = "./dist/resources/makepad_widgets", target = "makepad_widgets" },
    { src = "./dist/resources/robrix", target = "robrix" },
]

## Note: if you're using Makepad v1.0 or higher, you need to specify more resource files:
resources = [
    { src = "./dist/resources/makepad_widgets", target = "makepad_widgets" },
    { src = "./dist/resources/makepad_fonts_chinese_bold", target = "makepad_fonts_chinese_bold" },
    { src = "./dist/resources/makepad_fonts_chinese_bold_2", target = "makepad_fonts_chinese_bold_2" },
    { src = "./dist/resources/makepad_fonts_chinese_regular", target = "makepad_fonts_chinese_regular" },
    { src = "./dist/resources/makepad_fonts_chinese_regular_2", target = "makepad_fonts_chinese_regular_2" },
    { src = "./dist/resources/makepad_fonts_emoji", target = "makepad_fonts_emoji" },
    { src = "./dist/resources/robrix", target = "robrix" },
]
```

Once you have this package metadata fully completed in your app crate's `Cargo.toml`,
you are ready to run.

1. Install `cargo-packager`:
```sh
rustup update stable  ## Rust version 1.79 or higher is required
cargo +stable install --force --locked cargo-packager
```

2. Install this appropriate version of this crate, either from `crates.io` or from this git repo.
> [!IMPORTANT]
> For Makepad apps using Makepad versions *before* v1.0, install `robius-packaging-commands` `--version 0.1`.
>
> For Makepad apps using Makepad versions *after* v1.0, install the newest version of `robius-packaging-commands`.

```sh
# From crates.io
cargo install robius-packaging-commands --version <VERSION> --locked
```
```sh
# From this git repo
cargo install --version <VERSION> --locked [--git https://github.com/project-robius/robius-packaging-commands.git]
```

3. Then run the packaging routine:
```sh
cargo packager --release ## --verbose is optional
```

## More info

This program no longer requires the workspace root as the working directory.
It uses the current working directory for app resources (`./resources`) and build output (`./dist`),
and uses `--path-to-binary` to locate the target directory (e.g., a workspace `target/release`).

This program runs in two modes, one for each kind of before-packaging step in cargo-packager:
1. `before-packaging`: specifies that the `before-packaging-command` is being run by cargo-packager, which gets executed only *once* before cargo-packager generates any package bundles.

> [!IMPORTANT]
> The `before-packaging` command is not needed if building an app using Makepad v1.0 or higher.

2. `before-each-package`: specifies that the `before-each-package-command` is being run by cargo-packager, which gets executed multiple times: once for *each* package that cargo-packager is going to generate.
  * The environment variable `CARGO_PACKAGER_FORMAT` is set by cargo-packager to the declare which package format is about to be generated, which include the values given here: <https://docs.rs/cargo-packager/latest/cargo_packager/enum.PackageFormat.html>.
    * `app`, `dmg`: for macOS.
    * `deb`, `appimage`, `pacman`: for Linux.
    * `nsis`: for Windows; `nsis` generates an installer `setup.exe`.
    * `wix`: (UNSUPPORTED) for Windows; generates an `.msi` installer package.

> [!TIP]
> For `.deb` packages, runtime dependencies are computed automatically:
> * `dpkg-shlibdeps` handles every dynamically-linked lib, version-pinned.
> * The binary is scanned for what the linker can't see: dlopen'd libs (e.g. Makepad's
>   `libEGL.so.1`), TLS (`ca-certificates`), and `xdg-open` (`xdg-utils`). A soname only
>   counts if an instruction actually references it, so a lib merely named in an error
>   message doesn't become a dep.
> * `desktop-file-utils` and `hicolor-icon-theme` are always added; their dpkg triggers
>   are what register the `.desktop` file (deep links included) and the app icons.
>
> Makepad apps also skip the libs Makepad dlopens but runs fine without (its GStreamer
> video stack). Other apps don't inherit that, since skipping a lib an app really needs
> would ship a broken `.deb`.
>
> To fix anything missed: `--add-deb-dep <package>` adds a dep, and
> `--optional-deb-dep-prefix <stem>` marks a dlopen'd lib as optional so it's skipped.
> Both are repeatable, and ignored for non-`.deb` formats.

> [!TIP]
> `verify-deb` checks that a built `.deb` really declares what it uses:
> ```sh
> robius-packaging-commands verify-deb --deb ./dist/myapp_1.0.0_amd64.deb
> ```
> It installs the package into a minimal container with only its `Depends` (which also
> proves it's installable), boots the app under `strace`, and fails if anything the app
> loads or spawns isn't covered. On failure it prints the missing packages and the exact
> command to add them. Worth running in CI: scanning a binary can't prove a dlopen'd lib
> or spawned program was found.
>
> `--host` skips the container (quicker, weaker), `--image <distro:version>` picks the
> test distro, `--run-secs <n>` sets how long the app runs (default 10), and
> `--binary-name <name>` picks the binary if the `.deb` ships several.
>
> Note it only sees code paths a short boot exercises, so a pass means the
> startup-critical deps are there, not that the list is provably complete.

This program uses the `CARGO_PACKAGER_FORMAT` environment variable to determine
which specific build commands and configuration options should be used.

## License

MIT

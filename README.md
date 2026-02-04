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
> For Makepad apps using Makepad versions *after* v1.0, install `robius-packaging-commands` `--version ^0.2`.

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

This program uses the `CARGO_PACKAGER_FORMAT` environment variable to determine
which specific build commands and configuration options should be used.

## License

MIT

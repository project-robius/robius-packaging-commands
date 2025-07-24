# robius-packaging-commands
A multi-platform companion tool to help package your Rust app when using `cargo-packager`.

## Quick example of usage
This program should be invoked by `cargo-packager`'s "before-package" and "before-each-package" hooks,
which you must specify in your `Cargo.toml` file under the `[package.metadata.packager]` section.
See the example below:

```toml
## Configuration for `cargo packager`
[package.metadata.packager]
product_name = "Robrix"

[package.metadata.packager.macos]
## You can use `-` as the value for `signing_identity`,
## if you just want to test the packaging on macOS without signing the app.
signature_identity = "-"
...

## This runs just one time before packaging starts; thus, it is used
## mostly just to handle resources and target-agnostic stuff.
##
## Note: if your project is a Makepad app, you don't need to specify this, because
## before-each-package-command will automatically copy the app's resources
## to the target directory, so you can just use that command for other you want to do
## before packaging.
# before-packaging-command = """
# robius-packaging-commands before-packaging \
#     --binary-name robrix \
#     --path-to-binary ./target/release/robrix
# """

...

## This runs once before building each separate kind of package,
## so it is used to build your app specifically for each package kind.
before-each-package-command = """
robius-packaging-commands before-each-package \
    --binary-name robrix \
    --path-to-binary ./target/release/robrix
"""

## Note: for now, if you are using Makepad apps, you need to add some additional configuration to your `Cargo.toml` file.
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

2. Install this crate:
```sh
cargo install --locked --git https://github.com/project-robius/robius-packaging-commands.git
```

3. Then run the packaging routine:
```sh
cargo packager --release ## --verbose is optional
```

## More info

This program must be run from the root of the project directory,
which is also where the `cargo-packager` command must be invoked from,
so that shouldn't present any problems.

This program runs in two modes, one for each kind of before-packaging step in cargo-packager:
1. `before-packaging`: specifies that the `before-packaging-command` is being run by cargo-packager, which gets executed only *once* before cargo-packager generates any package bundles.
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

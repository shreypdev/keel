# undra

The command-line tool of [Undra](https://shreypdev.github.io/undra/), the Rust core for native
apps: one domain core written in Rust, and every platform gets the SwiftUI, Jetpack Compose or
React code its own engineers would have written.

```sh
npm install -g @undra/cli
undra --version
undra init myapp
```

This package contains no code of its own beyond a small launcher. The `undra` executable
comes from the platform package that npm installs next to it (`@undra/cli-darwin-arm64`,
`@undra/cli-darwin-x64`, `@undra/cli-linux-x64` or `@undra/cli-linux-arm64`, chosen by the
`os` and `cpu` fields), so a normal install downloads one binary. Installing with
`--omit=optional` (or `--no-optional`) leaves the binary out and the launcher says so.

Other ways to install the same binary:

```sh
brew install shreypdev/undra/undra
curl -fsSL https://shreypdev.github.io/undra/install.sh | sh
cargo install --git https://github.com/shreypdev/undra undra-cli
```

Linux builds need glibc; Alpine (musl) and Windows are not supported yet.

* Documentation: <https://shreypdev.github.io/undra/docs/>
* Source and issues: <https://github.com/shreypdev/undra>
* Licence: MIT OR Apache-2.0

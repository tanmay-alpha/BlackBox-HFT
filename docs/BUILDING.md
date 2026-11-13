# Building BlackBox HFT

## Requirements

- Rust 1.91.1 or newer (the pinned engine's minimum).
- A C compiler. The engine's `backtest` feature depends on the `zip` crate with default features,
  which builds `zstd` from C source.
- Node.js 20 or newer and npm, for the web UI.

On Linux and macOS the system compiler is enough. On Windows with the Visual Studio Build Tools
("Desktop development with C++") installed, the default `x86_64-pc-windows-msvc` toolchain works
with no further setup.

```
cargo build --release
cargo test --workspace
cd web && npm install && npm run build
```

## Windows without the Visual Studio Build Tools

Without the MSVC linker and Windows SDK the default toolchain cannot link anything, including
build scripts. Use the GNU toolchain with a MinGW-w64 GCC:

1. `rustup toolchain install stable-x86_64-pc-windows-gnu`
2. Unpack a MinGW-w64 GCC distribution (for example WinLibs, MSVCRT runtime, POSIX threads, SEH)
   to a directory **whose path contains no spaces**. GCC's linker driver splits its own install
   path on spaces and fails with `cannot find ...: No such file or directory`.
3. In the repository root create these two files (both are git-ignored, because they describe one
   machine):

   `rust-toolchain.toml`
   ```toml
   [toolchain]
   channel = "stable-x86_64-pc-windows-gnu"
   ```

   `.cargo/config.toml` (adjust the path)
   ```toml
   [target.x86_64-pc-windows-gnu]
   linker = "C:/path/to/mingw64/bin/gcc.exe"
   rustflags = ["-C", "dlltool=C:/path/to/mingw64/bin/dlltool.exe"]

   [env]
   CC_x86_64_pc_windows_gnu = "C:/path/to/mingw64/bin/gcc.exe"
   CXX_x86_64_pc_windows_gnu = "C:/path/to/mingw64/bin/g++.exe"
   AR_x86_64_pc_windows_gnu = "C:/path/to/mingw64/bin/ar.exe"
   ```

   `dlltool` is needed because some dependencies declare Windows imports that the GNU target
   resolves by generating import libraries at build time.

After that, plain `cargo build` and `cargo test` work from the repository root.

## Shell scripts on Windows

Run `scripts/*.sh` from Git Bash. On a machine where WSL is enabled but has no distribution
installed, the `bash` found first on `PATH` may be the WSL launcher, which fails with
`execvpe(/bin/bash) failed`.

## Reproducibility

- `Cargo.lock` is committed; the engine is pinned with `hftbacktest = "=0.9.4"`.
- A run's `result_sha256` is reproducible for the same binary on the same platform. Across
  platforms it is expected to match for strategies that use only exact arithmetic; strategies that
  call `ln`/`exp` (Avellaneda-Stoikov) can differ in the last bit of an intermediate value between
  math libraries, which changes a quote only if it lands exactly on a rounding boundary. Run
  manifests record the platform for that reason.

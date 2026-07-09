{
  description = "Xiaomi Powerbank Manager development environment";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        inherit (pkgs) lib;
      in
      {
        devShells.default = pkgs.mkShell {
          packages =
            with pkgs;
            [
              binaryen
              cargo
              clang
              clippy
              hidapi
              lld
              nodejs_24
              pkg-config
              rust-analyzer
              rustc
              rustfmt
              trunk
              wasm-bindgen-cli
            ]
            ++ lib.optionals stdenv.isLinux [
              systemd.dev
            ]
            ++ lib.optionals stdenv.isDarwin [
              libiconv
            ];

          shellHook =
            ''
              export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER="${pkgs.lld}/bin/wasm-ld"
              export RUST_BACKTRACE=1
              export CC="${pkgs.clang}/bin/clang"
            ''
            + lib.optionalString pkgs.stdenv.isDarwin ''
              export SDKROOT="$(xcrun --show-sdk-path 2>/dev/null || /usr/bin/xcrun --show-sdk-path 2>/dev/null || true)"
              if [ -n "$SDKROOT" ]; then
                export CFLAGS="-isysroot $SDKROOT ''${CFLAGS:-}"
              fi
              export LIBRARY_PATH="${pkgs.libiconv}/lib:''${LIBRARY_PATH:-}"
              export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="${pkgs.clang}/bin/clang"
              export CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER="${pkgs.clang}/bin/clang"
            '';
        };
      }
    );
}

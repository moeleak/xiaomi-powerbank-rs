# Xiaomi Powerbank RS

Rust implementation of the Xiaomi power bank USB HID protocol from
[`yomiel-s/xiaomi-powerbank-tools`](https://github.com/yomiel-s/xiaomi-powerbank-tools).

The project provides:

- `xiaomi-pb`: a CLI with colored output, shell completions, and an interactive REPL.
- `xiaomi-pb-gui`: a Material 3 desktop app built with `material-ui-rs`.
- A WebAssembly GUI that uses browser WebHID and deploys to Cloudflare Workers.

## Development

```sh
nix develop
cargo test --workspace
cargo run -p xiaomi-pb-cli -- info
cargo run -p xiaomi-pb-gui
trunk serve crates/xiaomi-pb-gui/web/index.html
```

The device must be placed in data transfer mode first: press the power bank
button 8 times, then connect it over USB.

## Linux udev

```sh
sudo tee /etc/udev/rules.d/99-xiaomi-powerbank.rules <<'EOF'
SUBSYSTEM=="usb", ATTR{idVendor}=="2717", MODE="0666"
SUBSYSTEM=="usb", ATTR{idVendor}=="1a86", MODE="0666"
EOF
sudo udevadm control --reload-rules
sudo udevadm trigger
```

## Cloudflare deployment

GitHub Actions deploys the WASM GUI to `xiaomi-powerbank.leak.moe`.

Required repository secrets:

- `CLOUDFLARE_ACCOUNT_ID`
- `CLOUDFLARE_API_TOKEN`

Set them with:

```sh
gh secret set CLOUDFLARE_ACCOUNT_ID
gh secret set CLOUDFLARE_API_TOKEN
```

The token needs permission to deploy Workers and configure the route for
`xiaomi-powerbank.leak.moe/*`.

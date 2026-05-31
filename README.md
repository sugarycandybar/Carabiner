# Carabiner

Carabiner is a desktop application designed to simplify the creation and management of secure network tunnels on Linux. 

It provides a clean, native-style interface for multiple tunnel providers, removing the need for complex CLI configurations.

## Why Carabiner?

- Easy to use: view and configure your network tunnels without complex CLI tools.
- Integrated support: built-in support for Ngrok and Playit.
- Live monitoring: watch tunnel status and logs in real time.
- All in the app: setup wizards, management, and monitoring in one place.

## Run Carabiner

[![Download on Flathub](https://flathub.org/assets/badges/flathub-badge-en.png)](https://flathub.org/en/apps/io.github.sugarycandybar.Carabiner)

<details>
<summary>Run from source (Rust)</summary>

### Linux

1. Install GTK4/libadwaita system packages:

```bash
# Fedora
sudo dnf install gtk4-devel libadwaita-devel

# Ubuntu/Debian
sudo apt install libgtk-4-dev libadwaita-1-dev
```

2. Run Carabiner:

```bash
cargo run
```

### Building with Meson

```bash
meson setup build
meson compile -C build
meson install -C build
```

### Building the Flatpak

```bash
flatpak-builder --user --install --force-clean build-dir io.github.sugarycandybar.Carabiner.json
flatpak run io.github.sugarycandybar.Carabiner
```

</details>

## License

GPL-3.0-or-later

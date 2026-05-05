# Carabiner (Rust)

Native Rust port of the Python Carabiner GTK4/libadwaita app.

The Rust version keeps the same application ID, data directory rules, JSON file names, provider setup flow, tunnel behavior, portal background request behavior, and command-line `--background` mode as the Python version.

## Build

Install GTK4 and libadwaita development packages, then run:

```bash
cargo run
```

This host does not have `libadwaita-1.pc` installed globally, so verification here used the installed GNOME SDK:

```bash
SDK=/var/lib/flatpak/runtime/org.gnome.Sdk/x86_64/50/1b43ad7e074959ab52d746fbd968108fe4f27bd53a4fb14adbb45aff4ef0354c/files
PKG_CONFIG_PATH="$SDK/lib/x86_64-linux-gnu/pkgconfig:$SDK/share/pkgconfig:$SDK/lib/pkgconfig" cargo check
```

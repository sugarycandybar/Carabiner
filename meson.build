project('carabiner', 'rust',
  version: '1.1.0',
  meson_version: '>= 0.59.0'
)

dependency('gtk4', version: '>= 4.0.0')
dependency('libadwaita-1', version: '>= 1.0.0')

cargo_options = [ '--manifest-path', meson.project_source_root() / 'Cargo.toml' ]
cargo_options += [ '--target-dir', meson.project_build_root() / 'src' ]

if get_option('buildtype') == 'release'
  cargo_options += [ '--release' ]
  rust_target = 'release'
else
  rust_target = 'debug'
endif

cargo_env = [ 'CARGO_HOME=' + meson.project_build_root() / 'cargo-home' ]

cargo_build = custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  command: [
    'sh', '-c',
    'export CARGO_HOME="$1"; cargo build --manifest-path "$2" --target-dir "$3" $4 && cp "$3"/"$5"/carabiner "$6"',
    '--',
    meson.project_build_root() / 'cargo-home',
    meson.project_source_root() / 'Cargo.toml',
    meson.project_build_root() / 'src',
    (get_option('buildtype') == 'release' ? '--release' : ''),
    rust_target,
    '@OUTPUT@'
  ]
)

install_data(
  'data/io.github.sugarycandybar.Carabiner.desktop',
  install_dir: get_option('datadir') / 'applications'
)

install_data(
  'data/io.github.sugarycandybar.Carabiner.metainfo.xml',
  install_dir: get_option('datadir') / 'metainfo'
)

install_data('data/icons/hicolor/scalable/apps/io.github.sugarycandybar.Carabiner.svg',
  install_dir: get_option('datadir') / 'icons/hicolor/scalable/apps'
)

install_data('data/icons/hicolor/symbolic/apps/io.github.sugarycandybar.Carabiner-symbolic.svg',
  install_dir: get_option('datadir') / 'icons/hicolor/symbolic/apps'
)


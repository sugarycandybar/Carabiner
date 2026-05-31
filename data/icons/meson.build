application_id = 'io.github.sugarycandybar.Carabiner'

scalable_dir = 'hicolor' / 'scalable' / 'apps'
install_data(
  scalable_dir / ('@0@.svg').format(application_id),
  scalable_dir / 'io.github.sugarycandybar.Hosty.svg',
  scalable_dir / 'io.github.sugarycandybar.Crucible.svg',
  install_dir: get_option('datadir') / 'icons' / scalable_dir
)

symbolic_dir = 'hicolor' / 'symbolic' / 'apps'
install_data(
  symbolic_dir / ('@0@-symbolic.svg').format(application_id),
  install_dir: get_option('datadir') / 'icons' / symbolic_dir
)

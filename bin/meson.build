conf = configuration_data()
conf.set('PYTHON', python.full_path())

configure_file(
  input: 'carabiner.in',
  output: 'carabiner',
  configuration: conf,
  install: true,
  install_dir: get_option('bindir'),
  install_mode: 'rwxr-xr-x'
)

python_sources = [
  '__init__.py',
  'application.py',
  'version.py',
  'window.py',
]

python.install_sources(
  python_sources,
  subdir: 'carabiner'
)

backend_sources = [
  'backend/__init__.py',
  'backend/cloudflare_manager.py',
  'backend/constants.py',
  'backend/events.py',
  'backend/ngrok_manager.py',
  'backend/playit_manager.py',
  'backend/tunnel_store.py',
]

python.install_sources(
  backend_sources,
  subdir: 'carabiner/backend'
)

# Also install backend files
# I need to check if there are any backend files

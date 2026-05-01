#!/usr/bin/env python3
import sys

# Ensure Carabiner is in the Python path
import os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from carabiner.application import CarabinerApplication

def main():
    app = CarabinerApplication()
    return app.run(sys.argv)

if __name__ == "__main__":
    sys.exit(main())

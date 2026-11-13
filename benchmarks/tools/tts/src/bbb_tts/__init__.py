"""Synthetic multi-speaker meeting audio from scripted turns."""

import os

from . import paths

# Keep downloaded model weights under benchmarks/data (untracked) instead of the user profile.
os.environ.setdefault("HF_HOME", str(paths.hf_cache_dir()))

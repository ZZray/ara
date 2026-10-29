"""Actual Host lifecycle evidence with current-shape imported archive frames.

This delegates the unchanged snapcompact Host runner and changes only the stale
seed's frame metadata to font 8x13, variant bw, and lineRepeat 1. Thresholds,
other inputs, assertions, CLI, process behavior, and binary pins stay unchanged.
This is a distinct current-frame input family, not acceptance of the original
legacy 49-fixture surface or the controlled legacy-usage test family.
"""
from __future__ import annotations

from pathlib import Path

import verify_snapcompact_host as host

_original_stale_seed = host.stale_seed
_original_source_pins = host.source_pins


def stale_seed(*, frames=16, kept_text="Keep this actual User source."):
    entries = _original_stale_seed(frames=frames, kept_text=kept_text)
    for frame in entries[-1]["preserveData"]["snapcompact"]["frames"]:
        frame.update(font="8x13", variant="bw", lineRepeat=1)
    return entries


def source_pins():
    pins = _original_source_pins()
    path = Path(__file__).resolve()
    pins[path.relative_to(host.shared.ROOT).as_posix()] = host.shared.digest(path)
    return pins


def main():
    host.stale_seed = stale_seed
    host.source_pins = source_pins
    return host.main()


if __name__ == "__main__":
    raise SystemExit(main())

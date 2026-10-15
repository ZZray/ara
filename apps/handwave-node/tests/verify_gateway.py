"""Actual cloud Gateway/model trial through an isolated local modern HandWave Hub.

Uses verify_cloud's credential-safe, temporary-identity and bounded task harness.
Production public HandWave is explicitly outside this acceptance boundary.
"""
import sys

from verify_cloud import main


if __name__ == "__main__":
    if "--isolated-hub" not in sys.argv:
        sys.argv.append("--isolated-hub")
    main()

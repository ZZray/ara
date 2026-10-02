"""Run fixed OMP model composition and selected actual Host module families.

Development uses --module for the affected family; --full runs one final
backend, inventory, dependency and build gate on a stable source snapshot.
This runner never opens account data or private local model configuration.
"""
import verify_config_values as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-registry-compose-batch"
shared.MODULES = {
    "patch": ("ara-cli", "model_patch"),
    "custom": ("ara-cli", "custom_models"),
    "static": ("ara-cli", "static_model_registry"),
    "values": ("ara-cli", "model_config_values"),
    "collapse": ("ara-cli", "@lib"),
    "selection": ("ara-cli", "daily_model_config"),
    "host": ("ara-cli", "config_command_cli"),
    "daily-cli": ("ara-cli", "openai_daily_cli"),
}

original_source_pins = inherited.source_pins


def source_pins():
    pins = original_source_pins()
    path = shared.ROOT / "scripts/verify_registry_composition.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = inherited.hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())

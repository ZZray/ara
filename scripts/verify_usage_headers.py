"""Run fixed OMP header-ingestion module families; --full adds one backend gate.

Storage uses the original Codex selection/merge inputs. Route tests exercise
the real Host callback, final OpenAI response and ordinary Codex boundary.
Controlled callbacks are distinct from actual model/account authorization.
"""
import hashlib
import verify_auth_storage as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-header-batch"
shared.MODULES = {
    "storage": ("ara-cli", "@lib"),
    "requests": ("ara-cli", "model_route"),
    "registry": ("ara-cli", "model_registry"),
    "chat": ("ara-ai", "openai_http"),
    "responses": ("ara-ai", "openai_responses_http"),
    "codex": ("ara-ai", "openai_codex_http"),
}
original_source_pins = shared.source_pins


def source_pins():
    pins = original_source_pins()
    path = shared.ROOT / "scripts/verify_usage_headers.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())

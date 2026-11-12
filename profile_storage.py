"""Private per-user storage for microphone enrollment recordings."""
import os
from pathlib import Path
import shutil


def reference_path():
    """Return a writable per-user location, independent of install directory."""
    base = os.environ.get("LOCALAPPDATA")
    if not base:
        base = Path.home() / "AppData" / "Local"
    return Path(base) / "VoiceIsolation" / "我的声样.wav"


def prepare_reference(destination, legacy_candidates=()):
    """Create storage and, if needed, copy an adjacent older profile once.

    The prior recording remains intact.  An existing destination always wins;
    nothing in another user's home directory is searched.
    """
    destination = Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.is_file():
        return False
    for candidate in legacy_candidates:
        candidate = Path(candidate)
        if not candidate.is_file():
            continue
        created = False
        try:
            with candidate.open('rb') as source:
                # Exclusive creation prevents overwriting an existing profile.
                with destination.open('xb') as target:
                    created = True
                    shutil.copyfileobj(source, target)
            return True
        except FileExistsError:
            return False
        except OSError:
            if created:
                destination.unlink(missing_ok=True)
            raise
    return False

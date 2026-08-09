#!/bin/sh
# Installs the system packages this project needs in order to build.
#
# Bevy links against ALSA, libudev and Wayland; without their -dev packages the
# build dies ~400 crates deep inside alsa-sys, with a pkg-config error that
# mentions none of this project. build.rs shells out to rsvg-convert for the
# resource-icon sprites, and to openscad/assimp for meshes -- the generated
# .gltf files are checked in, so those last two are only needed to regenerate
# them, and build.rs merely warns when they're absent.
#
# Wired up as a SessionStart hook in .claude/settings.json. Exits immediately
# and silently when there is nothing to do, so it only costs anything on a
# fresh container. It never fails the session: no apt-get (macOS), no sudo, or
# a failed install all just leave the machine as it was.
#
# See DEVELOPING.md ("Running tests on headless Linux") for doing this by hand.

set -u

# Bevy cannot link without these.
ESSENTIAL="libasound2-dev libudev-dev libwayland-dev librsvg2-bin"
# Only needed to regenerate meshes from buildables/*.scad.
MESHES="openscad assimp-utils"

# Deliberately checks only the essential packages: if the mesh tooling can't be
# installed here, we still want the fast exit below rather than an apt-get on
# every single session start.
have_essentials() {
    pkg-config --exists alsa 2>/dev/null || return 1
    pkg-config --exists libudev 2>/dev/null || return 1
    pkg-config --exists wayland-client 2>/dev/null || return 1
    command -v rsvg-convert >/dev/null 2>&1 || return 1
    return 0
}

have_essentials && exit 0
command -v apt-get >/dev/null 2>&1 || exit 0

if [ "$(id -u)" = 0 ]; then
    SUDO=""
elif sudo -n true 2>/dev/null; then
    SUDO="sudo -n"
else
    echo "orchitecture: build dependencies are missing and sudo isn't available." >&2
    echo "  Install them by hand -- see DEVELOPING.md." >&2
    exit 0
fi

echo "orchitecture: installing build dependencies (first run in this container)..." >&2

# Third-party PPAs on some images fail to refresh; that must not stop us from
# installing from the main archive, so the result is deliberately ignored.
$SUDO apt-get update -qq >/dev/null 2>&1

if $SUDO apt-get install -y -qq $ESSENTIAL >/dev/null 2>&1; then
    echo "orchitecture: build dependencies ready." >&2
else
    echo "orchitecture: failed to install: $ESSENTIAL" >&2
    echo "  'cargo build' will fail in alsa-sys until these are present." >&2
fi

# Best-effort: absence only costs mesh regeneration, so a failure is not worth
# reporting.
$SUDO apt-get install -y -qq $MESHES >/dev/null 2>&1 || true

exit 0

#!/usr/bin/env bash
set -euo pipefail
if pgrep -x "Emacs" > /dev/null || pgrep -x "emacs" > /dev/null; then
    echo "error: Emacs is currently running — please quit Emacs before signing" >&2
    exit 1
fi
usage() {
    echo "usage: $(basename "$0") <path-to-emacs-or-emacs.app>" >&2
    echo >&2
    echo "  $(basename "$0") /Applications/Emacs.app" >&2
    echo "  $(basename "$0") /opt/homebrew/bin/emacs" >&2
    echo "  $(basename "$0") \$(which emacs)" >&2
    exit 1
}

[[ $# -eq 1 ]] || usage

INPUT="$1"

# Resolve .app bundle to the actual binary
if [[ "$INPUT" == *.app ]]; then
    if [[ ! -d "$INPUT" ]]; then
        echo "error: .app bundle not found: $INPUT" >&2
        exit 1
    fi
    # Try common locations inside the bundle
    EMACS=""
    for candidate in \
        "$INPUT/Contents/MacOS/Emacs" \
        "$INPUT/Contents/MacOS/emacs"
    do
        if [[ -f "$candidate" ]]; then
            EMACS="$candidate"
            break
        fi
    done
    if [[ -z "$EMACS" ]]; then
        echo "error: could not find Emacs binary inside $INPUT" >&2
        exit 1
    fi
else
    if [[ ! -f "$INPUT" ]]; then
        echo "error: binary not found: $INPUT" >&2
        exit 1
    fi
    # Resolve symlinks (e.g. Homebrew shims point to the real binary)
    EMACS="$(readlink -f "$INPUT" 2>/dev/null || realpath "$INPUT")"
fi

echo "target: $EMACS"

ENTITLEMENTS=$(mktemp /tmp/emacs-sign.XXXXXX.plist)
trap 'rm -f "$ENTITLEMENTS"' EXIT

cat > "$ENTITLEMENTS" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.get-task-allow</key>
    <true/>
</dict>
</plist>
PLIST

codesign \
    --sign - \
    --force \
    --entitlements "$ENTITLEMENTS" \
    --options runtime \
    "$EMACS"

echo "done: $EMACS signed with get-task-allow"
echo "note: re-run this script after Emacs updates"

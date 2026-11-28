#!/bin/sh
# Install the occam-rzr CLI and the udev rule that lets you use Razer devices without root.
#   sudo ./install.sh              install to /usr/local/bin
#   sudo ./install.sh --uninstall  remove both
set -e
cd "$(dirname "$0")"
bin=/usr/local/bin/occam-rzr
rule=/etc/udev/rules.d/70-occam-rzr.rules
if [ "$(id -u)" -ne 0 ]; then
    echo "Run it with sudo: sudo $0 $*" >&2
    exit 1
fi
if [ "$1" = "--uninstall" ]; then
    rm -f "$bin" "$rule"
else
    install -m 755 occam-rzr "$bin"
    install -m 644 70-occam-rzr.rules "$rule"
fi
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --subsystem-match=hidraw 2>/dev/null || true
if [ "$1" = "--uninstall" ]; then
    echo "Removed $bin and $rule. Your settings stay in ~/.config/occam-rzr."
else
    echo "Installed $bin and $rule. Try: occam-rzr list"
    echo "If your device isn't listed, unplug it and plug it back in."
fi

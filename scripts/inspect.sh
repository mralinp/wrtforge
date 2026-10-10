. /etc/openwrt_release 2>/dev/null || { echo NOTOPENWRT; exit 0; }
echo "$DISTRIB_ID|$DISTRIB_RELEASE|$DISTRIB_TARGET|$DISTRIB_ARCH"
jsonfilter -i /etc/board.json -e '@.model.id' -e '@.model.name'
awk '/MemAvailable/{print $2}' /proc/meminfo
command -v sysupgrade >/dev/null && echo HAS_SYSUPGRADE
(fw_printenv -n boot_part >/dev/null 2>&1 && grep -q alt_kernel /proc/mtd && grep -q alt_rootfs /proc/mtd) && echo DUAL || true
command -v apk >/dev/null && echo PM_APK || echo PM_OPKG
[ -e /etc/config/passwall2 ] && echo HAS_PW2 || true

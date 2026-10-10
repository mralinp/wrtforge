# write the official image into the INACTIVE slot (the one we are not running from)
KERN=@KERN@; ROOT=@ROOT@
kn=$(awk -v n="\"$KERN\"" '$4==n{print substr($1,4,length($1)-4)}' /proc/mtd)
rn=$(awk -v n="\"$ROOT\"" '$4==n{print substr($1,4,length($1)-4)}' /proc/mtd)
[ -n "$kn" ] && [ -n "$rn" ]
for u in /sys/class/ubi/ubi[0-9]*; do
  m=$(cat $u/mtd_num 2>/dev/null) || continue
  [ "$m" = "$rn" ] || [ "$m" = "$kn" ] && ubidetach -d ${u##*ubi} || true
done
mtd -q erase $KERN >/dev/null 2>&1
ubiformat /dev/mtd$rn -y -q >/dev/null 2>&1
ubiattach -m $rn -d 8 >/dev/null
ubimkvol /dev/ubi8 -N rootfs -s @RSIZE@ >/dev/null
ubimkvol /dev/ubi8 -N rootfs_data -m >/dev/null
ubiupdatevol /dev/ubi8_0 -s @RSIZE@ /tmp/wf-root.bin
mtd -q write /tmp/wf-kernel.bin $KERN
k=$(dd if=/dev/mtd$kn bs=@KSIZE@ count=1 2>/dev/null | sha256sum | cut -d' ' -f1)
x=$(dd if=/dev/ubi8_0 bs=@RSIZE@ count=1 2>/dev/null | sha256sum | cut -d' ' -f1)
ubidetach -d 8
rm -f /tmp/wf-kernel.bin /tmp/wf-root.bin
echo "$k $x"

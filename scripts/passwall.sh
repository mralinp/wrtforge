PM=@PM@
BASE=@BASE@
DEPS="xray-core chinadns-ng geoview v2ray-geoip v2ray-geosite dns2socks tcping ip-full kmod-nft-tproxy kmod-nft-socket coreutils-nohup coreutils-base64 resolveip"
if [ $PM = apk ]; then
  wget -qO /etc/apk/keys/passwall.pem $BASE/apk.pub
  for f in @FEEDS@; do grep -qxF "$f" /etc/apk/repositories.d/customfeeds.list 2>/dev/null || echo "$f" >> /etc/apk/repositories.d/customfeeds.list; done
  apk update >/dev/null
  apk info -e dnsmasq-full >/dev/null || { apk del dnsmasq >/dev/null; apk add dnsmasq-full >/dev/null || apk add dnsmasq >/dev/null; }
  /etc/init.d/dnsmasq restart >/dev/null 2>&1 || true
  [ -e /www/cgi-bin/luci ] || apk add luci >/dev/null
  apk add $DEPS >/dev/null
  apk add luci-app-passwall2 >/dev/null
else
  wget -qO /tmp/ipk.pub $BASE/ipk.pub && opkg-key add /tmp/ipk.pub
  for f in @FEEDS@; do n=pw_$(basename $f); grep -q "$f" /etc/opkg/customfeeds.conf 2>/dev/null || echo "src/gz $n $f" >> /etc/opkg/customfeeds.conf; done
  opkg update >/dev/null
  opkg list-installed | grep -q '^dnsmasq-full ' || { opkg remove dnsmasq >/dev/null; opkg install dnsmasq-full >/dev/null || opkg install dnsmasq >/dev/null; }
  /etc/init.d/dnsmasq restart >/dev/null 2>&1 || true
  [ -e /www/cgi-bin/luci ] || opkg install luci >/dev/null
  opkg install $DEPS >/dev/null
  opkg install luci-app-passwall2 >/dev/null
fi
# passwall2 rejects the legacy value "disable" for these ports (nft errors on every start)
for o in tcp_no_redir_ports udp_no_redir_ports; do
  [ "$(uci -q get passwall2.@global_forwarding[0].$o)" = disable ] && uci delete passwall2.@global_forwarding[0].$o || true
done
uci commit passwall2 2>/dev/null || true
if [ "$(uci -q get passwall2.@global[0].enabled)" = 1 ]; then /etc/init.d/passwall2 restart >/dev/null 2>&1 || true; sleep 8; pgrep -f 'bin/(xray|sing-box)' >/dev/null && echo RUNNING; fi
echo INSTALLED

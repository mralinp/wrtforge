#!/usr/bin/env bash
# Runs on the CLIENT machine (wired to the router's LAN). Switches the router's Passwall2 to a local
# Xray server, pushes iperf3 traffic THROUGH the router, samples per-core router CPU, writes a CSV.
# The router's Passwall2 config is backed up first and restored on exit (also on Ctrl-C).
# usage: bench/run.sh <router-ip> <params.env> [secs=30] [reps=3] [scenarios...]
# scenarios: nat vless-tcp vless-tls ss-aes ss-chacha   (default: all; nat needs the server on ANOTHER machine,
# because a packet to this machine's own IP never crosses the router)
# needs: iperf3, ssh key access to the router (root), the server from bench/server.sh running.
set -eu
R=${1:?router ip}; ENVF=${2:?params.env}; SECS=${3:-30}; REPS=${4:-3}; shift 4 2>/dev/null || shift $#
SCEN=${*:-nat vless-tcp vless-tls ss-aes ss-chacha}
. "$ENVF"
PIN=$(openssl x509 -in "$(dirname "$ENVF")/cert.pem" -noout -fingerprint -sha256 | cut -d= -f2 | tr -d ':' | tr A-F a-f)
local_ip() { { ifconfig 2>/dev/null || ip -4 addr; } | grep -qE "inet $SERVER_IP([ /]|$)"; }
if local_ip; then
  case " $SCEN " in *" nat "*) echo "server is this machine: skipping the nat baseline (needs a second machine)"; SCEN=${SCEN/nat/};; esac
fi
gw=$(route -n get "$TARGET" 2>/dev/null | awk '/gateway:/{print $2}'); [ -n "$gw" ] || gw=$(ip route get "$TARGET" 2>/dev/null | awk '{for(i=1;i<NF;i++)if($i=="via")print $(i+1)}')
[ "$gw" = "$R" ] || { echo "traffic to $TARGET leaves via '$gw', not the router $R: put the LAN cable first in the network order / disable the other default route"; exit 1; }
SSH="ssh -o BatchMode=yes -o ConnectTimeout=8 root@$R"
OUT=${BENCH_OUT:-wrtforge-bench.csv}
MODEL=$($SSH 'jsonfilter -i /etc/board.json -e "@.model.name"')
REL=$($SSH '. /etc/openwrt_release; echo $DISTRIB_RELEASE')

BK=/root/passwall2.bench-backup
$SSH "[ -f $BK ] || cp /etc/config/passwall2 $BK"
restore() { $SSH "cp $BK /etc/config/passwall2 && /etc/init.d/passwall2 restart" >/dev/null 2>&1 \
  && echo "router restored from $BK"; }
trap restore EXIT

setnode() { # $1 = scenario
  case $1 in
    nat) $SSH "uci set passwall2.@global[0].enabled=0; uci commit passwall2; /etc/init.d/passwall2 stop"; DEST=$SERVER_IP; return ;;
  esac
  DEST=$TARGET
  $SSH "uci -q delete passwall2.wfbench; uci set passwall2.wfbench=nodes
    uci set passwall2.wfbench.remarks=wfbench; uci set passwall2.wfbench.type=Xray
    uci set passwall2.wfbench.address=$SERVER_IP; uci set passwall2.wfbench.tcp_fast_open=0
    case $1 in
      vless-tcp) uci set passwall2.wfbench.port=10001; uci set passwall2.wfbench.protocol=vless; uci set passwall2.wfbench.uuid=$UUID
                 uci set passwall2.wfbench.encryption=none; uci set passwall2.wfbench.transport=raw; uci set passwall2.wfbench.tls=0 ;;
      vless-tls) uci set passwall2.wfbench.port=10002; uci set passwall2.wfbench.protocol=vless; uci set passwall2.wfbench.uuid=$UUID
                 uci set passwall2.wfbench.encryption=none; uci set passwall2.wfbench.transport=raw; uci set passwall2.wfbench.tls=1
                 uci set passwall2.wfbench.tls_pinSHA256=$PIN; uci set passwall2.wfbench.tls_serverName=wrtbench ;;
      ss-aes)    uci set passwall2.wfbench.port=10003; uci set passwall2.wfbench.protocol=shadowsocks
                 uci set passwall2.wfbench.ss_method=aes-128-gcm; uci set passwall2.wfbench.password='$SSPASS' ;;
      ss-chacha) uci set passwall2.wfbench.port=10004; uci set passwall2.wfbench.protocol=shadowsocks
                 uci set passwall2.wfbench.ss_method=chacha20-ietf-poly1305; uci set passwall2.wfbench.password='$SSPASS' ;;
      *) echo unknown scenario; exit 1 ;;
    esac
    uci set passwall2.@global[0].enabled=1; uci set passwall2.@global[0].node=wfbench
    uci commit passwall2; /etc/init.d/passwall2 restart" >/dev/null
  sleep 10
  $SSH 'pgrep xray >/dev/null' || { echo "xray is not running on the router for $1"; exit 1; }
}

cpu_snap() { $SSH "grep '^cpu[0-9]' /proc/stat"; }
cpu_pct() { # before after -> "avg max" busy % over cores
  awk 'FNR==NR{for(i=2;i<=9;i++)t0[$1]+=$i; i0[$1]=$5+$6; next}
       {t=0; for(i=2;i<=9;i++)t+=$i; b=100*(1-(($5+$6)-i0[$1])/(t-t0[$1])); s+=b; n++; if(b>m)m=b}
       END{printf "%.0f %.0f", s/n, m}' <(echo "$1") <(echo "$2")
}
median() { sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}'; }
iperf() { # extra args -> Mbps (receiver side)
  iperf3 -c "$DEST" -P 8 -t "$SECS" "$@" | awk '/\[SUM\].*receiver/{v=$(NF-2);u=$(NF-1)} END{if(u~/^G/)v*=1000; printf "%.0f\n", v}'
}

[ -f "$OUT" ] || echo "router,openwrt,scenario,down_mbps,up_mbps,cpu_avg_down,cpu_maxcore_down,cpu_avg_up,cpu_maxcore_up" >"$OUT"
DEST=$TARGET
for s in $SCEN; do
  echo "== $MODEL: $s"; setnode "$s"
  iperf3 -c "$DEST" -t 2 >/dev/null || { echo "no traffic path to $DEST in $s"; exit 1; }
  dl=(); ul=(); cd=(); cu=()
  for _ in $(seq "$REPS"); do
    a=$(cpu_snap); dl+=("$(iperf -R)"); b=$(cpu_snap); cd+=("$(cpu_pct "$a" "$b")")
    ul+=("$(iperf)"); c=$(cpu_snap); cu+=("$(cpu_pct "$b" "$c")")
  done
  D=$(printf '%s\n' "${dl[@]}" | median); U=$(printf '%s\n' "${ul[@]}" | median)
  CD=$(printf '%s\n' "${cd[@]}" | sort -k2 -n | sed -n "$(( (REPS+1)/2 ))p"); CU=$(printf '%s\n' "${cu[@]}" | sort -k2 -n | sed -n "$(( (REPS+1)/2 ))p")
  echo "   down $D Mbps (cpu avg/max-core ${CD% *}/${CD#* }%)   up $U Mbps (cpu ${CU% *}/${CU#* }%)"
  echo "$MODEL,$REL,$s,$D,$U,${CD% *},${CD#* },${CU% *},${CU#* }" >>"$OUT"
done
echo "results appended to $OUT"

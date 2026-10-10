#!/usr/bin/env bash
# Runs on the SERVER machine, on the router's WAN side (e.g. 192.168.70.x; this can be the same laptop
# that runs run.sh over a LAN cable). Starts an Xray server (VLESS tcp, VLESS+TLS, Shadowsocks aes/chacha)
# whose outbound redirects everything to the local iperf3, and writes bench-server/params.env for run.sh.
# Needs: xray, iperf3, openssl.
# usage: bench/server.sh <this-machine-ip-on-the-router-wan-network>
set -eu
SERVER_IP=${1:?usage: $0 <server-ip>}
TARGET=203.0.113.10 # public-looking (TEST-NET-3) fake address: Passwall2 bypasses private IPs; Xray redirects it to iperf3
D=bench-server
mkdir -p "$D" && cd "$D"
for t in xray iperf3 openssl; do command -v $t >/dev/null || { echo "missing: $t"; exit 1; }; done

[ -s uuid ] || xray uuid >uuid
[ -s sspass ] || openssl rand -base64 16 >sspass
UUID=$(cat uuid); SSPASS=$(cat sspass)
[ -f cert.pem ] || openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj /CN=wrtbench \
  -keyout key.pem -out cert.pem 2>/dev/null

cat > config.json <<JSON
{ "log": {"loglevel": "warning"},
  "inbounds": [
    {"tag":"vless-tcp","port":10001,"protocol":"vless","settings":{"clients":[{"id":"$UUID"}],"decryption":"none"},"streamSettings":{"network":"tcp"}},
    {"tag":"vless-tls","port":10002,"protocol":"vless","settings":{"clients":[{"id":"$UUID"}],"decryption":"none"},"streamSettings":{"network":"tcp","security":"tls","tlsSettings":{"certificates":[{"certificateFile":"$PWD/cert.pem","keyFile":"$PWD/key.pem"}]}}},
    {"tag":"ss-aes","port":10003,"protocol":"shadowsocks","settings":{"method":"aes-128-gcm","password":"$SSPASS","network":"tcp,udp"}},
    {"tag":"ss-chacha","port":10004,"protocol":"shadowsocks","settings":{"method":"chacha20-ietf-poly1305","password":"$SSPASS","network":"tcp,udp"}}
  ],
  "outbounds": [{"protocol":"freedom","settings":{"redirect":"127.0.0.1:5201"}}] }
JSON

cat > params.env <<ENV
SERVER_IP=$SERVER_IP
TARGET=$TARGET
UUID=$UUID
SSPASS=$SSPASS
ENV

trap 'kill $(jobs -p) 2>/dev/null' EXIT INT TERM
iperf3 -s -B 127.0.0.1 >/dev/null &
echo "server ready: xray on $SERVER_IP:10001-10004, iperf3 behind $TARGET"
echo "copy $PWD/params.env to the client machine and run bench/run.sh"
xray run -c config.json

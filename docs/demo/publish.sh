#!/usr/bin/env bash
# Usage: publish.sh PORT
# Feeds the README demo broker made-up home data: retained state once, then
# a live update about every second until killed.
set -u
port=${1:?usage: publish.sh PORT}
pub() { mosquitto_pub -h 127.0.0.1 -p "$port" "$@"; }

pub -r -t home/hall/door -m closed
pub -r -t home/office/lamp -m ON
pub -r -t home/livingroom/tv/state -m '{"power":"on","source":"hdmi1","volume":18}'
pub -r -t garage/car/battery -m '{"soc":81,"range_km":312,"charging":false}'

i=0
while :; do
    t=$(awk -v i="$i" 'BEGIN { printf "%.1f", 21.2 + 0.8 * sin(i / 5) }')
    h=$(awk -v i="$i" 'BEGIN { printf "%d", 47 + 4 * cos(i / 7) }')
    w=$(awk -v i="$i" 'BEGIN { printf "%d", 1180 + 520 * sin(i / 3) + (i * 37) % 90 }')
    v=$(awk -v i="$i" 'BEGIN { printf "%.1f", 230.4 + 1.8 * sin(i / 4) + ((i * 13) % 7) / 10 }')
    k=$(awk -v i="$i" 'BEGIN { printf "%.2f", 3.1 + i * 0.01 }')
    pub -r -t home/livingroom/climate -m "{\"temperature\":$t,\"humidity\":$h}"
    pub -r -t home/bedroom/climate -m "{\"temperature\":$(awk -v t="$t" 'BEGIN { printf "%.1f", t - 1.6 }'),\"humidity\":$((h + 3))}"
    pub -r -t home/kitchen/plug -m "{\"POWER\":\"ON\",\"ENERGY\":{\"Power\":$w,\"Voltage\":$v,\"Today\":$k}}"
    pub -r -t home/garden/weather -m "{\"wind_kmh\":$((8 + i % 6)),\"rain_mm\":0.2,\"temperature\":$(awk -v t="$t" 'BEGIN { printf "%.1f", t - 9.3 }')}"
    ((i % 9 == 4)) && pub -t home/hall/door -m open
    ((i % 9 == 6)) && pub -r -t home/hall/door -m closed
    i=$((i + 1))
    sleep 1
done

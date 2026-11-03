# README demo. Re-record from the repo root after UI changes:
#   cargo build --release && vhs docs/demo/demo.tape
# Uses a throwaway local broker on port 18883 fed with made-up data.

Output docs/demo/demo.gif

Require mosquitto
Require mosquitto_pub

Set Shell bash
Set FontSize 15
Set Width 1280
Set Height 760
Set Padding 18
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set BorderRadius 10
Set TypingSpeed 70ms
Set Framerate 15
Set PlaybackSpeed 1.0

Hide
Type "export MQTT_ZAPP_CONFIG_DIR=docs/demo PATH=$PWD/target/release:$PATH"
Enter
Type "mosquitto -c docs/demo/mosquitto.conf >/dev/null 2>&1 & MQ=$!; sleep 0.5; docs/demo/publish.sh 18883 >/dev/null 2>&1 & PUB=$!"
Enter
# Build up some history for the chart.
Sleep 25s
Type "clear; mqtt-zapp"
Enter
Sleep 1s
Show

# The broker picker.
Sleep 1.5s
# Connect to the demo profile.
Enter
Sleep 2.5s

# Walk down to home/kitchen/plug.
Down@180ms 12
Sleep 2s

# Chart its numeric fields; the charted line is highlighted.
Type "c"
Sleep 3s
Screenshot docs/demo/screenshot.png
Type "c"
Sleep 2.5s
Type "c"
Sleep 2.5s
Type "c"
Sleep 1s

# Diff against the previous message.
Type "d"
Sleep 3s
Type "d"
Sleep 1s

# Filter the tree.
Type "/"
Sleep 300ms
Type "climate"
Sleep 2.5s
Escape
Sleep 1s

# Key help.
Type "?"
Sleep 3s
Escape
Sleep 500ms
Type "q"
Sleep 500ms

Hide
Type "kill $PUB $MQ"
Enter

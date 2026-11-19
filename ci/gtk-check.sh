#!/usr/bin/env bash
set -euxo pipefail

output=${RUI_CI_OUTPUT:-/tmp/rui-gtk-ci}
mkdir -p "$output"
test "$(pkg-config --modversion gtk4)" = 4.22.4
test "$(pkg-config --modversion cairo)" = 1.18.4
test "$(node --version)" = v26.8.1
rustc --version | grep '^rustc 1.98.1 '
python3 -c 'import gi, cairo; assert gi.__version__ == "3.58.1"; assert cairo.version == "1.28.0"'
python3 client-wasm/build.py
npm --prefix client-browser ci
npm --prefix demos/web ci
npm --prefix demos/semantic ci
test "$(node -p 'require("./client-browser/node_modules/playwright/package.json").version')" = 1.63.0
npm --prefix client-browser run build
npm --prefix demos/web run build
cargo build --release -p rui-client-desktop --features gtk-demo --bins --locked
python3 client-browser/tests/package-live.py --xvfb /usr/bin/Xvfb
RUI_LIFECYCLE=1 python3 client-browser/tests/package-live.py --xvfb /usr/bin/Xvfb
cargo build --release -p rui-client-desktop --features gtk-source-probe --example gtk_source_probe --locked
python3 fixtures/gtk/mirror_check.py --xvfb /usr/bin/Xvfb --scenario composition --output "$output/rui-shared-native"
python3 fixtures/gtk/measure.py --xvfb /usr/bin/Xvfb --output "$output/rui-coverage"
python3 fixtures/gtk/negative_check.py --xvfb /usr/bin/Xvfb --output "$output/rui-negatives"
python3 fixtures/gtk/inventory.py "$output/rui-coverage/measurement.json" "$output/rui-negatives/negative-report.json" --output "$output/rui-inventory.json"
cargo build --release -p rui-client-desktop --example render_capture --locked
python3 fixtures/gtk/border_check.py --xvfb /usr/bin/Xvfb --output "$output/rui-borders"
cargo build -p rui-producer-sdk --features gtk-fixture-hooks --bin gtk_command_check --locked
python3 fixtures/gtk/native_check.py --case editor --xvfb /usr/bin/Xvfb --output "$output/rui-editor"
python3 demos/semantic/session_check.py --dom-controls --xvfb /usr/bin/Xvfb --output "$output/rui-semantic-corpus"
python3 demos/semantic/session_check.py --dom-controls --xvfb /usr/bin/Xvfb --application editor --output "$output/rui-semantic-editor"
RUI_RESOURCE_RECOVERY=1 python3 demos/semantic/presentation_check.py --xvfb /usr/bin/Xvfb --output "$output/rui-presentation-corpus"
RUI_RESOURCE_RECOVERY=1 python3 demos/semantic/presentation_check.py --xvfb /usr/bin/Xvfb --application editor --output "$output/rui-presentation-editor"
python3 fixtures/atspi/native_check.py --xvfb /usr/bin/Xvfb --output "$output/rui-observed"
python3 demos/web/check.py --xvfb /usr/bin/Xvfb --timeout 90
RUI_BROWSER_BUILD=1 python3 demos/web/check.py --xvfb /usr/bin/Xvfb --timeout 90

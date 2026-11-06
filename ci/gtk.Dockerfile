FROM mcr.microsoft.com/playwright:v1.63.0-noble@sha256:eff16c30e6f3f4af0a03fa4b706120d5e9b0891c344a27d64559aff5900a4a27 AS browsers
FROM rust:1.98.1-slim-trixie@sha256:4cd829461bd5c4d511c32e269da9cb8929223b666519d8004e35fc8d1d771ab7

RUN apt-get update && apt-get install -y --no-install-recommends build-essential ca-certificates curl git xz-utils meson ninja-build pkg-config protobuf-compiler python3 python3-gi python3-gi-cairo python3-cairo python3-pil gobject-introspection libgirepository1.0-dev libglib2.0-dev libcairo2-dev libpango1.0-dev libharfbuzz-dev libfribidi-dev libgdk-pixbuf-2.0-dev libgraphene-1.0-dev libepoxy-dev libpng-dev libtiff-dev libjpeg-dev libx11-dev libxrandr-dev libxi-dev libxext-dev libxcursor-dev libxdamage-dev libxfixes-dev libxinerama-dev libdrm-dev xvfb xauth dbus-x11 at-spi2-core gir1.2-atspi-2.0 libatomic1 libssl-dev openssl fonts-dejavu-core fonts-liberation libnss3 libnspr4 libatk1.0-0 libatk-bridge2.0-0 libxcomposite1 libgbm1 libasound2t64 libxkbcommon0 libcups2t64 && rm -rf /var/lib/apt/lists/*

WORKDIR /tmp/gtk-build
RUN curl --fail --location --retry 3 https://download.gnome.org/sources/gtk/4.22/gtk-4.22.4.tar.xz -o gtk.tar.xz && echo '51bd9f60c7d23a665a556c7364c21fb2e4e282566b3e7e092455e8f910330893  gtk.tar.xz' | sha256sum --check && tar -xf gtk.tar.xz
RUN meson setup build gtk-4.22.4 --prefix=/opt/rui-gtk --libdir=lib --buildtype=release -Dwayland-backend=false -Dmedia-gstreamer=disabled -Dvulkan=disabled -Dintrospection=enabled -Dbuild-tests=false -Dbuild-testsuite=false -Dbuild-examples=false -Dbuild-demos=false -Ddocumentation=false -Dman-pages=false -Dprint-cups=disabled && meson compile -C build -j 2 && meson install -C build && rm -rf /tmp/gtk-build
ENV PKG_CONFIG_PATH=/opt/rui-gtk/lib/pkgconfig
ENV LD_LIBRARY_PATH=/opt/rui-gtk/lib
ENV GI_TYPELIB_PATH=/opt/rui-gtk/lib/girepository-1.0
ENV XDG_DATA_DIRS=/opt/rui-gtk/share:/usr/local/share:/usr/share
RUN test "$(pkg-config --modversion gtk4)" = 4.22.4 && test "$(pkg-config --modversion cairo)" = 1.18.4 && python3 -c 'import gi, cairo; gi.require_version("Gtk", "4.0"); from gi.repository import Gtk; assert (Gtk.get_major_version(), Gtk.get_minor_version(), Gtk.get_micro_version()) == (4, 22, 4)'

RUN curl --fail --location --retry 3 https://nodejs.org/dist/v26.8.1/node-v26.8.1-linux-x64.tar.xz -o /tmp/node.tar.xz && echo '3e301118d7df53d563b7e96c1617545f26e2f76f9724be668d6cab65c15dda5d  /tmp/node.tar.xz' | sha256sum --check && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 && rm /tmp/node.tar.xz
RUN rustup target add wasm32-unknown-unknown && cargo install wasm-bindgen-cli --version 0.2.129 --locked
COPY --from=browsers /ms-playwright/chromium-1243 /ms-playwright/chromium-1243
COPY --from=browsers /ms-playwright/chromium_headless_shell-1243 /ms-playwright/chromium_headless_shell-1243
COPY --from=browsers /ms-playwright/ffmpeg-1011 /ms-playwright/ffmpeg-1011
ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright
ENV RUI_TEST_BROWSERS=chromium
ENV RUI_BROWSER_HEADED=1
ENV RUI_BROWSER_HTTPS=1
RUN apt-get update && apt-get install -y --no-install-recommends libgirepository-2.0-dev python3-dev python3-pip python3-mesonpy && rm -rf /var/lib/apt/lists/*
RUN curl --fail --location --retry 3 https://files.pythonhosted.org/packages/40/d9/412da520de9052b7e80bfc810ec10f5cb3dbfa4aa3e23c2820dc61cdb3d0/pycairo-1.28.0.tar.gz -o /tmp/pycairo.tar.gz && echo '26ec5c6126781eb167089a123919f87baa2740da2cca9098be8b3a6b91cc5fbc  /tmp/pycairo.tar.gz' | sha256sum --check && pip3 install --no-deps --no-build-isolation --ignore-installed --break-system-packages /tmp/pycairo.tar.gz && rm /tmp/pycairo.tar.gz
RUN curl --fail --location --retry 3 https://files.pythonhosted.org/packages/65/1e/d590f82e14db75945b03f6b63e0f19662e0340babd8f574fdb1bb5a015e2/pygobject-3.58.1.tar.gz -o /tmp/pygobject.tar.gz && echo '4c80598ade17fbaa7798e01a25d0bf29ce109740786026a074c5c62bb3e79d23  /tmp/pygobject.tar.gz' | sha256sum --check && pip3 install --no-deps --no-build-isolation --ignore-installed --break-system-packages /tmp/pygobject.tar.gz && rm /tmp/pygobject.tar.gz
RUN python3 -c 'import gi; assert gi.__version__ == "3.58.1"; gi.require_foreign("cairo"); gi.require_version("PangoCairo", "1.0"); from gi.repository import Pango, PangoCairo; context = PangoCairo.FontMap.get_default().create_context(); assert PangoCairo.Font.get_scaled_font(context.load_font(Pango.FontDescription("Sans 12"))) is not None'
RUN apt-get update && apt-get install -y --no-install-recommends fonts-noto-core libzstd1 && rm -rf /var/lib/apt/lists/*
COPY --chmod=755 chromium.sh /usr/local/bin/rui-ci-chromium
ENV RUI_WASM_CHROMIUM=/usr/local/bin/rui-ci-chromium
WORKDIR /workspace

# Additional session feasibility: Hyprland 0.56.2

C4 investigated the installed Hyprland 0.56.2 commit
`efb50993780079460b0cbed1363e2166a2de1d9f`, Wayland 1.26.0 and its installed
capture/mapping headers. The [bounded report](../fixtures/session/hyprland-feasibility.json)
hashes the actual inputs and records the binary's version/ABI dependencies.
Reproduce with `python3 session-shell/feasibility.py --output /tmp/rui-feasibility-NEW.json`.
It fails on pin or protocol drift rather than applying this conclusion to an
unreviewed installation. This was source/interface inspection, not an owned
compositor action-equivalence run or a query of the user's live desktop.

Toplevel export advertises buffer format/size and copies into a native buffer.
The reviewed image-copy protocol attaches a `wl_buffer`; this cannot supply
the application's GSK scene vocabulary. The primary
[Wayland capture protocol source](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/staging/ext-image-copy-capture/ext-image-copy-capture-v1.xml)
and installed generated headers describe that image boundary. Hyprland's
[IPC documentation](https://wiki.hypr.land/IPC/) supplies information/control
sockets and window events, which do not provide source-vector content.

The conclusion is limited to these reviewed hooks: they do not enable a strict
vector desktop adapter. Toplevel mapping handles could help a future trusted
bridge, but neither a title nor a window address binds a registered producer
incarnation to every popup, decoration and shell surface. No RUI app-only input
grant has been proven against this backend's native dispatch/cleanup boundary.
Potential virtual/IPC injection cannot inherit whole-session authorization.

The supported session remains the isolated backend. No additional profiles or
raw input are advertised for Hyprland, and its buffers are never converted into
wire paths or assets. A future cooperating-vector/compositor bridge must prove
trusted identities, topology, unequal scale/occlusion/stale-route behavior and
held-input cleanup on an owned nested session before enabling support. C4 permits
this negative feasibility result; it does not establish general desktop support.

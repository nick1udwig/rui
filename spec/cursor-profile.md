# Vector cursor profile

`cursor.vector.v1` defines visible cursors for a RUI Session.
Producers requiring these appearances must require this profile during negotiation; clients advertise it explicitly.
The CPU renderer enables it only through `RenderProfile`, and the GTK mirror enables it only when its connection selected the profile.
A default renderer continues to reject visible cursors.
Hidden cursors need no profile.

A cursor paints after all surfaces, clipped to the Session canvas.
Its local origin is `cursor.position - cursor.hotspot` in desktop logical coordinates.
The transmitted hotspot is authoritative for every appearance.
No conventional platform hotspot is substituted.
Surface transforms, opacity, clips and parent topology do not affect the cursor.
The observer viewport maps cursor geometry with the same pan and scale as desktop geometry.
More than one visible Session cursor is rejected by this renderer.

The Default, Pointer and Text shapes use the exact bundled closed RVP1 polygons in [fixtures/render/cursors](../fixtures/render/cursors/geometry.json).
That manifest specifies every vertex, byte length and descriptor-bound digest.
Coordinates are cursor-local logical units with positive y down.
Each polygon has a nonzero opaque linear-white fill and an opaque linear-black centered stroke of width 1, Butt caps, Miter joins, miter limit 4, and no dashes.
Stroke coverage wins where fill and stroke overlap; the shape is one opaque mask and is not blended twice.
These are authored vectors, independent of operating system themes, fonts, bitmap cursors or native cursor APIs.
Hidden paints nothing.

An Outline appearance references an authorized VectorPath version-1 RVP1 resource.
Its exact verified descriptor/body must be available in the current view's resource closure.
It paints an opaque linear-black nonzero fill with no stroke.
Open contours close implicitly, and the pinned renderer's usual path flattening and boundary rules apply.
GlyphOutline resources, unsupported versions, unsafe paths, missing grants or missing bodies fail the whole render.
This profile has no custom color, animation, embedded image or platform-cursor fallback.

Coverage, linear composition, display transfer and allocation/work ceilings are those of [renderer-profile.md](renderer-profile.md).
The bundled assets use the same safe bounded path compiler as authorized custom outlines.
Cursor tests assert white interiors, topmost composition, exact custom-outline pixels, hotspot translation, observer zoom, Session clipping, profile opt-in, missing resources and reservation cleanup.
A rendered cursor does not itself establish input authority or an EIS route.

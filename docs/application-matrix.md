# Application and capability matrix

This matrix covers cooperating application content, not arbitrary GTK programs.
Identity comes from registration. Window titles, labels and render-node positions
never choose native dispatch targets. All rows use GTK 4.22.4, Cairo 1.18.4,
Linux X11 on an authenticated owned Xvfb, scale 1 and C.UTF-8. Other scales,
locales, themes and native backends need their own comparisons before support.

| Application/version | Style/window | Visual coverage | Semantics/actions | Consistency/evidence |
| --- | --- | --- | --- | --- |
| Owned corpus v1 | constrained main content and registered menu/modal | Supported registered controls under the strict opaque style; partial opacity is declined | Registered text, button, stable list, canvas, popup/modal; Activate/Focus/ReplaceText/SetSelection/ScrollTo | Coherent same submitted node; [native interaction](../fixtures/gtk/native-interaction.json), [incremental replay](../fixtures/gtk/incremental-publications.json) |
| Owned corpus v1 | default Adwaita main and menu | Partial; rounded content clips, shadows, translucent paint and Cairo subtrees | Registered semantics remain available; incomplete visuals do not authorize raw input | Same-node capture, explicitly partial; [fresh census](../fixtures/gtk/coverage-inventory.json) |
| Source-negative panel v1 | photograph, texture, Cairo, blur, shader | Intentionally unsupported; native vector control complete | Informational only | Same-node source rejection and exporter traps in every coverage run |
| Document editor v1 (selected next target) | document list beside editable form, separate confirmation dialog; constrained style | Pending C3 evidence | Application-owned document state; generic registered actions and Unicode draft text | Must use reusable adapter with no `fixtures.gtk` import; currently unclaimed |
| Owned AT-SPI applications | observed accessible hierarchy | Unsupported visuals; native surface association unresolved | Read-only roles/names, redacted passwords; actions disabled | OBSERVED, bounded disposable worker; [adapter contract](../adapters/atspi/README.md) |

The [fresh submitted-node inventory](../fixtures/gtk/coverage-inventory.json)
contains per-style node counts, distinct decline reasons, complete/partial frames
and actual callback workflows. The historical 104 default-style gaps are repeated
occurrences across frames, not 104 distinct missing features. Native negative
controls deliberately retain unsupported source content. Constrained standalone
measurement can observe transient partial-opacity carets; those frames remain
partial. The interactive source disables caret blinking and animation.

| Priority/workflow | Gap | Required treatment |
| --- | --- | --- |
| Entry, button and popup under default style | nonopaque sRGB paint, partial opacity | Negotiated source-color/compositing extension or keep partial; current linear profile cannot faithfully substitute |
| Default button/entry/popup rounded decoration | rounded clips over nonuniform child content | General curved clipping extension with all reducers/renderers; a rounded background alone is insufficient |
| Default raised controls | outset shadows | Bounded effect vocabulary plus measured source semantics; no texture fallback |
| Constrained editor control borders | opaque square borders with differing side paint | Exact path lowering into existing primitives is possible; source comparison required in C2 |
| Photos, Cairo recordings, texture/color glyph/shader content | unavailable source vectors | Outside strict profile; explicit partial/unsupported, never sampled into resources |

Reproduce a coverage run and include its negative companion:

```sh
python3 fixtures/gtk/measure.py --xvfb /path/to/Xvfb --output /tmp/rui-census-NEW
python3 fixtures/gtk/negative_check.py --xvfb /path/to/Xvfb --output /tmp/rui-negative-NEW
python3 fixtures/gtk/inventory.py /tmp/rui-census-NEW/measurement.json \
  /tmp/rui-negative-NEW/negative-report.json --output /tmp/rui-inventory-NEW.json
python3 -m unittest fixtures.gtk.test_inventory
```

A workflow outside a supported row is an expected coverage limit. Missing or
incorrect registered content inside that row is a regression. New rows require
native source/action comparisons, independent replay and explicit exclusions.
